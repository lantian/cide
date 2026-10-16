#!/usr/bin/env python3
"""Helper for the ignored cide-ide-mcp real_opencode test. No paid endpoints or user state.

The Rust test supplies the real IDE tools through cide-hook's existing MCP bridge.
Run: cargo build --locked -p cide-hook
     cargo test --locked -p cide-ide-mcp real_opencode -- --ignored --nocapture
"""
import base64
import fcntl
import json
import os
from pathlib import Path
import pty
import signal
import socket
import struct
import subprocess
import sys
import termios
import threading
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def run(control, direct):
    root = Path(control['root']) / ('direct' if direct else 'codemode')
    root.mkdir()
    source = root / 'audit.txt'
    source.write_text('disk text differs\nsecond line\n')
    ide = root / '.claude/ide'
    ide.mkdir(parents=True)
    (ide / (str(control['port']) + '.lock')).write_text(json.dumps({
        'pid': control['pid'], 'workspaceFolders': [str(root)], 'ideName': 'cide',
        'transport': 'ws', 'authToken': control['token'],
    }))
    captured = []
    tool_calls = [
        ('getDiagnostics', {}),
        ('openFile', {'filePath': str(source), 'startLine': 0, 'endLine': 1}),
        ('openDiff', {'old_file_path': str(source), 'new_file_path': str(source),
                      'new_file_contents': 'proposed', 'tab_name': 'audit-diff'}),
        ('close_tab', {'tab_name': 'audit-diff'}),
        ('closeAllDiffTabs', {}),
    ]

    class Provider(BaseHTTPRequestHandler):
        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            captured.append(body)
            functions = [tool.get('function', {}) for tool in body.get('tools', [])]
            names = [tool.get('name') for tool in functions]
            previous = [m for m in body.get('messages', []) if m.get('role') == 'tool']
            invoke = bool(functions) and len(previous) < (5 if direct else 2)
            stage = 0 if not previous else 1
            delta = {'role': 'assistant'}
            if invoke:
                if direct:
                    selected = tool_calls[:3] if stage == 0 else tool_calls[3:]
                    calls = [('cide_' + name, args) for name, args in selected]
                    assert all(name in names for name, _ in calls), names
                else:
                    assert 'execute' in names, names
                    if stage == 0:
                        code = 'console.log(await search({query:"cide"}));'
                    else:
                        assert all('tools.cide.' + name in json.dumps(previous) for name, _ in tool_calls), previous
                        code = '\n'.join('console.log(await tools.cide.' + name + '(' + json.dumps(args) + '));'
                                         for name, args in tool_calls)
                    calls = [('execute', {'code': code})]
                delta['tool_calls'] = [
                    {'index': i, 'id': f'ide_{stage}_{i}', 'type': 'function',
                     'function': {'name': name, 'arguments': json.dumps(args)}}
                    for i, (name, args) in enumerate(calls)
                ]
            else:
                delta['content'] = 'IDE audit complete'
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.end_headers()
            for event in [{'delta': delta, 'finish_reason': None},
                          {'delta': {}, 'finish_reason': 'tool_calls' if invoke else 'stop'}]:
                self.wfile.write(('data: ' + json.dumps({
                    'id': 'ide-audit', 'object': 'chat.completion.chunk', 'created': 0,
                    'model': 'test', 'choices': [{'index': 0, **event}],
                }) + '\n\n').encode())
            self.wfile.write(b'data: [DONE]\n\n')

        def log_message(self, *args):
            pass

    provider = ThreadingHTTPServer(('127.0.0.1', 0), Provider)
    threading.Thread(target=provider.serve_forever, daemon=True).start()
    mcp = {'type': 'local', 'command': [control['hook'], 'mcp'],
           'timeout': {'execution': 604800000}, 'disabled': False}
    if direct:
        mcp['codemode'] = False
    config = {
        'model': 'ide-audit/test',
        'providers': {'ide-audit': {
            'package': 'aisdk:@ai-sdk/openai-compatible',
            'settings': {'baseURL': f'http://127.0.0.1:{provider.server_port}/v1', 'apiKey': 'local-only'},
            'models': {'test': {'name': 'IDE audit model', 'limit': {'context': 32000, 'output': 2000}}},
        }},
        'mcp': {'servers': {'cide': mcp}},
        'permissions': [{'action': '*', 'resource': '*', 'effect': 'allow'}],
    }
    env = {key: value for key, value in os.environ.items()
           if not key.startswith(('OPENCODE_', 'OPENAI_', 'ANTHROPIC_', 'CODEX_', 'CIDE_', 'CLAUDE_'))}
    env.update(HOME=str(root), XDG_CONFIG_HOME=str(root / 'config'),
               XDG_DATA_HOME=str(root / 'data'), XDG_STATE_HOME=str(root / 'state'),
               XDG_CACHE_HOME=str(root / 'cache'), OPENCODE_CONFIG_CONTENT=json.dumps(config),
               OPENCODE_SERVER_PASSWORD='local-audit-only', OPENCODE_SERVER_USERNAME='opencode',
               CIDE_AGENT_SOCK=control['socket'], CIDE_SESSION='ide-audit-session',
               TERM='xterm-256color', COLORTERM='truecolor')
    # Both ports are deliberately absent, as in the production launch plan.
    env.pop('OPENCODE_EDITOR_SSE_PORT', None)
    binary = os.environ.get('OPENCODE_BINARY', 'opencode')
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]
    url = f'http://127.0.0.1:{port}'

    def request(path, body=None):
        auth = base64.b64encode(b'opencode:local-audit-only').decode()
        req = urllib.request.Request(url + path, headers={
            'Authorization': 'Basic ' + auth, 'Content-Type': 'application/json'},
            data=None if body is None else json.dumps(body).encode())
        with urllib.request.urlopen(req, timeout=3) as response:
            return json.load(response)

    processes = []
    master = None
    terminal = bytearray()
    with (root / 'server.log').open('w+') as log:
        try:
            server = subprocess.Popen([binary, 'serve', '--hostname', '127.0.0.1', '--port', str(port)],
                                      cwd=root, env=env, stdout=log, stderr=log, start_new_session=True)
            processes.append(server)
            for _ in range(150):
                try:
                    request('/openapi.json')
                    break
                except Exception:
                    if server.poll() is not None:
                        raise RuntimeError('OpenCode server exited')
                    time.sleep(.1)
            else:
                raise RuntimeError('OpenCode server did not start')
            session = request('/api/session', {'location': {'directory': str(root)}})['data']['id']
            master, slave = pty.openpty()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 32, 120, 0, 0))
            tui = subprocess.Popen([binary, '--server', url, '--session', session], cwd=root, env=env,
                                   stdin=slave, stdout=slave, stderr=slave, start_new_session=True)
            processes.append(tui)
            os.close(slave)

            def read_terminal():
                try:
                    while True:
                        terminal.extend(os.read(master, 65536))
                except OSError:
                    pass

            threading.Thread(target=read_terminal, daemon=True).start()
            started = time.monotonic()
            time.sleep(5)
            assert tui.poll() is None, terminal.decode(errors='replace')[-1200:]
            context = b'\x1b[200~@audit.txt (lines 1-2) \n\nUNSAVED_IDE_CONTEXT_71\n  preserved indentation\x1b[201~'
            os.write(master, context)
            time.sleep(2)
            assert not any(body.get('tools') for body in captured), 'Pasting context submitted a turn'
            os.write(master, b'\r')
            results = []
            for _ in range(250):
                results = [message for body in captured for message in body.get('messages', [])
                           if message.get('role') == 'tool']
                if len({message['tool_call_id'] for message in results}) >= (5 if direct else 2):
                    break
                assert tui.poll() is None, terminal.decode(errors='replace')[-1200:]
                time.sleep(.1)
            assert results, 'No tool results; terminal=' + terminal.decode(errors='replace')[-1500:]
            encoded = json.dumps(captured)
            assert all(text in encoded for text in ('UNSAVED_IDE_CONTEXT_71', 'audit.txt', '  preserved indentation')), 'Editor context did not reach the model intact'
            assert 'FILE_SAVED' in json.dumps(results), results
            while time.monotonic() - started < 30:
                assert tui.poll() is None
                time.sleep(.1)
            print(('direct' if direct else 'codemode') + ': editor paste stayed unsubmitted; all five tools ran; native connection observed for 30 seconds', flush=True)
        except Exception:
            log.flush()
            log.seek(0)
            print(log.read()[-1800:], file=sys.stderr)
            for candidate in root.glob('data/**/log/*.log'):
                lines = candidate.read_text(errors='replace').splitlines()
                print('\n'.join(line for line in lines if any(word in line.lower() for word in ('mcp', 'cide-hook', 'config')))[:5000], file=sys.stderr)
            raise
        finally:
            for process in reversed(processes):
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGTERM)
            for process in processes:
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
            if master is not None:
                os.close(master)
            provider.shutdown()
            provider.server_close()


if __name__ == '__main__':
    control = json.loads(Path(sys.argv[1]).read_text())
    for direct in (False, True):
        run(control, direct)
