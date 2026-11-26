"""Exercise Windows automatic Setup and the tray's session handoff on a disposable CI runner."""
import argparse
import datetime
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument('--setup', type=Path, required=True)
parser.add_argument('--installed', type=Path, required=True)
options = parser.parse_args()

# The app finds its state with the Known Folders API, which ignores the LOCALAPPDATA override below.
sid = subprocess.run(['whoami', '/user', '/fo', 'csv', '/nh'], capture_output=True, text=True,
                     check=True).stdout.strip().split(',')[-1].strip('"')
handoff = Path(os.environ['LOCALAPPDATA']) / 'Barista' / f'com.connorbescos.barista-{sid}' / 'update-handoff.json'

with tempfile.TemporaryDirectory(prefix='Barista update ') as temp:
    root = Path(temp)
    portable = root / 'portable copy'
    portable.mkdir()
    for name in ['barista.exe', 'barista-app.exe']:
        shutil.copy2(options.installed / name, portable / name)
    env = {**os.environ, 'APPDATA': str(root / 'roaming'), 'LOCALAPPDATA': str(root / 'local'),
           'BARISTA_DATA_DIR': str(root / 'usage'), 'CODEX_HOME': str(root / 'codex'),
           'CLAUDE_CONFIG_DIR': str(root / 'claude'), 'BARISTA_CLAUDE_CREDENTIALS': str(root / 'absent.json'),
           'BARISTA_CODEX_AUTH': str(root / 'absent.json')}
    prefs = root / 'roaming' / 'Barista'
    prefs.mkdir(parents=True)
    (prefs / 'settings.json').write_text(json.dumps({'checksForUpdates': False, 'shareActiveCount': False,
        'launchAtLoginChoiceMade': True, 'watchCowork': False, 'watchChatgpt': False,
        'claudeAccountLimits': False, 'codexAccountLimits': False, 'batteryThreshold': 0}))
    for folder in (options.installed, portable):
        cli = folder / 'barista.exe'
        app = folder / 'barista-app.exe'
        token = 'b636ba20-a842-4059-be22-29a3e86a8cb4'
        lease_id = '21c5d766-28a9-4092-bfec-12f1cac4a735'
        stamp = datetime.datetime.now(datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')
        lease = {'id': lease_id, 'title': 'Update integration', 'source': 'manual', 'startedAt': stamp,
                 'lastHeartbeat': stamp, 'keepDisplayAwake': False}
        def start(leases, resume=True):
            # What the previous tray writes before it exits; whichever copy binds the pipe claims it.
            handoff.parent.mkdir(parents=True, exist_ok=True)
            handoff.write_text(json.dumps({'saved_at': time.time(), 'sessions': leases, 'stop_token': token,
                'activity': {}, 'leases': {}, 'stopped': [], 'burst': {}}))
            flags = ['--resume-update'] if resume else []
            return subprocess.Popen([str(app), '--background', '--headless', *flags], stdin=subprocess.DEVNULL, env=env)
        def status(serving=None):
            result = None
            for _ in range(100):
                result = subprocess.run([str(cli), 'status', '--json', '--no-launch'], env=env, capture_output=True, timeout=10)
                if result.returncode == 0:
                    state = json.loads(result.stdout)['state']
                    assert state['stopToken'] == token and state['sessions'][0]['id'] == lease_id, state
                    assert not handoff.exists(), 'The serving copy left the handoff behind'
                    return state
                time.sleep(.1)
            exited = None if serving is None else serving.poll()
            raise AssertionError(f'Barista did not resume the update session (app exit code {exited}, status exit code '
                                 f'{result.returncode}): {result.stdout.decode(errors="replace")} {result.stderr.decode(errors="replace")}')
        # A copy an agent hook launches mid-update, without --resume-update, still takes the handoff.
        old = start([lease], resume=False)
        mcp = subprocess.Popen([str(cli), 'mcp', '--no-launch'], stdin=subprocess.PIPE, stdout=subprocess.PIPE, env=env)
        fresh = None
        try:
            status(old)
            args = [str(options.setup), '/BARISTAAUTOUPDATE', '/CURRENTUSER', '/VERYSILENT', '/SUPPRESSMSGBOXES',
                    '/NORESTART', '/NOLAUNCH', '/DIR=' + str(folder)]
            if folder == portable:
                args.append('/PORTABLEUPDATE')
            subprocess.run(args, check=True, timeout=180)
            assert old.poll() is None and mcp.poll() is None, 'Setup interrupted a running process'
            assert (folder / 'unins000.exe').exists() == (folder == options.installed)
            fresh = start(status(old)['sessions'])
            old.terminate()
            old.wait(timeout=10)
            assert status(fresh)['awake'] and fresh.poll() is None
            print('PASS Windows automatic installer and session handoff:', folder.name, flush=True)
        finally:
            for process in [fresh, mcp, old]:
                if process is not None and process.poll() is None:
                    process.terminate()
                    process.wait(timeout=10)
