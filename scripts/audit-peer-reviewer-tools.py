#!/usr/bin/env python3
"""Installed reviewer audits; production policy is supplied by real_peer_reviewer.rs.

Run: cargo test --locked -p cide-core --test real_peer_reviewer -- --ignored --nocapture
Only local fake model endpoints are used. Web fetching reads https://example.com;
OpenCode search uses its native keyless provider pool against the public internet.
"""
import json
import os
from pathlib import Path
import runpy
import select
import shlex
import signal
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def stop(process):
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=3)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait()


def environment(policy, root):
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(('ANTHROPIC_', 'CLAUDE_', 'CODEX_', 'CIDE_', 'OPENCODE_'))}
    env.update(dict(policy['env']))
    for key, leaf in [('CLAUDE_CONFIG_DIR', 'claude'), ('CODEX_HOME', 'codex'),
                      ('XDG_CONFIG_HOME', 'config'), ('XDG_DATA_HOME', 'data'),
                      ('XDG_CACHE_HOME', 'cache'), ('XDG_STATE_HOME', 'state')]:
        path = root / leaf
        path.mkdir(mode=0o700)
        env[key] = str(path)
    return env


def analysis_command(policy, source):
    code = f'import pathlib; print("ANALYSIS_OK", len(pathlib.Path({str(source)!r}).read_text().splitlines()))'
    launcher = ('import pathlib,subprocess,os; '
                'p=pathlib.Path(os.environ["CIDE_PEER_SCRATCH"])/"analysis.py"; '
                f'p.write_text({code!r}); subprocess.run(["python3",str(p)],check=True)')
    return 'python3 -c ' + shlex.quote(launcher)


def claude(policy):
    with tempfile.TemporaryDirectory(prefix='cide-peer-claude-') as tmp:
        root = Path(tmp)
        env = environment(policy, root)
        source = root / 'source.txt'
        source.write_text('source data\n' * 4096)
        captured = []
        failures = []

        class Provider(BaseHTTPRequestHandler):
            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                if self.path.endswith('/count_tokens'):
                    payload = json.dumps({'input_tokens': 10}).encode()
                    self.send_response(200)
                    self.send_header('Content-Type', 'application/json')
                    self.end_headers()
                    self.wfile.write(payload)
                    return
                captured.append(body)
                names = {t.get('name') for t in body.get('tools', [])}
                results = [c for m in body.get('messages', [])
                           for c in (m.get('content', []) if isinstance(m.get('content'), list) else [])
                           if c.get('type') == 'tool_result']
                resumed = 'RESUME_AUDIT' in json.dumps(body.get('messages', []))
                analysis_id = 'resumed_analysis' if resumed else 'analysis'
                if 'Bash' in names and not any(r.get('tool_use_id') == analysis_id for r in results):
                    content = [
                        {'type': 'tool_use', 'id': analysis_id, 'name': 'Bash',
                         'input': {'command': analysis_command(policy, source), 'description': 'Analyze fixture data in scratch'}},
                        {'type': 'tool_use', 'id': 'fetch', 'name': 'WebFetch',
                         'input': {'url': 'https://example.com', 'prompt': 'Return the page title.'}},
                    ]
                    if resumed:
                        content = content[:1]
                    reason = 'tool_use'
                else:
                    content = [{'type': 'text', 'text': 'Example Domain; analysis complete'}]
                    reason = 'end_turn'
                message = {'id': 'msg_peer_audit', 'type': 'message', 'role': 'assistant',
                           'model': body.get('model', 'claude-sonnet-4-6'), 'content': content,
                           'stop_reason': reason, 'stop_sequence': None,
                           'usage': {'input_tokens': 10, 'output_tokens': 10}}
                self.send_response(200)
                self.send_header('Content-Type', 'text/event-stream' if body.get('stream') else 'application/json')
                self.end_headers()
                if not body.get('stream'):
                    self.wfile.write(json.dumps(message).encode())
                    return
                events = [('message_start', {'type': 'message_start', 'message': {**message, 'content': [], 'stop_reason': None}})]
                for index, block in enumerate(content):
                    initial = {**block, 'input': {}} if block['type'] == 'tool_use' else {**block, 'text': ''}
                    delta = ({'type': 'input_json_delta', 'partial_json': json.dumps(block['input'])}
                             if block['type'] == 'tool_use' else {'type': 'text_delta', 'text': block['text']})
                    events.extend([
                        ('content_block_start', {'type': 'content_block_start', 'index': index, 'content_block': initial}),
                        ('content_block_delta', {'type': 'content_block_delta', 'index': index, 'delta': delta}),
                        ('content_block_stop', {'type': 'content_block_stop', 'index': index}),
                    ])
                events.extend([
                    ('message_delta', {'type': 'message_delta', 'delta': {'stop_reason': reason, 'stop_sequence': None}, 'usage': {'output_tokens': 10}}),
                    ('message_stop', {'type': 'message_stop'}),
                ])
                try:
                    for name, event in events:
                        self.wfile.write(f'event: {name}\ndata: {json.dumps(event)}\n\n'.encode())
                except (BrokenPipeError, ConnectionResetError) as error:
                    failures.append(str(error))

            def log_message(self, *_):
                pass

        provider = ThreadingHTTPServer(('127.0.0.1', 0), Provider)
        threading.Thread(target=provider.serve_forever, daemon=True).start()
        env.update(ANTHROPIC_BASE_URL=f'http://127.0.0.1:{provider.server_port}',
                   ANTHROPIC_API_KEY='local-audit-only', CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC='1')
        try:
            result = subprocess.run(
                ['claude', '--safe-mode', '--strict-mcp-config', '--setting-sources', '',
                 *policy['claudeArgs'], '--append-system-prompt', policy['instructions'],
                 '--model', 'claude-sonnet-4-6', '-p', 'Audit analysis and URL tools',
                 '--output-format', 'json', '--max-turns', '4'],
                env=env, cwd=root, capture_output=True, text=True, timeout=90)
            names = {t.get('name') for body in captured for t in body.get('tools', [])}
            assert {'Bash', 'Read', 'Glob', 'Grep', 'WebFetch', 'WebSearch'} <= names, (names, result.stdout, result.stderr)
            assert not names.intersection({'Edit', 'Write', 'NotebookEdit', 'Agent', 'Task', 'ExitPlanMode'}), names
            results = [c for body in captured for m in body.get('messages', [])
                       for c in (m.get('content', []) if isinstance(m.get('content'), list) else [])
                       if c.get('type') == 'tool_result']
            assert any('ANALYSIS_OK 4096' in json.dumps(r) for r in results), (results, result.stdout, result.stderr)
            fetch = [r for r in results if r.get('tool_use_id') == 'fetch']
            assert fetch and any(not r.get('is_error') and 'Example Domain' in json.dumps(r) for r in fetch), fetch
            assert source.read_text() == 'source data\n' * 4096
            assert result.returncode == 0, (result.stdout, result.stderr, failures)
            assert any(policy['scratch'] in json.dumps(body.get('system')) for body in captured)
            session = json.loads(result.stdout)['session_id']
            env.update(dict(policy['resumedEnv']))
            resumed = subprocess.run(
                ['claude', '--safe-mode', '--strict-mcp-config', '--setting-sources', '',
                 *policy['claudeArgs'], '--append-system-prompt', policy['resumedInstructions'],
                 '--resume', session, '--model', 'claude-sonnet-4-6', '-p', 'RESUME_AUDIT',
                 '--output-format', 'json', '--max-turns', '4'],
                env=env, cwd=root, capture_output=True, text=True, timeout=90)
            assert resumed.returncode == 0, (resumed.stdout, resumed.stderr)
            assert (Path(policy['resumedScratch']) / 'analysis.py').is_file(), resumed.stdout
            resumed_requests = [b for b in captured if 'RESUME_AUDIT' in json.dumps(b.get('messages'))]
            assert any(policy['resumedScratch'] in json.dumps(b.get('system')) for b in resumed_requests)
            assert source.read_text() == 'source data\n' * 4096
            print('Claude: temporary Python analysis executed, URL fetched, search exposed, mutation tools hidden', flush=True)
            print('Claude: restored session received fresh scratch and current instructions', flush=True)
        finally:
            provider.shutdown()
            provider.server_close()


def codex(policy):
    with tempfile.TemporaryDirectory(prefix='cide-peer-codex-', dir='target') as tmp:
        root = Path(tmp).resolve()
        env = environment(policy, root)
        source = root / 'source.txt'
        source.write_text('source data\n' * 4096)
        subprocess.run(['git', 'init', '--quiet', str(root)], check=True)
        process = subprocess.Popen(['codex', *policy['codexArgs'], 'app-server', '--listen', 'stdio://'],
                                   cwd=root, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=subprocess.DEVNULL, text=True, start_new_session=True)
        def request(rid, method, params):
            process.stdin.write(json.dumps({'id': rid, 'method': method, 'params': params}) + '\n')
            process.stdin.flush()
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                if not select.select([process.stdout], [], [], max(0, deadline - time.monotonic()))[0]:
                    break
                line = process.stdout.readline()
                assert line, 'Codex app-server closed'
                message = json.loads(line)
                if message.get('id') == rid:
                    assert 'error' not in message, message
                    return message['result']
            raise AssertionError(f'No Codex reply to {method}')
        try:
            request(1, 'initialize', {'clientInfo': {'name': 'peer-reviewer-audit', 'version': '1'}, 'capabilities': {'experimentalApi': True}})
            process.stdin.write('{"method":"initialized"}\n')
            process.stdin.flush()
            config = request(2, 'config/read', {'cwd': str(root), 'includeLayers': False})['config']
            assert config['default_permissions'] == 'cide-peer-reviewer'
            assert config['web_search'] == 'live'
            for rid, command, success in [
                (3, ['bash', '-c', analysis_command(policy, source)], True),
                (4, ['python3', '-c', f'from pathlib import Path; Path({str(source)!r}).write_text("changed")'], False),
                (5, ['python3', '-c', f'from pathlib import Path; Path({str(root / ".git/config")!r}).write_text("changed")'], False),
            ]:
                result = request(rid, 'command/exec', {'command': command, 'cwd': str(root),
                                 'permissionProfile': 'cide-peer-reviewer', 'env': dict(policy['env']), 'timeoutMs': 10000})
                assert (result['exitCode'] == 0) == success, result
                if success:
                    assert 'ANALYSIS_OK 4096' in result['stdout'], result
                else:
                    assert 'PermissionError' in result['stderr'] or 'Read-only file system' in result['stderr'], result
            assert source.read_text() == 'source data\n' * 4096
            print('Codex: scratch analysis executed, project/Git writes blocked, live web search configured', flush=True)
        finally:
            stop(process)


if __name__ == '__main__':
    policy = json.load(sys.stdin)
    harness = sys.argv[1]
    if harness == 'opencode':
        os.environ['CIDE_PEER_AUDIT_POLICY'] = json.dumps(policy)
        runpy.run_path('scripts/audit-opencode-peer-tools.py', run_name='__main__')
    elif harness == 'claude':
        claude(policy)
    elif harness == 'codex':
        codex(policy)
    else:
        raise SystemExit(f'Unknown harness: {harness}')
