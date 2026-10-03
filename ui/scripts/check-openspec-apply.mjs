/** Real-browser Apply lifecycle check. Requires Chromium, like demo:shots; override with CHROMIUM. */
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { existsSync, mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { createServer } from 'vite'
import react from '@vitejs/plugin-react'

const ui = resolve(import.meta.dirname, '..')
const chromium = [process.env.CHROMIUM, '/usr/bin/chromium', '/usr/bin/chromium-browser',
  '/usr/bin/google-chrome', '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome']
  .find((path) => path && existsSync(path))
assert.ok(chromium, 'Chromium is required; set CHROMIUM=/path/to/chromium')

// Exercise the real dialog, launcher controls and store, with no native IPC or model turn.
const mocks = {
  ipc: `export const specSessions = {
    settings: async () => ({ applyInWorktree: false }),
    harnesses: async () => [{ harness: 'codex', unavailable: null }],
  };
  export const settings = { effective: async () => ({ consoleHarness: 'codex' }) };`,
  agents: `import { create } from 'zustand';
    export const useAgents = create(() => ({ project: 'test-project', roster: { kind: 'ready', agents: [] } }));`,
  runs: `export function startSession(project, request) {
    window.applyCheck.launches.push({ project, request });
    return window.applyCheck.launch();
  }`,
  notices: `export function notifyFailure(error) { window.applyCheck.failures.push(String(error)); }`,
}

const html = `<!doctype html><html><body><button id="outside">Underlying control</button><div id="root"></div>
<script type="module">
import React from 'react';
import { createRoot } from 'react-dom/client';
import { flushSync } from 'react-dom';
import { ApplyDialog, useApplyDialog } from '/src/sidebar/OpenSpecPanel/ApplyDialog.tsx';
import '/src/styles/tokens.css';

const result = { checks: [] };
window.applyCheck = { launches: [], failures: [], launch: async () => 'mock-run' };
const mock = window.applyCheck;
let documentEscapes = 0, parentEscapes = 0;
document.addEventListener('keydown', (event) => { if (event.key === 'Escape') documentEscapes++; });
const assert = (condition, message) => { if (!condition) throw Error(message); result.checks.push(message); };
// Drain React's asynchronous launcher updates explicitly; Chromium's virtual clock can
// advance timers before React's MessageChannel scheduler has committed those promises.
const settle = async () => {
  window.IS_REACT_ACT_ENVIRONMENT = true;
  try { await React.act(async () => { await Promise.resolve(); }); }
  finally { window.IS_REACT_ACT_ENVIRONMENT = false; }
};
const dialog = () => document.querySelector('[data-audit="openspecApplyDialog"]');
const instructions = () => dialog().querySelector('textarea');
const outside = document.getElementById('outside');
const key = (key, target = document.activeElement) => {
  const event = new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true });
  flushSync(() => target.dispatchEvent(event));
  return event;
};
const escape = (target) => {
  const beforeDocument = documentEscapes, beforeParent = parentEscapes;
  const event = key('Escape', target);
  assert(event.defaultPrevented, 'handled Escape prevents default');
  assert(documentEscapes === beforeDocument && parentEscapes === beforeParent,
    'handled Escape does not reach native or React observers outside the portal');
};
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
const type = (text) => {
  const target = instructions();
  target.focus();
  Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set.call(target, text);
  flushSync(() => target.dispatchEvent(new Event('input', { bubbles: true })));
};
const open = async (change = 'test-change') => {
  outside.focus();
  flushSync(() => useApplyDialog.getState().open(change));
  assert(dialog().contains(document.activeElement), 'opening places focus inside Apply');
  await settle();
};
const closed = () => assert(!dialog() && useApplyDialog.getState().change === null, 'Escape closes the store and dialog');

let root;
try {
  root = createRoot(document.getElementById('root'));
  flushSync(() => root.render(React.createElement('div', {
    onKeyDown: (event) => { if (event.key === 'Escape') parentEscapes++; },
  }, React.createElement(ApplyDialog, { project: 'test-project' }))));
  assert(!dialog(), 'host starts closed');

  // No wait or click between opening and Escape: focus must be established before paint.
  outside.focus();
  flushSync(() => useApplyDialog.getState().open('immediate'));
  assert(dialog().contains(document.activeElement), 'immediate opening receives focus');
  escape();
  closed();
  assert(mock.launches.length === 0, 'immediate Escape never launches');

  await open();
  type('keep the old API');
  assert(instructions().value === 'keep the old API', 'instructions accept input');
  escape();
  closed();
  await open();
  assert(instructions().value === '', 'dismissal clears draft instructions on reopening');
  const ordinary = key('a');
  assert(!ordinary.defaultPrevented && !!dialog(), 'other keys retain normal handling');

  const launcher = dialog().querySelector('[role="combobox"]');
  assert(!launcher.disabled, 'launcher control is ready to receive focus');
  launcher.focus();
  assert(document.activeElement === launcher, 'launcher control receives focus');
  // Changing the selected change while visible also reruns the opening effect.
  flushSync(() => useApplyDialog.getState().open('another-change'));
  assert(document.activeElement === launcher, 'opening preserves focus already inside');
  await settle();
  escape();
  closed();
  assert(mock.launches.length === 0, 'Escape from instructions and launcher never launches');

  await open();
  const dropdown = dialog().querySelector('[role="combobox"]');
  dropdown.focus();
  key('ArrowDown');
  await settle();
  assert(dropdown.getAttribute('aria-expanded') === 'true', 'launcher popup opens normally');
  key('Escape');
  assert(!!dialog() && dropdown.getAttribute('aria-expanded') === 'false', 'nested popup consumes its first Escape');
  escape();
  closed();

  await open();
  type('launch instructions');
  const start = dialog().querySelector('[data-audit="openspecApplyStart"]');
  assert(!start.disabled, 'mocked launcher is ready');
  flushSync(() => start.click());
  await settle();
  assert(!dialog() && mock.launches.length === 1, 'successful launch closes Apply once');
  assert(mock.launches[0].project === 'test-project'
    && mock.launches[0].request.text === 'launch instructions'
    && mock.launches[0].request.launcher.harness === 'codex', 'launch receives the selected launcher and instructions');
  await open();
  assert(instructions().value === '', 'successful launch clears instructions before reopening');
  escape();
  closed();

  // Hold the same launch pending across repeated Escape, then resolve it normally.
  const pending = deferred();
  mock.launch = () => pending.promise;
  await open();
  type('pending instructions');
  flushSync(() => dialog().querySelector('[data-audit="openspecApplyStart"]').click());
  assert(mock.launches.length === 2, 'pending session starts once');
  dialog().querySelector('[aria-label="Close"]').focus();
  escape();
  escape();
  assert(!!dialog() && useApplyDialog.getState().change === 'test-change'
    && instructions().value === 'pending instructions', 'pending Escape retains the dialog and instructions');
  assert(dialog().querySelector('[data-audit="openspecApplyStart"]').disabled
    && mock.launches.length === 2, 'pending Escape does not duplicate launch');
  pending.resolve('pending-run');
  await settle();
  assert(!dialog() && mock.launches.length === 2, 'pending Escape does not cancel successful launch');

  const failed = deferred();
  mock.launch = () => failed.promise;
  await open();
  assert(instructions().value === '', 'pending success clears instructions');
  type('failed instructions');
  flushSync(() => dialog().querySelector('[data-audit="openspecApplyStart"]').click());
  failed.reject(Error('mock launch failure'));
  await settle();
  assert(!!dialog() && instructions().value === 'failed instructions'
    && !dialog().querySelector('[data-audit="openspecApplyStart"]').disabled,
    'failure retains instructions and releases the launch guard');
  assert(mock.failures.length === 1 && mock.failures[0].includes('mock launch failure'), 'launch failure is reported');
  instructions().focus();
  escape();
  closed();
  await open();
  assert(instructions().value === '', 'Escape after failure clears instructions');
  escape();
  closed();

  outside.focus();
  const beforeOutside = documentEscapes;
  const unhandled = key('Escape');
  assert(!unhandled.defaultPrevented && document.activeElement === outside, 'closed host does not intercept Escape');
  assert(documentEscapes === beforeOutside + 1, 'Escape outside a closed dialog reaches the bubbling observer');
  assert(mock.launches.length === 3, 'reopening and dismissal do not launch again');
  result.ok = true;
} catch (error) {
  result.error = String(error.stack);
} finally {
  if (root) flushSync(() => root.unmount());
  document.body.innerHTML = '<pre id="result"></pre>';
  document.getElementById('result').textContent = JSON.stringify(result);
}
</script></body></html>`

const profile = mkdtempSync(join(tmpdir(), 'cide-apply-check-'))
let server, child
let browserClosed = Promise.resolve()
try {
  server = await createServer({
    root: ui, configFile: false, logLevel: 'error',
    resolve: { alias: { '@': join(ui, 'src') } },
    plugins: [react(), {
      name: 'apply-browser-fixture', enforce: 'pre',
      resolveId(source, importer) {
        // Vite's alias plugin runs before user pre-plugins, including this mock resolver.
        const aliased = (path) => source === '@/' + path || source === join(ui, 'src', path)
          || source === join(ui, 'src', path) + '.ts'
        const mock = aliased('ipc/client') ? 'ipc'
          : aliased('chrome/notices') ? 'notices'
          : source === '../agentsStore' && importer?.endsWith('/LauncherPicker.tsx') ? 'agents'
          : source === './specRuns' && importer?.endsWith('/ApplyDialog.tsx') ? 'runs' : null
        return mock === null ? null : '\0apply-check:' + mock
      },
      load(id) { return id.startsWith('\0apply-check:') ? mocks[id.slice('\0apply-check:'.length)] : null },
      configureServer(server) {
        server.middlewares.use('/apply-check', async (_request, response, next) => {
          try {
            response.setHeader('Content-Type', 'text/html')
            response.end(await server.transformIndexHtml('/apply-check', html))
          } catch (error) { next(error) }
        })
      },
    }],
    server: { host: '127.0.0.1', port: 0 },
  })
  await server.listen()
  const output = await new Promise((resolve, reject) => {
    child = spawn(chromium, ['--headless=new', '--disable-gpu', '--no-first-run',
      '--no-default-browser-check', '--disable-dev-shm-usage', '--no-sandbox',
      '--user-data-dir=' + profile, '--dump-dom', '--virtual-time-budget=10000',
      'http://127.0.0.1:' + server.httpServer.address().port + '/apply-check'])
    browserClosed = new Promise((resolve) => child.once('close', resolve))
    let stdout = '', stderr = ''
    child.stdout.on('data', (chunk) => {
      stdout += chunk
      // Some macOS Chrome versions dump the DOM but keep running. The complete fixture
      // result is the completion signal; finally stops only our temporary-profile child.
      if (/<pre id="result">.*?<\/pre>/s.test(stdout)) {
        clearTimeout(timeout)
        resolve(stdout)
      }
    })
    child.stderr.on('data', (chunk) => { stderr += chunk })
    const timeout = setTimeout(() => { child.kill(); reject(Error('Apply browser check timed out: ' + stderr.slice(-1500) + stdout.slice(-1500))) }, 45000)
    child.on('error', (error) => { clearTimeout(timeout); reject(error) })
    child.on('exit', (code) => {
      clearTimeout(timeout)
      code === 0 ? resolve(stdout) : reject(Error('Chromium exited ' + code + ': ' + stderr.slice(-1500)))
    })
  })
  const encoded = output.match(/<pre id="result">(.*?)<\/pre>/s)?.[1]
  assert.ok(encoded, 'Browser produced no fixture result: ' + output.slice(-1500))
  const result = JSON.parse(encoded.replaceAll('&lt;', '<').replaceAll('&gt;', '>').replaceAll('&quot;', '"').replaceAll('&amp;', '&'))
  assert.ok(result.ok, result.error)
  console.log('OpenSpec Apply: ' + result.checks.length + ' browser assertions passed')
} finally {
  // kill() only sends a signal. Chrome can still write to its profile until it exits,
  // so removing that directory immediately races shutdown even after every assertion passes.
  const forceStop = child?.pid && child.exitCode === null && child.signalCode === null
    ? setTimeout(() => child.kill('SIGKILL'), 5000)
    : undefined
  child?.kill()
  try {
    await browserClosed
  } finally {
    clearTimeout(forceStop)
  }
  await server?.close()
  rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 })
}
