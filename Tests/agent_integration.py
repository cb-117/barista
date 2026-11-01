#!/usr/bin/env python3
"""Exercise the shipped CLI and stdio MCP against an isolated, fake-power service."""
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser()
parser.add_argument('--bin-dir', type=Path, default=ROOT / '.build/debug')
parser.add_argument('--service', type=Path, help='A binary that serves a fake-power control socket with --serve PATH '
                    '(default: BaristaChecks in --bin-dir; the Rust build uses barista-app)')
options = parser.parse_args()
cli = options.bin_dir.resolve() / 'barista'
checks = (options.service or options.bin_dir / 'BaristaChecks').resolve()

with tempfile.TemporaryDirectory(prefix='caf-', dir='/tmp') as temporary:
    path = str(Path(temporary) / 'control.sock')
    os.chmod(temporary, 0o700)
    server = subprocess.Popen([str(checks), '--serve', path], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    mcp = None
    child = None
    try:
        for _ in range(60):
            if Path(path).exists():
                break
            if server.poll() is not None:
                raise RuntimeError(server.stderr.read().decode())
            time.sleep(0.05)
        assert Path(path).exists(), 'test service failed to bind'
        assert Path(path).stat().st_mode & 0o777 == 0o600, 'socket must be private'

        def invoke(*args, ok=True):
            result = subprocess.run([str(cli), *args, '--socket', path, '--no-launch', '--json'], capture_output=True, text=True, timeout=10)
            reply = json.loads(result.stdout)
            assert reply['ok'] is ok, (args, reply, result.stderr)
            assert result.returncode == (0 if ok else 1), (args, result.returncode)
            return reply

        assert not invoke('status')['state']['awake']
        first = invoke('start', '--name', 'CLI build', '--seconds', '90')['session']['id']
        second = invoke('start', '--name', 'CLI presentation', '--seconds', '90', '--display')['session']['id']
        state = invoke('status')['state']
        assert len(state['sessions']) == 2 and state['displayAwake']
        updated = invoke('heartbeat', first, '--ttl', '90', '--progress', '0.6', '--message', 'Running tests')['session']
        assert updated['progress'] == 0.6 and updated['message'] == 'Running tests'
        invoke('stop', second)
        state = invoke('status')['state']
        assert len(state['sessions']) == 1 and not state['displayAwake']
        invoke('heartbeat', first, '--progress', '2', ok=False)
        invoke('start', '--seconds', '-1', ok=False)
        invoke('status', '--display', ok=False)
        print('PASS CLI: named sessions, display aggregation, progress, targeted stop, validation', flush=True)

        command = subprocess.run([str(cli), 'run', '--name', 'Exit-code check', '--socket', path, '--no-launch', '--', sys.executable, '-c', 'print("child-output"); raise SystemExit(7)'], capture_output=True, text=True, timeout=10)
        assert command.returncode == 7 and command.stdout == 'child-output\n', command
        state = invoke('status')['state']
        assert [s['id'] for s in state['sessions']] == [first]
        assert any(h['title'] == 'Exit-code check' and h['reason'] == 'released' for h in state['history'])
        print('PASS command runner: output and exit code preserved, own lease released', flush=True)

        child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'])
        watched = invoke('start', '--name', 'Watched process', '--pid', str(child.pid), '--seconds', '60')['session']['id']
        child.terminate()
        child.wait(timeout=3)
        state = invoke('status')['state']
        assert watched not in [s['id'] for s in state['sessions']]
        assert any(h['id'] == watched and h['reason'] == 'processExited' for h in state['history'])
        short = invoke('start', '--name', 'Short lease', '--seconds', '0.2')['session']['id']
        time.sleep(0.3)
        invoke('heartbeat', short, ok=False)
        print('PASS lifecycle: process exit and stale heartbeats end sessions', flush=True)

        mcp = subprocess.Popen([str(cli), 'mcp', '--socket', path, '--no-launch'], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, bufsize=1)
        def rpc(identifier, method, params=None):
            message = {'jsonrpc': '2.0', 'id': identifier, 'method': method}
            if params is not None:
                message['params'] = params
            mcp.stdin.write(json.dumps(message) + '\n')
            mcp.stdin.flush()
            response = json.loads(mcp.stdout.readline())
            assert response['id'] == identifier, response
            return response

        initialized = rpc(1, 'initialize', {'protocolVersion': '2025-11-25', 'capabilities': {}, 'clientInfo': {'name': 'integration-check', 'version': '1'}})['result']
        assert initialized['protocolVersion'] == '2025-11-25' and 'tools' in initialized['capabilities']
        mcp.stdin.write(json.dumps({'jsonrpc': '2.0', 'method': 'notifications/initialized'}) + '\n')
        mcp.stdin.flush()
        tools = rpc(2, 'tools/list')['result']['tools']
        assert len(tools) == 6 and all(t['inputSchema']['additionalProperties'] is False for t in tools)
        assert rpc('ping', 'ping')['result'] == {}
        result = rpc(3, 'tools/call', {'name': 'barista_acquire', 'arguments': {'title': 'MCP test', 'ttl_seconds': 60}})['result']
        assert not result['isError']
        lease = result['structuredContent']['session']['id']
        heartbeat = rpc(4, 'tools/call', {'name': 'barista_heartbeat', 'arguments': {'id': lease, 'progress': 0.75, 'message': 'Verifying'}})['result']
        assert heartbeat['structuredContent']['session']['progress'] == 0.75
        invalid = rpc(5, 'tools/call', {'name': 'barista_acquire', 'arguments': {'title': 'Invalid boolean', 'ttl_seconds': True}})['result']
        assert invalid['isError']
        assert 'error' in rpc(6, 'tools/call', {'name': 'barista_status', 'arguments': {'unexpected': 1}})
        assert rpc(7, 'unsupported/method')['error']['code'] == -32601
        released = rpc(8, 'tools/call', {'name': 'barista_release', 'arguments': {'id': lease}})['result']
        assert not released['isError']
        assert [s['id'] for s in invoke('status')['state']['sessions']] == [first]
        mcp.stdin.write('not-json\n')
        mcp.stdin.flush()
        assert json.loads(mcp.stdout.readline())['error']['code'] == -32700
        print('PASS MCP: handshake, discovery, acquire, progress, release, schema checks, protocol errors', flush=True)

        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
            connection.settimeout(3)
            connection.connect(path)
            connection.sendall(b'{"broken":true}\n')
            response = json.loads(connection.makefile('rb').readline())
            assert response['ok'] is False
        assert invoke('status')['state']['awake'], 'malformed clients must not disrupt live sessions'
        print('PASS local transport: private permissions, malformed-request isolation', flush=True)

        def hook(event, session_id='claude-test-1', raw=None):
            payload = raw if raw is not None else json.dumps({'session_id': session_id, 'hook_event_name': event, 'cwd': '/tmp/my-project'})
            result = subprocess.run([str(cli), 'hook', '--socket', path, '--no-launch'], input=payload, capture_output=True, text=True, timeout=10)
            assert result.returncode == 0 and result.stdout == '', (event, result)
        def claude_sessions():
            return [s for s in invoke('status')['state']['sessions'] if s['title'] == 'Claude Code · my-project']

        hook('UserPromptSubmit')
        [lease] = claude_sessions()
        assert lease['source'] == 'agent' and lease['processID'] == os.getpid() and not lease['keepDisplayAwake']
        hook('PreToolUse'); hook('PostToolUse')
        assert [s['id'] for s in claude_sessions()] == [lease['id']], 'tool calls renew, not duplicate'
        hook('Stop')
        assert claude_sessions() == []
        assert any(h['id'] == lease['id'] and h['reason'] == 'released' for h in invoke('status')['state']['history'])
        hook('UserPromptSubmit')
        [lease] = claude_sessions()
        invoke('stop', '--all')  # A user stop, as from the app; 'stop ID' is an owner release.
        assert any(h['id'] == lease['id'] and h['reason'] == 'stopped' for h in invoke('status')['state']['history'])
        hook('PreToolUse'); hook('PostToolUse')
        assert claude_sessions() == [], 'tool calls must not override a user stop'
        hook('UserPromptSubmit')
        assert len(claude_sessions()) == 1, 'a new prompt is new user-initiated work'
        hook('UserPromptSubmit', session_id='claude-test-2')
        assert len(claude_sessions()) == 2, 'separate conversations get separate sessions'
        hook('SessionEnd'); hook('SessionEnd', session_id='claude-test-2')
        assert claude_sessions() == []
        # Claude's Stop ends the turn: a late tool event must not start another session.
        hook('UserPromptSubmit'); hook('Stop'); hook('PostToolUse')
        assert claude_sessions() == [], 'late tool events must not revive a finished turn'
        # A user stop is respected even after it scrolls out of the 20-entry history.
        hook('UserPromptSubmit')
        invoke('stop', '--all')
        for index in range(21):
            invoke('stop', invoke('start', '--name', f'filler {index}', '--seconds', '60')['session']['id'])
        hook('PreToolUse')
        assert claude_sessions() == [], 'tool calls must not override an old user stop'
        hook('UserPromptSubmit')
        assert len(claude_sessions()) == 1
        hook('SessionEnd')
        assert claude_sessions() == []
        # A turn that lapsed during a long tool call resumes on its next tool event...
        state_file = Path(path).parent / 'hooks' / 'claude-test-1'
        def lapsed_turn():
            lapsed = invoke('start', '--name', 'Lapsing turn', '--seconds', '1')['session']['id']
            time.sleep(1.5)
            state_file.write_text(f"{lapsed} {int(time.time())} {invoke('status')['state']['stopToken']}")
        lapsed_turn(); hook('PostToolUse')
        assert len(claude_sessions()) == 1, 'a lapsed turn resumes within its cap'
        hook('Stop')
        # ...but never after Stop all, even though its old lease ended as "expired".
        lapsed_turn(); invoke('stop', '--all'); hook('PostToolUse')
        assert claude_sessions() == [], 'Stop all revokes a lapsed turn'
        # Stop while one of Claude's background commands still runs keeps the Mac awake for it...
        hook('UserPromptSubmit')
        background = subprocess.Popen(['/bin/sh', '-c', 'sleep 20; : /.claude/shell-snapshots/test'])
        # e.g. an MCP server: never counts, even when its environment mentions a shell snapshot.
        # (A non-Apple binary: macOS hides the environment of its own system binaries.)
        other_child = subprocess.Popen([str(checks), '--serve', str(Path(temporary) / 'mcp-like.sock')],
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                       env={**os.environ, 'SNAPSHOT': '/.claude/shell-snapshots/env'})
        try:
            hook('Stop')
            [waiting] = claude_sessions()
            assert waiting['message'] == 'Waiting on background work', waiting
            hook('PreToolUse')  # Claude resumes when the command reports back.
            [working] = claude_sessions()
            assert working['id'] == waiting['id'] and working['message'] == 'Working'
            hook('SessionEnd')
            assert claude_sessions() == []
            background.kill(); background.wait()
            # ...but other children, like MCP servers, never do.
            hook('UserPromptSubmit'); hook('Stop')
            assert claude_sessions() == [], 'only Claude background commands keep a finished turn awake'
        finally:
            for child in (background, other_child):
                child.kill(); child.wait()
        hook('UserPromptSubmit', raw='not json')
        hook('UserPromptSubmit', session_id='../escape')
        assert claude_sessions() == []
        # Cold start: hooks create the private control directory before Barista ever ran.
        cold = Path(temporary) / 'cold' / 'control.sock'
        result = subprocess.run([str(cli), 'hook', '--socket', str(cold), '--no-launch'], input=json.dumps({'session_id': 'cold-1', 'hook_event_name': 'UserPromptSubmit'}), capture_output=True, text=True, timeout=10)
        assert result.returncode == 0 and result.stdout == ''
        assert (cold.parent / 'hooks').is_dir() and cold.parent.stat().st_mode & 0o777 == 0o700, 'hook must get past its lock on a fresh account'
        print('PASS Claude Code hooks: prompt acquires, tools renew, stop releases, background commands kept awake, user stop respected, no revival, cold start, silent stdout', flush=True)

        settings = Path(temporary) / 'claude' / 'settings.json'
        settings.parent.mkdir()
        settings.write_text(json.dumps({'model': 'opus', 'hooks': {'Stop': [{'hooks': [{'type': 'command', 'command': 'say done'}, {'type': 'command', 'command': 'echo barista hook'}]}]}}))
        def hooks_cmd(*args):
            result = subprocess.run([str(cli), 'hooks', *args, '--settings', str(settings)], capture_output=True, text=True, timeout=10)
            assert result.returncode == 0, result
            return result.stdout
        assert hooks_cmd('status').startswith('Not connected')
        hooks_cmd('install'); hooks_cmd('install')
        config = json.loads(settings.read_text())
        assert config['model'] == 'opus'
        ours = lambda groups: [h for g in groups for h in g['hooks'] if h['command'].endswith(' hook') and not h['command'].startswith('echo')]
        for event in ['UserPromptSubmit', 'PreToolUse', 'PostToolUse', 'Stop', 'SessionEnd']:
            assert len(ours(config['hooks'][event])) == 1, (event, config)
        assert any(h['command'] == 'say done' for g in config['hooks']['Stop'] for h in g['hooks'])
        assert hooks_cmd('status').startswith('Connected')
        assert (settings.parent / 'settings.json.barista-backup').exists()
        hooks_cmd('uninstall')
        assert json.loads(settings.read_text()) == {'model': 'opus', 'hooks': {'Stop': [{'hooks': [{'type': 'command', 'command': 'say done'}, {'type': 'command', 'command': 'echo barista hook'}]}]}}, 'unrelated hooks ending in "barista hook" must survive'
        # A partial install (only the Stop hook) must not report as connected.
        hooks_cmd('install')
        partial = json.loads(settings.read_text())
        for event in ['UserPromptSubmit', 'PreToolUse', 'PostToolUse', 'SessionEnd']:
            del partial['hooks'][event]
        settings.write_text(json.dumps(partial))
        assert hooks_cmd('status').startswith('Needs repair')
        hooks_cmd('install'); assert hooks_cmd('status').startswith('Connected')
        hooks_cmd('uninstall')
        settings.write_text('{broken')
        assert subprocess.run([str(cli), 'hooks', 'install', '--settings', str(settings)], capture_output=True, timeout=10).returncode == 1
        assert settings.read_text() == '{broken', 'invalid settings are never overwritten'
        print('PASS hook installer: idempotent, preserves other settings and hooks, clean uninstall, refuses invalid JSON', flush=True)

        # Codex: same hook shape in ~/.codex/hooks.json, its own sessions, JSON on Stop.
        codex_settings = Path(temporary) / 'codex' / 'hooks.json'
        codex_settings.parent.mkdir()
        codex_settings.write_text(json.dumps({'hooks': {'Stop': [{'hooks': [{'type': 'command', 'command': 'notify-send done'}]}]}}))
        def codex_hooks_cmd(*args):
            result = subprocess.run([str(cli), 'hooks', *args, '--agent', 'codex', '--settings', str(codex_settings)], capture_output=True, text=True, timeout=10)
            assert result.returncode == 0, result
            return result.stdout
        assert codex_hooks_cmd('status').startswith('Not connected')
        assert '/hooks' in codex_hooks_cmd('install'), 'install must explain the one-time trust step'
        codex_hooks_cmd('install')
        codex_config = json.loads(codex_settings.read_text())
        for event in ['UserPromptSubmit', 'PreToolUse', 'PostToolUse', 'Stop', 'SessionEnd']:
            commands = [h for g in codex_config['hooks'][event] for h in g['hooks'] if h['command'].endswith(' hook --agent codex')]
            assert len(commands) == 1, (event, codex_config)
            assert commands[0]['timeout'] == (3 if event == 'SessionEnd' else 10)
        assert any(h['command'] == 'notify-send done' for g in codex_config['hooks']['Stop'] for h in g['hooks'])
        assert codex_hooks_cmd('status').startswith('Connected')
        assert hooks_cmd('status').startswith('Not connected'), 'Codex and Claude settings are independent'
        def codex_hook(event, session_id='thr_codex_1'):
            payload = json.dumps({'session_id': session_id, 'hook_event_name': event, 'cwd': '/tmp/my-project', 'turn_id': 't1', 'model': 'gpt'})
            result = subprocess.run([str(cli), 'hook', '--agent', 'codex', '--socket', path, '--no-launch'], input=payload, capture_output=True, text=True, timeout=10)
            assert result.returncode == 0, (event, result)
            return result.stdout
        def codex_sessions():
            return [s for s in invoke('status')['state']['sessions'] if s['title'] == 'Codex · my-project']
        assert codex_hook('UserPromptSubmit') == '', 'UserPromptSubmit stdout would be added to the model context'
        [codex_lease] = codex_sessions()
        assert codex_hook('PreToolUse') == '' and codex_sessions()[0]['message'] == 'Working'
        hook('UserPromptSubmit', session_id='thr_codex_1')  # a Claude session that happens to share the ID
        assert len(codex_sessions()) == 1 and len(claude_sessions()) == 1, 'agents never share a lease'
        hook('SessionEnd', session_id='thr_codex_1')
        assert json.loads(codex_hook('Stop')) == {}, 'Codex Stop hooks must print JSON'
        assert codex_sessions() == []
        assert any(h['id'] == codex_lease['id'] and h['reason'] == 'released' for h in invoke('status')['state']['history'])
        codex_hook('UserPromptSubmit'); assert codex_hook('SessionEnd') == '' and codex_sessions() == []
        codex_hooks_cmd('uninstall')
        assert json.loads(codex_settings.read_text()) == {'hooks': {'Stop': [{'hooks': [{'type': 'command', 'command': 'notify-send done'}]}]}}
        codex_settings.unlink(); codex_hooks_cmd('install'); codex_hooks_cmd('uninstall')
        assert not codex_settings.exists(), 'a hooks.json that only held Barista hooks is removed on disconnect'
        print('PASS Codex hooks: installer, separate sessions, JSON Stop reply, silent prompt hook, clean disconnect', flush=True)

        # barista setup connects every agent found for this user, then confirms Barista is running.
        home = Path(temporary) / 'home'
        (home / '.claude').mkdir(parents=True); (home / '.codex').mkdir()
        env = {**os.environ, 'HOME': str(home), 'CFFIXED_USER_HOME': str(home)}
        result = subprocess.run([str(cli), 'setup', '--socket', path, '--no-launch'], capture_output=True, text=True, timeout=10, env=env)
        assert result.returncode == 0, result
        assert 'Connected Claude Code' in result.stdout and 'Connected Codex' in result.stdout and '/hooks' in result.stdout, result.stdout
        assert (home / '.claude' / 'settings.json').exists() and (home / '.codex' / 'hooks.json').exists()
        only_claude = Path(temporary) / 'home2'
        (only_claude / '.claude').mkdir(parents=True)
        result = subprocess.run([str(cli), 'setup', '--socket', path, '--no-launch'], capture_output=True, text=True, timeout=10,
                                env={**os.environ, 'HOME': str(only_claude), 'CFFIXED_USER_HOME': str(only_claude)})
        assert result.returncode == 0 and 'Codex' not in result.stdout and not (only_claude / '.codex').exists(), result
        print('PASS setup: connects the agents it finds, leaves others alone', flush=True)

        first = invoke('start', '--name', 'CLI build', '--seconds', '90')['session']['id']
        # A periodic heartbeat must not undo an explicit user stop.
        wrapped = subprocess.Popen([str(cli), 'run', '--name', 'User-stopped command', '--socket', path, '--no-launch', '--', sys.executable, '-c', 'import time; time.sleep(12)'], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            task = None
            for _ in range(40):
                task = next((s for s in invoke('status')['state']['sessions'] if s['title'] == 'User-stopped command'), None)
                if task:
                    break
                time.sleep(0.05)
            assert task is not None
            invoke('stop', task['id'])
            time.sleep(10.5)
            assert task['id'] not in [s['id'] for s in invoke('status')['state']['sessions']]
            assert wrapped.wait(timeout=5) == 0
        finally:
            if wrapped.poll() is None:
                wrapped.terminate()
                wrapped.wait(timeout=5)
        print('PASS user control: stopping a command lease stays stopped through automatic heartbeats', flush=True)
        invoke('stop', first)
        assert not invoke('status')['state']['awake']

        server.terminate()
        server.wait(timeout=5)
        # A crash leaves a stale socket. The next service may safely recover its own socket.
        server = subprocess.Popen([str(checks), '--serve', path], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        for _ in range(50):
            result = subprocess.run([str(cli), 'status', '--socket', path, '--no-launch', '--json'], capture_output=True, text=True, timeout=5)
            if result.returncode == 0:
                break
            time.sleep(0.05)
        assert result.returncode == 0 and not json.loads(result.stdout)['state']['awake']
        print('PASS service recovery: stale sockets recover; old awake sessions never resume', flush=True)
        print('All CLI and MCP integration checks passed.', flush=True)
    finally:
        if mcp:
            mcp.terminate()
            mcp.wait(timeout=5)
        if child and child.poll() is None:
            child.terminate()
            child.wait(timeout=5)
        if server.poll() is None:
            server.terminate()
            server.wait(timeout=5)
