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
        def start(leases):
            process = subprocess.Popen([str(app), '--background', '--headless', '--resume-update'], stdin=subprocess.PIPE, env=env)
            process.stdin.write(json.dumps({'saved_at': time.time(), 'sessions': leases, 'stop_token': token,
                'activity': {}, 'leases': {}, 'stopped': [], 'burst': {}}).encode())
            process.stdin.close()
            return process
        def status():
            for _ in range(60):
                result = subprocess.run([str(cli), 'status', '--json', '--no-launch'], env=env, capture_output=True, timeout=10)
                if result.returncode == 0:
                    state = json.loads(result.stdout)['state']
                    assert state['stopToken'] == token and state['sessions'][0]['id'] == lease_id, state
                    return state
                time.sleep(.1)
            raise AssertionError('Barista did not resume the update session')
        old = start([lease])
        mcp = subprocess.Popen([str(cli), 'mcp', '--no-launch'], stdin=subprocess.PIPE, stdout=subprocess.PIPE, env=env)
        fresh = None
        try:
            status()
            args = [str(options.setup), '/BARISTAAUTOUPDATE', '/CURRENTUSER', '/VERYSILENT', '/SUPPRESSMSGBOXES',
                    '/NORESTART', '/NOLAUNCH', '/DIR=' + str(folder)]
            if folder == portable:
                args.append('/PORTABLEUPDATE')
            subprocess.run(args, check=True, timeout=180)
            assert old.poll() is None and mcp.poll() is None, 'Setup interrupted a running process'
            assert (folder / 'unins000.exe').exists() == (folder == options.installed)
            fresh = start(status()['sessions'])
            old.terminate()
            old.wait(timeout=10)
            assert status()['awake'] and fresh.poll() is None
            print('PASS Windows automatic installer and session handoff:', folder.name, flush=True)
        finally:
            for process in [fresh, mcp, old]:
                if process is not None and process.poll() is None:
                    process.terminate()
                    process.wait(timeout=10)
