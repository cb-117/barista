//! Single-request local IPC: one newline-terminated JSON request, one reply.
//! Unix: a socket in a private (0700) directory, peer user checked. Windows: a named pipe whose
//! access list allows only this user, refusing remote clients. No TCP listener, ever.
use crate::models::*;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const MAX_CONNECTIONS: usize = 32;
pub type Handler = Arc<dyn Fn(ControlRequest) -> ControlReply + Send + Sync>;

/// The account's default control endpoint.
pub fn default_path() -> String {
    #[cfg(unix)]
    return format!("/tmp/com.connorbescos.barista-{}/control.sock", unsafe { libc::getuid() });
    #[cfg(windows)]
    return format!(
        r"\\.\pipe\com.connorbescos.barista-{}",
        win::current_user_sid().unwrap_or_else(|| "user".into())
    );
}

/// Where per-endpoint state (agent hook leases) lives.
pub fn state_dir(endpoint: &str) -> PathBuf {
    #[cfg(windows)]
    if let Some(name) = endpoint.strip_prefix(r"\\.\pipe\") {
        let safe: String = name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' })
            .collect();
        let base = dirs::data_local_dir().unwrap_or_else(std::env::temp_dir);
        return base.join("Barista").join(safe);
    }
    Path::new(endpoint)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Creates `path` (mode 700 on Unix) if needed and refuses anything not private to this user.
pub fn ensure_private_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt};
        if let Err(error) = std::fs::DirBuilder::new().mode(0o700).create(path) {
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(error.into());
            }
        }
        let info = std::fs::symlink_metadata(path)?;
        if !info.is_dir() || info.uid() != unsafe { libc::getuid() } || info.mode() & 0o077 != 0 {
            return Err(CoffeeError::msg(
                "The control directory must be a private, user-owned directory (mode 700).",
            ));
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        // Under the user's local app data, which only they (and administrators) can read.
        std::fs::create_dir_all(path)?;
        Ok(())
    }
}

/// Reads one newline-terminated message. SO_RCVTIMEO-style timeouts bound each read; this bounds
/// the whole message, so trickled bytes can't hold a worker.
pub(crate) fn read_message(stream: &mut impl TimedRead, limit: usize, deadline: Duration) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    let mut buffer = [0u8; 4096];
    let give_up = Instant::now() + deadline;
    while data.len() <= limit {
        let now = Instant::now();
        if now >= give_up {
            return Err(CoffeeError::msg("Control message took too long to arrive."));
        }
        // Each read waits at most the time left, so the whole message is bounded by `deadline`.
        stream.limit_next_read((give_up - now).min(Duration::from_secs(3)));
        let count = match stream.read(&mut buffer) {
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        };
        if count == 0 {
            return Err(CoffeeError::msg("Control connection closed before replying."));
        }
        if let Some(newline) = buffer[..count].iter().position(|b| *b == b'\n') {
            data.extend_from_slice(&buffer[..newline]);
            if data.len() > limit {
                break;
            }
            return Ok(data);
        }
        data.extend_from_slice(&buffer[..count]);
    }
    Err(CoffeeError::msg("Control message exceeds the size limit."))
}

/// A stream whose next read can be given a timeout.
pub(crate) trait TimedRead: Read {
    fn limit_next_read(&mut self, _timeout: Duration) {}
}

#[cfg(unix)]
impl TimedRead for std::os::unix::net::UnixStream {
    fn limit_next_read(&mut self, timeout: Duration) {
        // Best effort: some systems refuse socket options once the peer has closed, but the reply
        // may already be buffered; the 3-second timeout set on connect still applies.
        let _ = self.set_read_timeout(Some(timeout.max(Duration::from_millis(1))));
    }
}

/// Windows pipes have no read timeout; a watchdog cancels stuck reads instead.
#[cfg(windows)]
impl TimedRead for std::fs::File {}

/// Says why a connection over the cap is closed, without blocking on a slow client.
fn reply_busy(stream: &mut impl Write) {
    let _ = write_message(stream, &to_json(&ControlReply::failure("Barista is busy. Try again.")));
}

pub(crate) fn write_message(stream: &mut impl Write, json: &str) -> Result<()> {
    let mut payload = json.as_bytes().to_vec();
    payload.push(b'\n');
    stream.write_all(&payload)?;
    stream.flush()?;
    Ok(())
}

#[derive(Clone)]
pub struct ControlClient {
    pub path: String,
}

impl ControlClient {
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }

    pub fn send(&self, request: &ControlRequest) -> Result<ControlReply> {
        let mut stream = connect(&self.path)?;
        write_message(&mut stream, &to_json(request))?;
        let data = read_message(&mut stream, 262_144, Duration::from_secs(5))?;
        serde_json::from_slice(&data).map_err(|error| CoffeeError::msg(format!("Unexpected reply from Barista: {error}")))
    }
}

fn unreachable(error: &std::io::Error) -> bool {
    use std::io::ErrorKind::*;
    matches!(error.kind(), NotFound | ConnectionRefused)
}

#[cfg(unix)]
fn connect(path: &str) -> Result<std::os::unix::net::UnixStream> {
    let stream = std::os::unix::net::UnixStream::connect(path).map_err(|error| {
        if unreachable(&error) {
            CoffeeError::Unreachable("Barista is not running.".into())
        } else {
            CoffeeError::msg(error.to_string())
        }
    })?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    unix::check_peer(&stream)?;
    Ok(stream)
}

#[cfg(windows)]
fn connect(path: &str) -> Result<std::fs::File> {
    win::connect(path)
}

pub struct ControlServer {
    stopped: Arc<AtomicBool>,
    path: String,
}

impl ControlServer {
    /// Binds the endpoint and serves requests on background threads until `stop()`.
    pub fn start(path: &str, handler: Handler) -> Result<Self> {
        let stopped = Arc::new(AtomicBool::new(false));
        #[cfg(unix)]
        unix::serve(path, handler, stopped.clone())?;
        #[cfg(windows)]
        win::serve(path, handler, stopped.clone())?;
        Ok(Self {
            stopped,
            path: path.to_string(),
        })
    }

    pub fn stop(&self) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        #[cfg(unix)]
        {
            // Unlink synchronously, then wake the accept loop so it notices.
            let _ = std::fs::remove_file(&self.path);
            let _ = std::os::unix::net::UnixStream::connect(&self.path);
        }
        #[cfg(windows)]
        {
            let _ = std::fs::OpenOptions::new().read(true).write(true).open(&self.path);
        }
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Handles one accepted connection: read, decode, answer, close.
fn respond(stream: &mut (impl TimedRead + Write), handler: &Handler) {
    let reply = match read_message(stream, 65_536, Duration::from_secs(5)) {
        Ok(data) => match serde_json::from_slice::<ControlRequest>(&data) {
            Ok(request) => handler(request),
            Err(error) => ControlReply::failure(format!("Invalid control request: {error}")),
        },
        Err(error) => ControlReply::failure(format!("Invalid control request: {error}")),
    };
    let _ = write_message(stream, &to_json(&reply));
}

/// Bounds open connections and blocked workers, so a stuck or flooding client
/// can't exhaust descriptors or starve real heartbeats.
struct Slot(Arc<AtomicUsize>);
impl Slot {
    fn take(counter: &Arc<AtomicUsize>) -> Option<Slot> {
        let previous = counter.fetch_add(1, Ordering::SeqCst);
        if previous >= MAX_CONNECTIONS {
            counter.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Slot(counter.clone()))
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
    use std::os::unix::io::AsRawFd;
    use std::os::unix::net::{UnixListener, UnixStream};

    pub fn peer_uid(stream: &UnixStream) -> Option<u32> {
        let fd = stream.as_raw_fd();
        #[cfg(any(target_os = "linux", target_os = "android"))]
        unsafe {
            let mut credentials: libc::ucred = std::mem::zeroed();
            let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
            let result = libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut credentials as *mut _ as *mut libc::c_void,
                &mut length,
            );
            (result == 0).then_some(credentials.uid)
        }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        unsafe {
            let (mut user, mut group) = (0, 0);
            (libc::getpeereid(fd, &mut user, &mut group) == 0).then_some(user)
        }
    }

    pub fn check_peer(stream: &UnixStream) -> Result<()> {
        if peer_uid(stream) == Some(unsafe { libc::getuid() }) {
            Ok(())
        } else {
            Err(CoffeeError::msg("Only processes belonging to your account can use Barista."))
        }
    }

    pub fn serve(path: &str, handler: Handler, stopped: Arc<AtomicBool>) -> Result<()> {
        // A client that disconnects mid-reply must never kill the host process.
        unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };
        let directory = Path::new(path)
            .parent()
            .ok_or_else(|| CoffeeError::msg("Control socket path has no directory."))?;
        ensure_private_directory(directory)?;
        if let Ok(info) = std::fs::symlink_metadata(path) {
            if !info.file_type().is_socket() || info.uid() != unsafe { libc::getuid() } {
                return Err(CoffeeError::msg(
                    "Refusing to replace an unexpected file at the control socket path.",
                ));
            }
            match UnixStream::connect(path) {
                Ok(_) => return Err(CoffeeError::msg("Another Barista instance is already serving this account.")),
                Err(error) if unreachable(&error) => {
                    if let Err(error) = std::fs::remove_file(path) {
                        if error.kind() != std::io::ErrorKind::NotFound {
                            return Err(error.into());
                        }
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        let listener = UnixListener::bind(path)?;
        if let Err(error) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            // Only remove a socket successfully bound by this instance.
            let _ = std::fs::remove_file(path);
            return Err(error.into());
        }
        let slots = Arc::new(AtomicUsize::new(0));
        std::thread::Builder::new().name("barista-control".into()).spawn(move || {
            for connection in listener.incoming() {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                let mut stream = match connection {
                    Ok(stream) => stream,
                    Err(error) => {
                        // Out of descriptors: pause instead of spinning.
                        if matches!(error.raw_os_error(), Some(libc::EMFILE) | Some(libc::ENFILE)) {
                            std::thread::sleep(Duration::from_millis(100));
                        }
                        continue;
                    }
                };
                let Some(slot) = Slot::take(&slots) else {
                    if stream.set_nonblocking(true).is_ok() {
                        reply_busy(&mut stream);
                    }
                    continue;
                };
                let handler = handler.clone();
                std::thread::spawn(move || {
                    let _slot = slot;
                    let configured = stream.set_nonblocking(false).is_ok()
                        && stream.set_read_timeout(Some(Duration::from_secs(3))).is_ok()
                        && stream.set_write_timeout(Some(Duration::from_secs(3))).is_ok();
                    if !configured {
                        return;
                    }
                    if let Err(error) = check_peer(&stream) {
                        let _ = write_message(
                            &mut stream,
                            &to_json(&ControlReply::failure(format!("Invalid control request: {error}"))),
                        );
                        return;
                    }
                    respond(&mut stream, &handler);
                });
            }
        })?;
        Ok(())
    }
}

#[cfg(windows)]
pub(crate) mod win {
    use super::*;
    use std::ffi::c_void;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, LocalFree, ERROR_ACCESS_DENIED, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1, SE_KERNEL_OBJECT,
    };
    use windows_sys::Win32::Security::{
        EqualSid, GetTokenInformation, TokenUser, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY,
        TOKEN_USER,
    };
    use windows_sys::Win32::Storage::FileSystem::{FlushFileBuffers, FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, WaitNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
        PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows_sys::Win32::System::IO::CancelSynchronousIo;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain([0]).collect()
    }

    /// This process's user, as a token-owned buffer holding a TOKEN_USER.
    fn token_user() -> Option<Vec<u8>> {
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return None;
            }
            let mut length = 0u32;
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut length);
            let mut buffer = vec![0u8; length as usize];
            let ok = GetTokenInformation(token, TokenUser, buffer.as_mut_ptr() as *mut c_void, length, &mut length) != 0;
            CloseHandle(token);
            ok.then_some(buffer)
        }
    }

    fn sid_string(sid: PSID) -> Option<String> {
        unsafe {
            let mut text: *mut u16 = std::ptr::null_mut();
            if ConvertSidToStringSidW(sid, &mut text) == 0 {
                return None;
            }
            let length = (0..).take_while(|i| *text.add(*i) != 0).count();
            let value = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
            LocalFree(text as *mut c_void);
            Some(value)
        }
    }

    pub fn current_user_sid() -> Option<String> {
        let buffer = token_user()?;
        let user = unsafe { &*(buffer.as_ptr() as *const TOKEN_USER) };
        sid_string(user.User.Sid)
    }

    /// The pipe must be owned by this user, so another account can't squat on the name.
    fn check_owner(handle: HANDLE) -> Result<()> {
        let buffer = token_user().ok_or_else(|| CoffeeError::msg("Couldn't read this account's identity."))?;
        let user = unsafe { &*(buffer.as_ptr() as *const TOKEN_USER) };
        unsafe {
            let mut owner: PSID = std::ptr::null_mut();
            let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
            let status = GetSecurityInfo(
                handle,
                SE_KERNEL_OBJECT,
                OWNER_SECURITY_INFORMATION,
                &mut owner,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut descriptor,
            );
            let same = status == 0 && EqualSid(owner, user.User.Sid) != 0;
            if !descriptor.is_null() {
                LocalFree(descriptor);
            }
            if same {
                Ok(())
            } else {
                Err(CoffeeError::msg("Only processes belonging to your account can use Barista."))
            }
        }
    }

    pub fn connect(path: &str) -> Result<std::fs::File> {
        let deadline = Instant::now() + Duration::from_secs(3);
        // Between one client connecting and the server's next instance appearing, the name can
        // briefly vanish; retry "not found" for a moment before concluding Barista isn't running.
        let vanished_grace = Instant::now() + Duration::from_millis(250);
        loop {
            match std::fs::OpenOptions::new().read(true).write(true).open(path) {
                Ok(file) => {
                    check_owner(file.as_raw_handle() as HANDLE)?;
                    return Ok(file);
                }
                Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) && Instant::now() < deadline => unsafe {
                    WaitNamedPipeW(wide(path).as_ptr(), 500);
                },
                Err(error) if unreachable(&error) && Instant::now() < vanished_grace => std::thread::sleep(Duration::from_millis(25)),
                Err(error) if unreachable(&error) => return Err(CoffeeError::Unreachable("Barista is not running.".into())),
                Err(error) => return Err(error.into()),
            }
        }
    }

    struct Security {
        descriptor: PSECURITY_DESCRIPTOR,
    }
    unsafe impl Send for Security {}
    impl Drop for Security {
        fn drop(&mut self) {
            unsafe { LocalFree(self.descriptor) };
        }
    }

    /// Full access for this user only. "P" blocks inherited entries.
    fn security() -> Result<Security> {
        let sid = current_user_sid().ok_or_else(|| CoffeeError::msg("Couldn't read this account's identity."))?;
        let sddl = wide(&format!("D:P(A;;GA;;;{sid})"));
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut descriptor, std::ptr::null_mut())
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(Security { descriptor })
    }

    fn create_instance(name: &[u16], security: &Security, first: bool) -> std::io::Result<HANDLE> {
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: security.descriptor,
            bInheritHandle: 0,
        };
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 },
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                65_536,
                65_536,
                0,
                &attributes,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(handle)
        }
    }

    pub fn serve(path: &str, handler: Handler, stopped: Arc<AtomicBool>) -> Result<()> {
        let name = wide(path);
        let security = security()?;
        // The first instance claims the name; if it exists, another copy is serving this account.
        let first = create_instance(&name, &security, true).map_err(|error| {
            if error.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) || error.raw_os_error() == Some(231) {
                CoffeeError::msg("Another Barista instance is already serving this account.")
            } else {
                CoffeeError::from(error)
            }
        })?;
        let slots = Arc::new(AtomicUsize::new(0));
        let first = first as usize;
        std::thread::Builder::new().name("barista-control".into()).spawn(move || {
            let mut next = Some(first as HANDLE);
            loop {
                let handle = match next.take() {
                    Some(handle) => handle,
                    None => match create_instance(&name, &security, false) {
                        Ok(handle) => handle,
                        Err(_) => {
                            std::thread::sleep(Duration::from_millis(100));
                            continue;
                        }
                    },
                };
                let connected =
                    unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) } != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
                // Open the next instance right away, so the name never disappears while this one is busy.
                next = create_instance(&name, &security, false).ok();
                // SAFETY: we own this pipe handle; File closes it on drop.
                let mut pipe = unsafe { std::fs::File::from_raw_handle(handle as _) };
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                if !connected {
                    continue;
                }
                let Some(slot) = Slot::take(&slots) else {
                    reply_busy(&mut pipe); // Fits in the pipe's buffer, so this doesn't block.
                    continue;
                };
                let handler = handler.clone();
                let (done, finished) = std::sync::mpsc::channel::<()>();
                let worker = std::thread::spawn(move || {
                    let _slot = slot;
                    respond(&mut pipe, &handler);
                    unsafe { FlushFileBuffers(pipe.as_raw_handle() as HANDLE) };
                    let _ = done.send(());
                });
                // Blocking pipe reads have no timeout of their own: cancel a stuck one.
                let worker_handle = worker.as_raw_handle() as usize;
                std::thread::spawn(move || {
                    if finished.recv_timeout(Duration::from_secs(6)).is_err() && !worker.is_finished() {
                        unsafe { CancelSynchronousIo(worker_handle as HANDLE) };
                    }
                    let _ = worker.join();
                });
            }
        })?;
        Ok(())
    }
}
