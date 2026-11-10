#!/usr/bin/env python3
"""Check `barista usage` and the Cursor hooks against the shared fixture (see docs/USAGE.md).

    python3 Tests/usage_check.py --bin-dir .build/debug          # Swift (macOS)
    python3 Tests/usage_check.py --bin-dir rust/target/debug     # Rust (any OS)
"""
import argparse
import datetime
import glob
import http.server
import threading
import time
import json
import os
import re
import shlex
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / 'Tests' / 'fixtures' / 'usage'
parser = argparse.ArgumentParser()
parser.add_argument('--bin-dir', type=Path, required=True)
options = parser.parse_args()
cli = str(options.bin_dir.resolve() / ('barista.exe' if os.name == 'nt' else 'barista'))
expected = json.loads((FIXTURE / 'expected.json').read_text())


def run(home, data, *args, stdin=None, ok=True, tz=None, extra=None):
    env = {**os.environ, 'HOME': str(home), 'USERPROFILE': str(home), 'CFFIXED_USER_HOME': str(home),
           'BARISTA_DATA_DIR': str(data)}
    # Explicit locations work on every OS (Windows finds the home folder without HOME).
    env['CLAUDE_CONFIG_DIR'] = str(home / '.claude')
    env['CODEX_HOME'] = str(home / '.codex')
    env.pop('XDG_DATA_HOME', None)
    env['BARISTA_CLAUDE_APP_DIR'] = str(Path(data) / 'claude-app')
    if tz:
        env['TZ'] = tz
    env.update(extra or {})
    result = subprocess.run([cli, *args], input=stdin, capture_output=True, text=True, timeout=60, env=env)
    assert (result.returncode == 0) == ok, (args, result)
    return result


def report(home, data, period='all'):
    reply = json.loads(run(home, data, 'usage', '--period', period, '--json').stdout)
    assert [p['id'] for p in reply['periods']] == [period], reply
    assert 'estimate' in reply['estimateNote'].lower()
    assert [l['agent'] for l in reply['limits']] == ['claude', 'codex', 'cursor'], reply['limits']
    return reply


def usage(home, data, period='all', tz=None):
    if tz:
        reply = json.loads(run(home, data, 'usage', '--period', period, '--json', tz=tz).stdout)
        return reply['periods'][0]
    return report(home, data, period)['periods'][0]


def installed_statusline(home, data, settings, stdin):
    """Runs the status line command exactly as Claude Code would, from the settings file."""
    command = shlex.split(json.loads(Path(settings).read_text())['statusLine']['command'])
    assert Path(command[0]).name.startswith('barista') and command[1] == 'statusline', command
    return run(home, data, *command[1:], stdin=stdin).stdout.strip()


def close(a, b):
    return a is None and b is None or (a is not None and b is not None and abs(a - b) < 1e-9)


def same_path(a, b):
    return os.path.normcase(os.path.normpath(a)) == os.path.normcase(os.path.normpath(b))


def check_projects(period, want, root):
    assert period['activeSeconds'] == want['activeSeconds'], period['activeSeconds']
    got = period['projects']
    assert [p['name'] for p in got] == [p['name'] for p in want['projects']], [(p['name'], p['project']) for p in got]
    for project, spec in zip(got, want['projects']):
        if spec['path'] is None:
            assert project['project'] is None, project
        else:
            assert same_path(project['project'], str(root / spec['path'])), (project['project'], spec['path'])
        for key, value in spec.items():
            if key in ('name', 'path'):
                continue
            if key == 'agents':
                assert [a['agent'] for a in project['agents']] == [a['agent'] for a in value], (project['name'], project['agents'])
                for agent, fields in zip(project['agents'], value):
                    for field, number in fields.items():
                        assert close(agent[field], number) if field == 'estimatedCost' else agent[field] == number, (project['name'], agent, field)
            elif key == 'models':
                assert [[m['agent'], m['model']] for m in project['models']] == value, (project['name'], project['models'])
            elif key == 'days':
                assert len(project['days']) == len(value), (project['name'], project['days'])
                for day, fields in zip(project['days'], value):
                    for field, number in fields.items():
                        assert close(day[field], number) if field == 'estimatedCost' else day[field] == number, (project['name'], day, field)
            elif key == 'estimatedCost':
                assert close(project[key], value), (project['name'], key, project[key], value)
            else:
                assert project[key] == value, (project['name'], key, project[key], value)


def check(period, want):
    assert period['totalTokens'] == want['totalTokens'], period['totalTokens']
    assert close(period['estimatedCost'], want['estimatedCost']), period['estimatedCost']
    agents = period['agents']
    assert [a['agent'] for a in agents] == ['claude', 'codex', 'cursor'], agents
    for agent in agents:
        spec = want['agents'][agent['agent']]
        for key, value in spec.items():
            if key == 'models':
                assert [m['model'] for m in agent['models']] == [m['model'] for m in value], (agent['agent'], agent['models'])
                for got, model in zip(agent['models'], value):
                    for field, number in model.items():
                        assert close(got[field], number) if field == 'estimatedCost' else got[field] == number, (agent['agent'], got, field)
            elif key == 'estimatedCost':
                assert close(agent[key], value), (agent['agent'], key, agent[key], value)
            else:
                assert agent[key] == value, (agent['agent'], key, agent[key], value)


with tempfile.TemporaryDirectory(prefix='barista-usage-') as temporary:
    temporary = Path(temporary)
    home, data = temporary / 'home', temporary / 'data'
    shutil.copytree(FIXTURE / 'home', home)
    shutil.copytree(FIXTURE / 'data', data)
    # Project folders: alpha is a Git repository, alpha-wt a linked worktree of it, beta a plain folder.
    root = temporary / 'work'
    (root / 'alpha' / '.git' / 'worktrees' / 'wt').mkdir(parents=True)
    (root / 'alpha' / 'src').mkdir()
    (root / 'alpha-wt').mkdir()
    (root / 'alpha-wt' / '.git').write_text('gitdir: ../alpha/.git/worktrees/wt\n')  # Relative, as some tools write it.
    (root / 'beta').mkdir()
    for path in [*home.rglob('*.jsonl'), *data.rglob('*.jsonl')]:
        path.write_text(path.read_text().replace('__ROOT__', json.dumps(str(root))[1:-1]))

    check(usage(home, data), expected)
    check(usage(home, data), expected)  # Second run reads the cache.
    caches = [p.name for p in data.iterdir() if p.name.startswith('usage-cache')]
    assert caches, 'usage keeps a cache in the data folder'
    print('PASS fixture totals: dedupe, cache writes by TTL, subagents, Codex deltas, unpriced models, Cursor turns', flush=True)

    utc = usage(home, data, 'all', tz='UTC')
    check(utc, expected)
    check_projects(utc, expected, root)
    text = run(home, data, 'usage', '--period', 'all', tz='UTC').stdout
    projects_block = text[text.index('Projects'):]
    assert 'alpha' in projects_block and 'gamma' in projects_block and 'Unknown' in projects_block, text
    assert '19m active' in projects_block, projects_block
    # Days follow local midnight. At UTC+14, alpha's 09:59-10:10 UTC run crosses midnight: 60 seconds
    # on Oct 30, the rest on Oct 31, where every token is (its first bucket starts at 00:00 local).
    # (Windows ignores TZ and uses the system time zone, so this runs on macOS and Linux.)
    if os.name != 'nt':
        kiritimati = usage(home, data, 'all', tz='Pacific/Kiritimati')
        alpha = next(p for p in kiritimati['projects'] if p['name'] == 'alpha')
        assert [(d['date'], d['activeSeconds'], d['totalTokens']) for d in alpha['days']] == [
            ('2026-10-30', 60, 0), ('2026-10-31', 1085, 6745)], alpha['days']
    print('PASS projects: Git roots, worktrees, active time joined across files and agents, sessions, models, days', flush=True)

    out = temporary / 'report.html'
    reply = json.loads(run(home, data, 'report', '--no-open', '--output', str(out), '--period', 'month', '--json').stdout)
    assert reply == {'opened': False, 'path': str(out)} or (reply['opened'] is False and same_path(reply['path'], str(out))), reply
    page = out.read_text()
    assert page.count('/*BARISTA_REPORT_DATA*/') == 0 and '<html' in page.lower(), 'the template was filled in'
    start = page.index('let DATA = ') + len('let DATA = ')
    embedded = json.JSONDecoder().raw_decode(page[start:])[0]
    assert embedded['initialPeriod'] == 'month', embedded.get('initialPeriod')
    assert [p['id'] for p in embedded['periods']] == ['today', 'week', 'month', 'all'], [p['id'] for p in embedded['periods']]
    assert not re.search(r'(src|href|action)\s*=\s*["\']?\s*(https?:)?//', page, re.I) and '@import' not in page \
        and 'url(' not in page and 'fetch(' not in page, 'the report loads nothing from the network'
    assert 'Wrote ' in run(home, data, 'report', '--no-open', '--output', str(out)).stdout
    print('PASS report: self-contained HTML with every period, initial period, JSON reply', flush=True)

    limits = report(home, data)['limits']
    claude, codex, cursor = limits
    assert not claude['available'] and claude['windows'] == [] and claude['observedAt'] is None, claude
    assert not cursor['available'] and cursor['windows'] == [], cursor
    assert codex['available'] and codex['source'] == 'Codex' and codex['plan'] == 'prolite', codex
    assert codex['credits'] == {'balance': '75000.0000000000', 'unlimited': False}, codex
    assert codex['observedAt'].startswith('2026-10-30T12:08:00'), codex
    windows = {w['id']: w for w in codex['windows']}
    assert windows['primary']['label'] == 'Weekly' and windows['primary']['usedPercent'] == 6 and not windows['primary']['reset'], windows
    assert windows['primary']['windowMinutes'] == 10080 and windows['primary']['resetsAt'].startswith('2100-01-01'), windows
    assert windows['secondary']['label'] == '5-hour' and windows['secondary']['usedPercent'] == 0 and windows['secondary']['reset'], windows
    text = run(home, data, 'usage', '--period', 'all').stdout
    assert 'Limits' in text and 'ChatGPT plan' in text and 'Weekly 6%' in text, text

    status = {'model': {'id': 'claude-opus-5-5', 'display_name': 'Opus'}, 'session_id': 's',
              'rate_limits': {'five_hour': {'used_percentage': 23.5, 'resets_at': 4102444800},
                              'seven_day': {'used_percentage': 41.2, 'resets_at': 946684800}}}
    line = run(home, data, 'statusline', stdin=json.dumps(status)).stdout.strip()
    assert line == 'Opus · 5h 24%', line
    claude = report(home, data)['limits'][0]
    assert claude['available'] and claude['source'] == 'Claude Code' and claude['name'] == 'Claude plan', claude
    windows = {w['id']: w for w in claude['windows']}
    assert windows['five_hour']['usedPercent'] == 23.5 and windows['five_hour']['label'] == '5-hour' and not windows['five_hour']['reset'], windows
    assert windows['seven_day']['usedPercent'] == 0 and windows['seven_day']['reset'] and windows['seven_day']['label'] == 'Weekly', windows
    # A reply without limits keeps the last ones; nothing on stdin still prints something harmless.
    assert run(home, data, 'statusline', stdin=json.dumps({'model': {'display_name': 'Opus'}})).stdout.strip() == 'Opus'
    run(home, data, 'statusline', stdin='not json')
    assert report(home, data)['limits'][0]['source'] == 'Claude Code'
    # The Claude desktop app's newer snapshot wins (no reset times).
    app = data / 'claude-app'
    app.mkdir()
    (app / 'plan-usage-history.json').write_text(json.dumps({'version': 2, 'samples': [
        {'t': 1000, 'org': 'o', 'u': {'fh': 99, 'sd': 99}},
        {'t': 4102444800000, 'org': 'o', 'u': {'fh': 5, 'sd': 27}}]}))
    claude = report(home, data)['limits'][0]
    windows = {w['id']: w for w in claude['windows']}
    assert claude['source'] == 'Claude app' and windows['five_hour']['usedPercent'] == 5 and windows['seven_day']['usedPercent'] == 27, claude
    assert windows['five_hour']['resetsAt'] is None and not windows['five_hour']['reset'], windows
    (app / 'plan-usage-history.json').write_text('{broken')
    assert report(home, data)['limits'][0]['source'] == 'Claude Code', 'an unreadable app file is ignored'
    (app / 'plan-usage-history.json').write_text(json.dumps({'samples': [{'t': int((time.time() - 7200) * 1000), 'u': {'fh': 99, 'sd': 99}}]}))
    stale_home, stale_data = temporary / 'stale-home', temporary / 'stale-data'
    stale_home.mkdir(); (stale_data / 'claude-app').mkdir(parents=True)
    shutil.copy(app / 'plan-usage-history.json', stale_data / 'claude-app' / 'plan-usage-history.json')
    assert not report(stale_home, stale_data)['limits'][0]['available'], 'a Claude app snapshot older than an hour is ignored'

    # Your Claude account, through a local stand-in for Anthropic's usage endpoint.
    served = {'count': 0, 'status': 200, 'auth': None, 'beta': None}
    body = (FIXTURE / 'claude-account-usage.json').read_bytes()
    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path == '/elsewhere':
                # Where the 302 below points. Answers with good limits, so a client that followed the
                # redirect would succeed, and be caught.
                served['followed'] = served.get('followed', 0) + 1
                self.send_response(200); self.send_header('Content-Type', 'application/json'); self.end_headers()
                self.wfile.write(body)
                return
            served['count'] += 1
            served['auth'] = self.headers.get('Authorization'); served['beta'] = self.headers.get('anthropic-beta')
            self.send_response(served['status']); self.send_header('Content-Type', 'application/json')
            if served['status'] == 302:
                self.send_header('Location', '/elsewhere')
            self.end_headers()
            self.wfile.write(body if served['status'] == 200 else b'{"error":"nope"}')
        def log_message(self, *args): pass
    server = http.server.HTTPServer(('127.0.0.1', 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    credentials = temporary / 'credentials.json'
    def write_credentials(expires_ms):
        credentials.write_text(json.dumps({'claudeAiOauth': {'accessToken': 'test-token-123', 'refreshToken': 'refresh-xyz', 'expiresAt': expires_ms}}))
    account_env = {'BARISTA_CLAUDE_CREDENTIALS': str(credentials), 'BARISTA_CLAUDE_USAGE_URL': f'http://127.0.0.1:{server.server_port}/api/oauth/usage'}
    write_credentials(int((time.time() + 3600) * 1000))
    # A user's ~/.curlrc must never apply (it could trace headers or add URLs).
    (home / '.curlrc').write_text(f'trace-ascii = "{temporary / "curl-trace.txt"}"\n')
    result = json.loads(run(home, data, 'limits', 'refresh', '--json', extra=account_env).stdout)
    assert result['ok'] is True and result['message'] == 'Claude account limits updated.', result
    assert not (temporary / 'curl-trace.txt').exists(), '~/.curlrc was used'
    (home / '.curlrc').unlink()
    assert served['count'] == 1 and served['auth'] == 'Bearer test-token-123' and served['beta'] == 'oauth-2025-04-20', served
    saved = (data / 'claude-account-limits.json').read_text()
    assert 'test-token-123' not in saved and 'refresh-xyz' not in saved, 'the token is never written'
    claude = report(home, data)['limits'][0]
    windows = {w['id']: w for w in claude['windows']}
    assert claude['source'] == 'Claude account' and claude['available'], claude
    # Window ids stay the same as 0.0.8's status-line ids, so scripts keep working.
    assert windows['five_hour']['label'] == 'Session' and windows['five_hour']['usedPercent'] == 24 and windows['five_hour']['windowMinutes'] == 300, windows
    assert windows['seven_day']['label'] == 'Weekly' and windows['seven_day']['usedPercent'] == 8 and windows['seven_day']['resetsAt'].startswith('2100-01-08'), windows
    assert windows['seven_day_fable']['label'] == 'Weekly · Fable' and windows['seven_day_fable']['usedPercent'] == 0, windows
    text = run(home, data, 'usage', '--period', 'today').stdout
    assert 'Extra usage 5% ($12.50 of $250.00)' in text, text
    assert windows['extra_usage']['usedUSD'] == 12.5 and windows['extra_usage']['limitUSD'] == 250 and windows['extra_usage']['usedPercent'] == 5, windows
    # An expired login is never used (or refreshed); a failed request keeps the last good result.
    write_credentials(int((time.time() - 60) * 1000))
    expired = json.loads(run(home, data, 'limits', 'refresh', '--json', extra=account_env, ok=False).stdout)
    assert expired['ok'] is False and 'expired' in expired['message'], expired
    assert served['count'] == 1, 'an expired token must not be sent'
    write_credentials(int((time.time() + 3600) * 1000)); served['status'] = 500
    run(home, data, 'limits', 'refresh', extra=account_env, ok=False)
    assert served['count'] == 2 and report(home, data)['limits'][0]['source'] == 'Claude account', 'errors keep the previous result'
    served['status'] = 302
    run(home, data, 'limits', 'refresh', extra=account_env, ok=False)
    assert served['count'] == 3 and 'followed' not in served, f'a redirect is never followed: {served}'
    assert report(home, data)['limits'][0]['source'] == 'Claude account', 'redirects are refused and keep the previous result'
    # The endpoint override only applies together with a test credentials file.
    no_creds = {'BARISTA_CLAUDE_USAGE_URL': account_env['BARISTA_CLAUDE_USAGE_URL'], 'BARISTA_CLAUDE_CREDENTIALS': ''}
    before = served['count']
    if sys.platform != 'darwin':  # On macOS this would read the real Keychain.
        override_home = temporary / 'override-home'
        (override_home / '.claude').mkdir(parents=True)
        (override_home / '.claude' / '.credentials.json').write_text(credentials.read_text())
        run(override_home, data, 'limits', 'refresh', extra=no_creds, ok=False)
        assert served['count'] == before, 'without a test credentials file the override is ignored'
    # Account results older than an hour are not shown.
    saved_path = data / 'claude-account-limits.json'
    saved_json = json.loads(saved_path.read_text())
    saved_json['observedAt'] = (datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(hours=2)).strftime('%Y-%m-%dT%H:%M:%SZ')
    saved_path.write_text(json.dumps(saved_json))
    assert report(home, data)['limits'][0]['source'] != 'Claude account', 'stale account results are ignored'
    assert json.loads(credentials.read_text())['claudeAiOauth']['refreshToken'] == 'refresh-xyz', 'the login is never modified'
    server.shutdown()
    print('PASS limits: Codex windows, plan and credits; Claude status line, reset windows, Claude app fallback (stale ignored); Claude account fetch, never sends an expired token, keeps results on errors', flush=True)

    # Pace: where even use would put each window, the projection at its reset, and when it runs out.
    def claude_windows(used5, left5, used7, left7):
        start = time.time()
        status = {'model': {'display_name': 'Opus'}, 'rate_limits': {
            'five_hour': {'used_percentage': used5, 'resets_at': int(start + left5)},
            'seven_day': {'used_percentage': used7, 'resets_at': int(start + left7)}}}
        line = run(home, data, 'statusline', stdin=json.dumps(status)).stdout.strip()
        reply = report(home, data)
        return start, line, reply['limits'], {w['id']: w for w in reply['limits'][0]['windows']}
    def near(value, want, slack):
        return value is not None and abs(value - want) <= slack
    def seconds(iso):
        return datetime.datetime.fromisoformat(iso.replace('Z', '+00:00')).timestamp()
    pace_keys = ('elapsedPercent', 'projectedPercent', 'limitAt', 'pace')
    # Half of the 5-hour window gone with 60% used: on course for 120%, out in 100 minutes.
    start, line, limits, windows = claude_windows(60, 9000, 10, 6 * 86400)
    assert line == 'Opus · 5h 60% · 7d 10%', line
    assert limits[0]['source'] == 'Claude Code', limits[0]
    five, seven = windows['five_hour'], windows['seven_day']
    assert near(five['elapsedPercent'], 50, 0.2) and near(five['projectedPercent'], 120, 0.6) and five['pace'] == 'over', five
    assert five['limitAt'] and near(seconds(five['limitAt']) - start, 6000, 20), five
    # A day into the week with 10% used: on course for 70%, so it lasts.
    assert near(seven['elapsedPercent'], 14.3, 0.2) and near(seven['projectedPercent'], 70, 0.6), seven
    assert seven['pace'] == 'ok' and seven['limitAt'] is None, seven
    text = run(home, data, 'usage').stdout
    assert 'runs out' in text and 'on pace for 70%' in text, text
    # Windows that reset far in the future (more than a window away) have no pace; the fields stay, as null.
    for window in limits[1]['windows']:
        assert all(key in window and window[key] is None for key in pace_keys), window
    # Too early in a window (under 8% gone), or under 1% used, to trust the rate.
    start, line, limits, windows = claude_windows(5, 17700, 0.5, 6 * 86400)
    five, seven = windows['five_hour'], windows['seven_day']
    assert near(five['elapsedPercent'], 1.7, 0.2) and all(five[key] is None for key in pace_keys[1:]), five
    assert near(seven['elapsedPercent'], 14.3, 0.2) and all(seven[key] is None for key in pace_keys[1:]), seven
    # Used up: over, with nothing left to run out.
    start, line, limits, windows = claude_windows(100, 9000, 10, 6 * 86400)
    five = windows['five_hour']
    assert five['pace'] == 'over' and near(five['projectedPercent'], 200, 1) and five['limitAt'] is None, five
    print('PASS pace: elapsed, projected, and run-out time; none too early in a window or far from a reset', flush=True)

    # The Claude plan's name, from the profile Claude Code keeps (no network).
    profile = home / '.claude' / '.claude.json'
    def plan_for(account):
        profile.write_text(account if isinstance(account, str) else json.dumps({'numStartups': 3, 'oauthAccount': account}))
        return report(home, data)['limits'][0]['plan']
    assert report(home, data)['limits'][0]['plan'] is None, 'no profile, no plan name'
    assert plan_for({'organizationType': 'claude_max', 'userRateLimitTier': 'default_claude_max_5x', 'emailAddress': 'a@b.c'}) == 'Max 5x'
    assert plan_for({'organizationType': 'claude_max', 'organizationRateLimitTier': 'default_claude_max_20x'}) == 'Max 20x'
    assert plan_for({'organizationType': 'claude_max', 'userRateLimitTier': 'default_claude_ai'}) == 'Max'
    assert plan_for({'organizationType': 'claude_team', 'userRateLimitTier': 'default_claude_max_5x'}) == 'Team'
    assert plan_for({'organizationType': 'claude_enterprise'}) == 'Enterprise'
    assert plan_for({'organizationType': 'claude_pro'}) == 'Pro'
    assert plan_for({'organizationType': 'something', 'organizationRateLimitTier': 'default_raven'}) is None
    assert plan_for('{broken') is None and plan_for({}) is None
    profile.unlink()
    print('PASS Claude plan name: from the local profile, by organization type and rate-limit tier', flush=True)

    # Your ChatGPT account, through a local stand-in for ChatGPT's usage endpoint.
    import base64
    def jwt(exp):
        part = lambda value: base64.urlsafe_b64encode(json.dumps(value).encode()).decode().rstrip('=')
        return f"{part({'alg': 'none'})}.{part({'exp': exp, 'sub': 'user-123'})}.signature"
    chatgpt = {'count': 0, 'status': 200, 'auth': None, 'account': None, 'agent': None,
               'body': (FIXTURE / 'codex-account-usage.json').read_bytes()}
    class ChatGPT(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path == '/elsewhere':
                # Where the 302 below points: good limits, so following the redirect would succeed.
                chatgpt['followed'] = chatgpt.get('followed', 0) + 1
                self.send_response(200); self.send_header('Content-Type', 'application/json'); self.end_headers()
                self.wfile.write(json.dumps({'plan_type': 'followed', 'rate_limit': {'primary_window': {'used_percent': 1, 'limit_window_seconds': 18000, 'reset_at': 4102444800}}}).encode())
                return
            chatgpt['count'] += 1
            chatgpt['auth'] = self.headers.get('Authorization'); chatgpt['account'] = self.headers.get('ChatGPT-Account-Id')
            chatgpt['agent'] = self.headers.get('User-Agent')
            self.send_response(chatgpt['status']); self.send_header('Content-Type', 'application/json')
            if chatgpt['status'] == 302:
                self.send_header('Location', '/elsewhere')
            self.end_headers()
            self.wfile.write(chatgpt['body'] if chatgpt['status'] == 200 else b'{"error":"nope"}')
        def log_message(self, *args): pass
    chatgpt_server = http.server.HTTPServer(('127.0.0.1', 0), ChatGPT)
    threading.Thread(target=chatgpt_server.serve_forever, daemon=True).start()
    codex_auth = temporary / 'codex-auth.json'
    def write_codex_auth(token, account='acct-1'):
        tokens = {'id_token': 'id-secret', 'access_token': token, 'refresh_token': 'refresh-abc'}
        if account:
            tokens['account_id'] = account
        codex_auth.write_text(json.dumps({'auth_mode': 'chatgpt', 'OPENAI_API_KEY': None, 'tokens': tokens}))
    codex_env = {'BARISTA_CODEX_AUTH': str(codex_auth), 'BARISTA_CODEX_USAGE_URL': f'http://127.0.0.1:{chatgpt_server.server_port}/backend-api/wham/usage'}
    def refresh_codex(ok=True):
        return json.loads(run(home, data, 'limits', 'refresh', '--agent', 'codex', '--json', extra=codex_env, ok=ok).stdout)
    token = jwt(int(time.time()) + 3600)
    write_codex_auth(token)
    result = refresh_codex()
    assert result == {'ok': True, 'message': 'ChatGPT account limits updated.'}, result
    assert chatgpt['count'] == 1 and chatgpt['auth'] == f'Bearer {token}' and chatgpt['account'] == 'acct-1', chatgpt
    assert chatgpt['agent'].startswith('Barista/'), chatgpt['agent']
    saved_path = data / 'codex-account-limits.json'
    saved = saved_path.read_text()
    for secret in (token, 'refresh-abc', 'id-secret', 'someone@example.com', 'user-123', 'acct-1'):
        assert secret not in saved, f'{secret} must never be written'
    assert sorted(json.loads(saved)['usage']) == ['credits', 'plan_type', 'rate_limit'], saved
    codex = report(home, data)['limits'][1]
    windows = {w['id']: w for w in codex['windows']}
    assert codex['source'] == 'ChatGPT account' and codex['available'] and codex['plan'] == 'plus', codex
    assert codex['credits'] == {'balance': '1200.0000000000', 'unlimited': False}, codex
    assert windows['primary']['label'] == '5-hour' and windows['primary']['usedPercent'] == 20 and windows['primary']['windowMinutes'] == 300, windows
    assert windows['primary']['resetsAt'] == '2100-01-01T00:00:00Z' and not windows['primary']['reset'], windows
    assert windows['secondary']['label'] == 'Weekly' and windows['secondary']['usedPercent'] == 41.5 and windows['secondary']['windowMinutes'] == 10080, windows
    assert windows['secondary']['resetsAt'].startswith('2100-01-02'), windows
    text = run(home, data, 'usage').stdout
    assert '5-hour 20%' in text and 'Weekly 42%' in text, text
    # A plan with one window and no credits, as ChatGPT reports it.
    chatgpt['body'] = json.dumps({'plan_type': 'prolite', 'email': 'someone@example.com', 'rate_limit': {
        'primary_window': {'used_percent': 7, 'limit_window_seconds': 604800, 'reset_at': 4102444800}, 'secondary_window': None}}).encode()
    write_codex_auth('opaque-token', account=None)
    assert refresh_codex()['ok'] and chatgpt['auth'] == 'Bearer opaque-token' and chatgpt['account'] is None, 'a token with no readable expiry is sent as it is'
    codex = report(home, data)['limits'][1]
    assert [w['id'] for w in codex['windows']] == ['primary'] and codex['windows'][0]['label'] == 'Weekly' and codex['credits'] is None, codex
    assert codex['plan'] == 'prolite' and codex['source'] == 'ChatGPT account', codex
    # An expired login is never sent (or refreshed); failures keep the last good result.
    before = chatgpt['count']
    write_codex_auth(jwt(int(time.time()) - 60))
    expired = refresh_codex(ok=False)
    assert expired['ok'] is False and 'expired' in expired['message'] and chatgpt['count'] == before, (expired, chatgpt)
    write_codex_auth(token); chatgpt['status'] = 500
    assert refresh_codex(ok=False)['message'] == 'ChatGPT replied 500.'
    assert chatgpt['count'] == before + 1 and report(home, data)['limits'][1]['source'] == 'ChatGPT account', 'errors keep the previous result'
    chatgpt['status'] = 302
    assert not refresh_codex(ok=False)['ok']
    assert chatgpt['count'] == before + 2 and 'followed' not in chatgpt, f'a redirect is never followed: {chatgpt}'
    assert report(home, data)['limits'][1]['plan'] == 'prolite', 'redirects are refused and keep the previous result'
    # No ChatGPT sign-in (an API key only), and the override without a test login file.
    codex_auth.write_text(json.dumps({'auth_mode': 'apikey', 'OPENAI_API_KEY': 'sk-test', 'tokens': None}))
    missing = refresh_codex(ok=False)
    assert missing['message'] == 'No Codex login found.', missing
    before = chatgpt['count']
    run(home, data, 'limits', 'refresh', '--agent', 'codex', extra={'BARISTA_CODEX_USAGE_URL': codex_env['BARISTA_CODEX_USAGE_URL'], 'BARISTA_CODEX_AUTH': ''}, ok=False)
    assert chatgpt['count'] == before, 'without a test login file the override is ignored (the fixture home has no Codex login)'
    # Plain `limits refresh` still means Claude.
    write_credentials(int((time.time() - 60) * 1000))
    for agent in ([], ['--agent', 'claude']):
        assert 'Claude Code' in json.loads(run(home, data, 'limits', 'refresh', *agent, '--json', extra=account_env, ok=False).stdout)['message']
    run(home, data, 'limits', 'refresh', '--agent', 'cursor', ok=False)
    # Account results older than an hour are not shown: Codex's logs take over again.
    saved_json = json.loads(saved_path.read_text())
    saved_json['observedAt'] = (datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(hours=2)).strftime('%Y-%m-%dT%H:%M:%SZ')
    saved_path.write_text(json.dumps(saved_json))
    assert report(home, data)['limits'][1]['source'] == 'Codex', 'stale account results are ignored'
    chatgpt_server.shutdown()
    print('PASS ChatGPT account: live limits, plan and credits; only limits are saved; never sends an expired token; keeps results on errors', flush=True)

    # The partial last line is finished later: it counts once, on the next run.
    claude_log = home / '.claude' / 'projects' / '-tmp-proj' / 'claude-s2.jsonl'
    with claude_log.open('a') as handle:
        handle.write('\n')
    grown = usage(home, data)
    claude = grown['agents'][0]
    assert claude['requests'] == 5 and claude['inputTokens'] == 36 + 999999, claude
    # Appending to a log is picked up incrementally.
    codex_log = next((home / '.codex' / 'sessions').rglob('*.jsonl'))
    with codex_log.open('a') as handle:
        handle.write(json.dumps({'timestamp': '2026-10-30T12:09:00Z', 'type': 'event_msg', 'payload': {'type': 'token_count', 'info': {
            'total_token_usage': {'input_tokens': 3600, 'cached_input_tokens': 2400, 'output_tokens': 170, 'reasoning_output_tokens': 30, 'total_tokens': 3770}}}}) + '\n')
    codex = usage(home, data)['agents'][1]
    assert codex['requests'] == 4 and codex['inputTokens'] == 1200 and codex['outputTokens'] == 170, codex
    # Archiving a Codex conversation moves its log; it must not count twice.
    archived = home / '.codex' / 'archived_sessions' / codex_log.name
    archived.parent.mkdir(parents=True)
    codex_log.rename(archived)
    moved = usage(home, data)['agents'][1]
    assert moved['requests'] == 4 and moved['inputTokens'] == 1200 and moved['totalTokens'] == codex['totalTokens'], moved
    codex_log = archived
    # An emptied log takes back what it contributed.
    subagent = home / '.claude' / 'projects' / '-tmp-proj' / 'claude-s1' / 'subagents' / 'agent-1.jsonl'
    subagent.write_text('')
    claude = usage(home, data)['agents'][0]
    assert claude['requests'] == 4 and claude['outputTokens'] == 351 - 200 + 999999, claude
    # A rewritten (shorter) log is re-read from the start, replacing what it contributed.
    # Here it keeps only its first line, a prompt with no tokens.
    claude_log.write_text(claude_log.read_text().splitlines()[0] + '\n')
    claude = usage(home, data)['agents'][0]
    assert claude['requests'] == 2 and claude['unpricedTokens'] == 0 and claude['sessions'] == 1, claude
    # Deleted logs keep their history.
    shutil.rmtree(home / '.codex')
    assert usage(home, data)['agents'][1]['requests'] == 4
    print('PASS incremental reads: partial lines, appends, archived Codex logs, emptied and rewritten logs, deleted logs keep history', flush=True)

    # Period edges use real request times: 00:15 today in a half-hour time zone counts as today.
    zone_home, zone_data = temporary / 'zone-home', temporary / 'zone-data'
    project = zone_home / '.claude' / 'projects' / 'p'
    project.mkdir(parents=True)
    kolkata = datetime.timezone(datetime.timedelta(hours=5, minutes=30))
    now = datetime.datetime.now(kolkata)
    early = now.replace(hour=0, minute=15, second=0, microsecond=0)
    future = now + datetime.timedelta(days=2)
    def entry(when, message):
        stamp = when.astimezone(datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%S.000Z')
        return json.dumps({'type': 'assistant', 'timestamp': stamp, 'sessionId': 'z', 'requestId': message,
                           'message': {'id': message, 'model': 'claude-opus-5-5', 'usage': {'input_tokens': 100, 'output_tokens': 0}}}) + '\n'
    # Windows ignores TZ (it uses the system time zone), so there only "nothing after now" is checked;
    # otherwise the result would depend on whether it's the same date in Kolkata and on the runner.
    counts_early = os.name != 'nt' and early <= now
    lines = (entry(early, 'm1') if counts_early else '') + entry(future, 'm2')
    (project / 's.jsonl').write_text(lines)
    today = usage(zone_home, zone_data, 'today', tz='Asia/Kolkata')['agents'][0]
    assert today['inputTokens'] == (100 if counts_early else 0), ('a request at 00:15 local time is today; future ones are not', today)
    print('PASS periods: exact boundaries in half-hour time zones, nothing after now', flush=True)

    # Periods: requests from the fixture's past dates are outside today; a fresh one is inside.
    empty = temporary / 'empty-home'
    empty.mkdir()
    fresh = temporary / 'fresh-data'
    run(empty, fresh, 'hook', '--agent', 'cursor', '--socket', str(temporary / 'none' / 'control.sock'), '--no-launch',
        stdin=json.dumps({'hook_event_name': 'beforeSubmitPrompt', 'conversation_id': 'conv-1', 'generation_id': 'gen-1',
                          'model': 'gpt-6.1-sol', 'workspace_roots': ['/tmp/proj']}))
    stop = run(empty, fresh, 'hook', '--agent', 'cursor', '--socket', str(temporary / 'none' / 'control.sock'), '--no-launch',
               stdin=json.dumps({'hook_event_name': 'stop', 'conversation_id': 'conv-1', 'generation_id': 'gen-1', 'status': 'completed'}))
    assert json.loads(stop.stdout) == {}, stop.stdout
    records = [json.loads(line) for line in (fresh / 'cursor-usage.jsonl').read_text().splitlines()]
    assert [r['event'] for r in records] == ['prompt', 'stop'] and records[0]['model'] == 'gpt-6.1-sol', records
    assert records[0]['project'] == '/tmp/proj' and 'project' not in records[1], records
    assert records[1]['status'] == 'completed' and records[1]['generation'] == 'gen-1'
    today = usage(empty, fresh, 'today')
    assert today['agents'][2]['turns'] == 1 and today['agents'][2]['sessions'] == 1, today
    assert usage(empty, fresh, 'week')['agents'][2]['turns'] == 1
    assert usage(empty, fresh, 'month')['agents'][2]['turns'] == 1
    run(empty, fresh, 'hook', '--agent', 'cursor', '--socket', str(temporary / 'none' / 'control.sock'), '--no-launch',
        stdin=json.dumps({'hook_event_name': 'stop', 'conversation_id': 'conv-9', 'generation_id': 'gen-9', 'model': 'gpt-5.5[1m]', 'status': 'completed'}))
    models = [m['model'] for m in usage(empty, fresh, 'today')['agents'][2]['models']]
    assert 'gpt-5.5' in models and 'gpt-5.5[1m]' not in models, models
    prompt = run(empty, fresh, 'hook', '--agent', 'cursor', '--socket', str(temporary / 'none' / 'control.sock'), '--no-launch',
                 stdin=json.dumps({'hook_event_name': 'beforeSubmitPrompt', 'conversation_id': 'conv-2', 'generation_id': 'gen-2'}))
    assert json.loads(prompt.stdout) == {'continue': True}, prompt.stdout
    assert json.loads(run(empty, fresh, 'hook', '--agent', 'cursor', '--no-launch', stdin='not json').stdout) == {}
    default = json.loads(run(empty, fresh, 'usage', '--json').stdout)
    assert [p['id'] for p in default['periods']] == ['today', 'week', 'month'], default
    text = run(empty, fresh, 'usage').stdout
    assert 'Cursor' in text and 'estimate' in text.lower(), text
    # Project names are data: the report page must never let one close its script.
    run(empty, fresh, 'hook', '--agent', 'cursor', '--socket', str(temporary / 'none' / 'control.sock'), '--no-launch',
        stdin=json.dumps({'hook_event_name': 'beforeSubmitPrompt', 'conversation_id': 'conv-x', 'generation_id': 'gen-x',
                          'workspace_roots': ['/nowhere/x</script><img src=x onerror=alert(1)>&b']}))
    hostile = temporary / 'hostile.html'
    run(empty, fresh, 'report', '--no-open', '--output', str(hostile))
    page = hostile.read_text()
    assert '</script><img' not in page and '<img src=x' not in page, 'project names are escaped in the report'
    names = [p['name'] for p in usage(empty, fresh, 'today')['projects']]
    assert 'script><img src=x onerror=alert(1)>&b' in names, names
    print('PASS Cursor hooks: prompt/stop records with project, JSON replies, periods; default and text output; report escaping', flush=True)

    settings = temporary / 'cursor' / 'hooks.json'
    settings.parent.mkdir()
    settings.write_text(json.dumps({'version': 1, 'hooks': {'stop': [{'command': './hooks/audit.sh'}]}}))
    def hooks_cmd(*args):
        return run(empty, fresh, 'hooks', *args, '--agent', 'cursor', '--settings', str(settings)).stdout
    assert hooks_cmd('status').startswith('Not connected')
    hooks_cmd('install'); hooks_cmd('install')
    config = json.loads(settings.read_text())
    assert config['version'] == 1
    for event in ['beforeSubmitPrompt', 'postToolUse', 'afterAgentResponse', 'stop', 'sessionEnd']:
        ours = [h for h in config['hooks'][event] if h['command'].endswith(' hook --agent cursor')]
        assert len(ours) == 1 and ours[0]['timeout'] == 10, (event, config)
    assert {'command': './hooks/audit.sh'} in config['hooks']['stop']
    assert hooks_cmd('status').startswith('Connected')
    hooks_cmd('uninstall')
    assert json.loads(settings.read_text()) == {'version': 1, 'hooks': {'stop': [{'command': './hooks/audit.sh'}]}}
    settings.unlink(); hooks_cmd('install'); hooks_cmd('uninstall')
    assert not settings.exists(), 'a hooks.json that held only Barista hooks is removed'
    claude_settings = temporary / 'claude-settings.json'
    original = {'theme': 'dark', 'statusLine': {'type': 'command', 'command': 'echo hi', 'padding': 1}}
    claude_settings.write_text(json.dumps(original))
    def statusline_cmd(*args):
        return run(empty, fresh, 'statusline', *args, '--settings', str(claude_settings)).stdout
    statusline_cmd('install'); statusline_cmd('install')
    installed = json.loads(claude_settings.read_text())
    assert installed['theme'] == 'dark' and ' statusline --chain ' in installed['statusLine']['command'], installed
    assert installed['statusLine']['type'] == 'command'
    assert statusline_cmd('status').startswith('Connected (in front of your own status line)')
    assert installed_statusline(empty, fresh, claude_settings, json.dumps(status)) == 'hi', 'your own status line keeps working'
    # A second settings file (another profile) must not disturb the first one's saved status line.
    other = temporary / 'other-settings.json'
    other.write_text(json.dumps({'theme': 'light'}))
    run(empty, fresh, 'statusline', 'install', '--settings', str(other))
    assert installed_statusline(empty, fresh, other, json.dumps(status)) == 'Opus · 5h 24%'
    assert installed_statusline(empty, fresh, claude_settings, json.dumps(status)) == 'hi', 'installing elsewhere keeps the first backup'
    run(empty, fresh, 'statusline', 'uninstall', '--settings', str(other))
    assert json.loads(other.read_text()) == {'theme': 'light'}
    statusline_cmd('uninstall')
    assert json.loads(claude_settings.read_text()) == original, 'uninstall restores your status line'
    # If Barista's saved copy disappears, uninstalling refuses rather than dropping your status line.
    statusline_cmd('install')
    for backup in glob.glob(str(fresh / 'statusline-chain*')):
        os.remove(backup)
    run(empty, fresh, 'statusline', 'uninstall', '--settings', str(claude_settings), ok=False)
    assert json.loads(claude_settings.read_text())['statusLine']['command'].split()[-1] != 'echo', 'settings left unchanged'
    claude_settings.write_text(json.dumps(original))
    claude_settings.write_text(json.dumps({'theme': 'dark'}))
    statusline_cmd('install')
    assert statusline_cmd('status').startswith('Connected')
    assert installed_statusline(empty, fresh, claude_settings, json.dumps(status)) == 'Opus · 5h 24%'
    statusline_cmd('uninstall')
    assert json.loads(claude_settings.read_text()) == {'theme': 'dark'} and statusline_cmd('status').startswith('Not connected')
    print('PASS status line installer: chains your own status line per settings file, restores it, refuses without its backup, clean uninstall', flush=True)

    # Hooks a 0.0.7 install wrote (no Codex labels) still count as connected.
    codex_file = temporary / 'codex-hooks.json'
    run(empty, fresh, 'hooks', 'install', '--agent', 'codex', '--settings', str(codex_file))
    legacy = json.loads(codex_file.read_text())
    legacy.pop('description', None)
    for groups in legacy['hooks'].values():
        for group in groups:
            for hook in group['hooks']:
                hook.pop('statusMessage', None)
    codex_file.write_text(json.dumps(legacy))
    assert run(empty, fresh, 'hooks', 'status', '--agent', 'codex', '--settings', str(codex_file)).stdout.startswith('Connected'), 'unlabeled 0.0.7 hooks are not "needs repair"'
    print('PASS compatibility: unlabeled 0.0.7 Codex hooks still report connected', flush=True)

    if os.name == 'nt':
        print('PASS Cursor hook installer: preserves other hooks, idempotent, clean uninstall (setup check skipped on Windows)', flush=True)
        print('All usage checks passed.', flush=True)
        raise SystemExit(0)
    (empty / '.cursor').mkdir()
    setup = run(empty, fresh, 'setup', '--socket', str(temporary / 'none' / 'control.sock'), '--no-launch', ok=False)
    assert 'Connected Cursor' in setup.stdout, setup
    assert (empty / '.cursor' / 'hooks.json').exists()
    print('PASS Cursor hook installer: preserves other hooks, idempotent, clean uninstall, setup finds Cursor', flush=True)
    print('All usage checks passed.', flush=True)
