#!/usr/bin/env python3
"""Smoke-test the Windows and Linux (Rust) build on any OS: CLI, command runner, MCP, and hooks,
against an isolated fake-power service. Uses only the CLI, so it also covers Windows named pipes.

    python3 Tests/portable_smoke.py --bin-dir rust/target/debug
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import uuid

parser = argparse.ArgumentParser()
parser.add_argument('--bin-dir', type=Path, required=True)
options = parser.parse_args()
suffix = '.exe' if os.name == 'nt' else ''
cli = str(options.bin_dir.resolve() / f'barista{suffix}')
app = str(options.bin_dir.resolve() / f'barista-app{suffix}')

with tempfile.TemporaryDirectory(prefix='caf-') as temporary:
    if os.name == 'nt':
        endpoint = r'\\.\pipe\barista-smoke-' + uuid.uuid4().hex
    else:
        os.chmod(temporary, 0o700)
        endpoint = str(Path(temporary) / 'control.sock')
    server = subprocess.Popen([app, '--serve', endpoint], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    mcp = None
    try:
        def invoke(*args, ok=True):
            result = subprocess.run([cli, *args, '--socket', endpoint, '--no-launch', '--json'], capture_output=True, text=True, timeout=15)
            reply = json.loads(result.stdout)
            assert reply['ok'] is ok, (args, reply, result.stderr)
            assert result.returncode == (0 if ok else 1), (args, result.returncode)
            return reply

        for _ in range(100):
            if server.poll() is not None:
                raise RuntimeError(server.stderr.read().decode())
            probe = subprocess.run([cli, 'status', '--socket', endpoint, '--no-launch', '--json'], capture_output=True, text=True, timeout=15)
            if probe.returncode == 0:
                break
            time.sleep(0.1)
        assert not invoke('status')['state']['awake']
        first = invoke('start', '--name', 'Build', '--seconds', '90')['session']['id']
        second = invoke('start', '--name', 'Slides', '--seconds', '90', '--display')['session']['id']
        state = invoke('status')['state']
        assert len(state['sessions']) == 2 and state['displayAwake']
        updated = invoke('heartbeat', first, '--progress', '0.5', '--message', 'Halfway')['session']
        assert updated['progress'] == 0.5 and updated['message'] == 'Halfway'
        invoke('stop', second)
        assert not invoke('status')['state']['displayAwake']
        invoke('heartbeat', first, '--progress', '2', ok=False)
        invoke('start', '--seconds', '-1', ok=False)
        duplicate = subprocess.run([app, '--serve', endpoint], capture_output=True, text=True, timeout=15)
        assert duplicate.returncode == 1 and 'already serving' in duplicate.stderr, duplicate
        print('PASS CLI: sessions, display aggregation, progress, validation, single instance', flush=True)

        command = subprocess.run([cli, 'run', '--name', 'Exit code', '--socket', endpoint, '--no-launch', '--', sys.executable, '-c', 'print("child-output"); raise SystemExit(7)'],
                                 capture_output=True, text=True, timeout=15)
        assert command.returncode == 7 and command.stdout.strip() == 'child-output', command
        state = invoke('status')['state']
        assert [s['id'] for s in state['sessions']] == [first]
        assert any(h['title'] == 'Exit code' and h['reason'] == 'released' for h in state['history'])
        child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'])
        watched = invoke('start', '--name', 'Watched', '--pid', str(child.pid), '--seconds', '60')['session']['id']
        child.kill(); child.wait()
        time.sleep(0.2)
        state = invoke('status')['state']
        assert watched not in [s['id'] for s in state['sessions']]
        assert any(h['id'] == watched and h['reason'] == 'processExited' for h in state['history'])
        print('PASS run and process watching: exit code preserved, own lease released, process exit ends session', flush=True)

        mcp = subprocess.Popen([cli, 'mcp', '--socket', endpoint, '--no-launch'], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
        def rpc(identifier, method, params=None):
            message = {'jsonrpc': '2.0', 'id': identifier, 'method': method}
            if params is not None:
                message['params'] = params
            mcp.stdin.write(json.dumps(message) + '\n'); mcp.stdin.flush()
            response = json.loads(mcp.stdout.readline())
            assert response['id'] == identifier, response
            return response
        assert rpc(1, 'initialize', {'protocolVersion': '2025-06-18'})['result']['protocolVersion'] == '2025-06-18'
        assert len(rpc(2, 'tools/list')['result']['tools']) == 6
        lease = rpc(3, 'tools/call', {'name': 'barista_acquire', 'arguments': {'title': 'MCP'}})['result']['structuredContent']['session']['id']
        assert not rpc(4, 'tools/call', {'name': 'barista_release', 'arguments': {'id': lease}})['result']['isError']
        print('PASS MCP: handshake, tools, acquire, release', flush=True)

        def hook(event, agent=None):
            payload = json.dumps({'session_id': 'smoke-1', 'hook_event_name': event, 'cwd': str(Path(temporary) / 'project')})
            extra = ['--agent', agent] if agent else []
            result = subprocess.run([cli, 'hook', *extra, '--socket', endpoint, '--no-launch'], input=payload, capture_output=True, text=True, timeout=15)
            assert result.returncode == 0, result
            return result.stdout
        def titled(title):
            return [s for s in invoke('status')['state']['sessions'] if s['title'] == title]
        assert hook('UserPromptSubmit') == ''
        assert len(titled('Claude Code · project')) == 1
        hook('PreToolUse')
        assert len(titled('Claude Code · project')) == 1
        hook('Stop')
        assert titled('Claude Code · project') == []
        hook('UserPromptSubmit', 'codex')
        assert len(titled('Codex · project')) == 1
        assert json.loads(hook('Stop', 'codex')) == {}
        assert titled('Codex · project') == []
        settings = Path(temporary) / 'settings.json'
        settings.write_text(json.dumps({'model': 'x'}))
        for action in ('install', 'status', 'uninstall'):
            result = subprocess.run([cli, 'hooks', action, '--settings', str(settings)], capture_output=True, text=True, timeout=15)
            assert result.returncode == 0, result
            if action == 'status':
                assert result.stdout.startswith('Connected'), result.stdout
        assert json.loads(settings.read_text()) == {'model': 'x'}
        print('PASS hooks: Claude and Codex turns, installer round trip', flush=True)
        invoke('stop', '--all')
        assert not invoke('status')['state']['awake']
        print('All portable smoke checks passed.', flush=True)
    finally:
        if mcp:
            mcp.stdin.close()
            mcp.wait(timeout=5)
        server.kill()
        server.wait(timeout=5)
