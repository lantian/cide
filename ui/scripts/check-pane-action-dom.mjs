/** Event coverage used by check:awaiting. Mount the actual pane frame; mock only IPC.
 * JSDOM cannot measure layout, so check the cause of the missed click: acknowledgement
 * must not remove Waiting until the button's action has run.
 */
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { resolve } from 'node:path'
import { JSDOM } from 'jsdom'
import React, { act } from 'react'

export async function checkPaneActionEvents() {
  mkdirSync('node_modules/.cache', { recursive: true })
  const out = mkdtempSync('node_modules/.cache/cide-pane-action-')
  const dom = new JSDOM('<!doctype html><div id="root"></div>', {
    url: 'https://cide.test',
    pretendToBeVisual: true,
  })
  for (const name of ['window', 'document', 'location', 'HTMLElement', 'Element', 'Node'])
    globalThis[name] = dom.window[name]
  globalThis.self = dom.window
  globalThis.IS_REACT_ACT_ENVIRONMENT = true
  globalThis.requestAnimationFrame = dom.window.requestAnimationFrame.bind(dom.window)
  globalThis.cancelAnimationFrame = dom.window.cancelAnimationFrame.bind(dom.window)
  // The imported terminal library probes canvas support; this fixture renders no terminal.
  dom.window.HTMLCanvasElement.prototype.getContext = () => null

  const callbacks = new Map()
  const listeners = new Map()
  const calls = []
  const session = 'waiting-session'
  let nextCallback = 0
  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: 'shell:test' } },
    transformCallback: (callback) => {
      callbacks.set(++nextCallback, callback)
      return nextCallback
    },
    invoke: async (command, args) => {
      if (command === 'plugin:event|listen') {
        listeners.set(args.event, callbacks.get(args.handler))
        return args.handler
      }
      if (command === 'window_awaiting_sessions') return [session]
      if (command === 'plugin:window|is_maximized') return false
      if (command === 'window_set_awaiting') calls.push(['acknowledge', args])
      else calls.push([command])
    },
  }
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} }

  let root
  try {
    execFileSync('node', [
      'node_modules/vite/bin/vite.js', 'build',
      '--ssr', 'src/layout/PaneTitleBar.tsx',
      '--outDir', out, '--logLevel', 'error',
    ], { stdio: 'inherit' })
    const { PaneFrame } = await import(`file://${resolve(out, 'PaneTitleBar.js')}`)
    const { createRoot } = await import('react-dom/client')
    root = createRoot(document.getElementById('root'))
    let mountId = 0
    const pane = { id: 'pane', kind: 'claude', title: 'Console', role: 'primary', session }
    const frame = () => document.querySelector('[data-audit="pane"]')
    const waiting = () => frame().dataset.awaiting === 'true'
    const button = (label) => frame().querySelector(`button[aria-label="${label}"]`)
    const action = (name) => () => {
      assert.equal(waiting(), true, `${name} runs while Waiting is still visible`)
      calls.push([name])
    }
    const mount = async (props = {}, detached = false) => {
      dom.reconfigure({ url: `https://cide.test${detached ? '?window=pane:test' : ''}` })
      await act(async () => root.render(React.createElement(PaneFrame, {
        key: ++mountId,
        pane,
        index: 1,
        focused: false,
        maximized: false,
        onFocus: () => calls.push(['focus']),
        onAddTile: action('add'),
        onMaximize: action('maximize'),
        onDetach: action('detach'),
        onClose: action('close'),
        onMove: () => {},
        ...props,
      }, React.createElement('textarea', { id: 'terminal' }))))
      // Keep focus in the pane content, as the cluster's mousedown guard does in the app.
      // JSDOM emits a window blur when focus moves from BODY to a portalled menu.
      await act(async () => document.getElementById('terminal').focus())
      await act(async () => listeners.get('cide://session-awaiting')({
        payload: { sessions: [session] },
      }))
      assert.equal(waiting(), true)
      assert.equal(frame().querySelector('[data-tone="accent"]').textContent, 'Waiting')
      calls.length = 0
    }
    const mouse = async (target, type, options = {}) => {
      const event = new window.MouseEvent(type, {
        bubbles: true, cancelable: true, button: 0, ...options,
      })
      await act(async () => target.dispatchEvent(event))
      return event
    }
    const ack = ['acknowledge', { session, awaiting: false }]
    const acknowledgements = () => calls.filter(([name]) => name === 'acknowledge')

    // The original failure: pressing fullscreen removes the badge before mouse release.
    // Use separate acts so the pointer-down render is flushed before the later click.
    for (const maximized of [false, true]) {
      await mount({ maximized })
      const control = button(maximized ? 'Restore pane' : 'Maximize pane')
      const icon = control.querySelector('path')
      await mouse(icon, 'pointerdown')
      assert.equal(waiting(), true, 'pointer-down does not shift the button')
      assert.deepEqual(calls, [['focus']], 'pointer-down still raises the pane')
      const press = await mouse(icon, 'mousedown')
      assert.equal(press.defaultPrevented, true, 'mouse press keeps terminal DOM focus')
      await act(async () => control.focus())
      assert.equal(waiting(), true, 'focus does not shift the button either')
      calls.length = 0
      await mouse(icon, 'pointerup')
      await mouse(icon, 'mouseup')
      await mouse(icon, 'click')
      assert.deepEqual(calls, [['maximize'], ack], 'action precedes acknowledgement exactly once')
      assert.equal(waiting(), false)
      assert.equal(frame().querySelector('[data-tone="accent"]'), null)
    }

    await mount()
    const canceled = button('Maximize pane')
    await mouse(canceled, 'pointerdown')
    await mouse(canceled, 'pointercancel')
    await mouse(document.getElementById('terminal'), 'pointerup')
    assert.equal(waiting(), true, 'a canceled press leaves Waiting visible')
    assert.equal(acknowledgements().length, 0)

    // Keyboard activation produces a click with detail 0, after the button takes focus.
    await mount()
    const keyboard = button('Maximize pane')
    await act(async () => keyboard.focus())
    assert.equal(waiting(), true, 'Tab focus alone does not acknowledge a pane action')
    calls.length = 0
    await act(async () => keyboard.click())
    assert.deepEqual(calls, [['maximize'], ack], 'keyboard activation uses the same order')

    for (const [label, name] of [
      ['Detach pane into a window', 'detach'],
      ['Close pane', 'close'],
    ]) {
      await mount({ pane: { ...pane, role: 'auxiliary' } })
      const control = button(label)
      await mouse(control.querySelector('svg'), 'pointerdown')
      assert.equal(waiting(), true, `${name} also waits for the click`)
      calls.length = 0
      await mouse(control.querySelector('svg'), 'click')
      assert.deepEqual(calls, [[name], ack])
    }

    await mount()
    const add = button('Add a pane to this row')
    await mouse(add.querySelector('[data-icon="chevron-down"] path'), 'pointerdown')
    assert.equal(waiting(), true, 'the add-menu caret also defers acknowledgement')
    await mouse(add, 'click')
    assert.equal(add.getAttribute('aria-expanded'), 'true', 'the add menu opens on the first click')
    assert.deepEqual(acknowledgements(), [ack])

    // Other acknowledgement routes and detached-window exceptions keep their behavior.
    for (const pointerButton of [0, 1, 2]) {
      await mount()
      await mouse(document.getElementById('terminal'), 'pointerdown', { button: pointerButton })
      assert.equal(waiting(), pointerButton === 2, 'left/middle content press clears; right does not')
    }
    await mount()
    await act(async () => document.getElementById('terminal').blur())
    await act(async () => document.getElementById('terminal').focus())
    assert.equal(waiting(), false, 'terminal focus still acknowledges immediately')
    await mount()
    await mouse(frame().querySelector('[data-pane-grab]'), 'pointerdown')
    assert.equal(waiting(), false, 'the drag handle keeps its pointer-down acknowledgement')
    await mount()
    await mouse(button('Maximize pane'), 'pointerdown', { button: 2 })
    assert.equal(waiting(), true, 'right-clicking a pane action does not acknowledge')

    await mount({}, true)
    for (const name of ['minimize', 'zoom', 'close']) {
      const control = frame().querySelector(`[data-window-button="${name}"]`)
      await mouse(control.querySelector('path'), 'pointerdown')
      await act(async () => control.focus())
      await mouse(control.querySelector('path'), 'click')
      assert.equal(waiting(), true, `${name} window control never acknowledges`)
    }
    assert.equal(acknowledgements().length, 0)
  } finally {
    if (root) await act(async () => root.unmount())
    dom.window.close()
    rmSync(out, { recursive: true, force: true })
  }
}
