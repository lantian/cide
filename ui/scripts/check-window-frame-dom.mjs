/** Event coverage used by check:window-controls. Mount the actual frame and mock only IPC. */
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { resolve } from 'node:path'
import { JSDOM } from 'jsdom'
import React, { act } from 'react'

export async function checkWindowFrameEvents(agents) {
  // SSR keeps React and Tauri external, so its output needs node_modules above it.
  mkdirSync('node_modules/.cache', { recursive: true })
  const out = mkdtempSync('node_modules/.cache/cide-window-frame-')
  const dom = new JSDOM('<!doctype html><div id="root"></div>', {
    url: 'https://cide.test',
    pretendToBeVisual: true,
  })
  for (const name of ['window', 'document', 'HTMLElement', 'Element'])
    globalThis[name] = dom.window[name]
  globalThis.IS_REACT_ACT_ENVIRONMENT = true

  let root
  try {
    execFileSync('node', [
      'node_modules/vite/bin/vite.js', 'build',
      '--ssr', 'src/chrome/WindowFrame.tsx',
      '--outDir', out, '--logLevel', 'error',
    ], { stdio: 'inherit' })
    const { WindowFrame, useWindowChrome, toggleMaximize } =
      await import(`file://${resolve(out, 'WindowFrame.js')}`)
    const { createRoot } = await import('react-dom/client')

    const calls = []
    let maximized = false
    let nativeMaximizable = false
    let nextListener = 0
    window.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'shell:test' } },
      transformCallback: () => 1,
      invoke: async (command) => {
        calls.push(command)
        if (command === 'plugin:window|is_maximized') return maximized
        if (command === 'plugin:window|toggle_maximize') maximized = !maximized
        if (command === 'plugin:window|internal_toggle_maximize' && nativeMaximizable)
          maximized = !maximized
        if (command === 'plugin:event|listen') return ++nextListener
      },
    }
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} }

    function Probe() {
      const { isMaximized } = useWindowChrome()
      return React.createElement('output', { id: 'state' }, String(isMaximized))
    }
    const markup = `
      <header>
        <div id="filler" data-tauri-drag-region data-window-drag="true"></div>
        <button id="action">Header action</button>
        <button id="zoom" data-window-button="zoom">Zoom</button>
      </header>
      <header>
        <div id="detached" data-tauri-drag-region data-window-drag="true"></div>
      </header>
      <div id="fallback" data-window-drag="true"><span id="fallback-child"></span></div>
      <div id="outside"></div>
    `
    const mount = async () => {
      root = createRoot(document.getElementById('root'))
      await act(async () => root.render(React.createElement(WindowFrame, null,
        React.createElement(Probe),
        React.createElement('div', { dangerouslySetInnerHTML: { __html: markup } }),
      )))
    }
    const unmount = async () => {
      await act(async () => root.unmount())
      root = null
    }
    const mouse = (id, type, options = {}) => {
      const event = new window.MouseEvent(type, {
        bubbles: true, composed: true, cancelable: true,
        button: 0, detail: 2, clientX: 100, clientY: 12, ...options,
      })
      document.getElementById(id).dispatchEvent(event)
      return event
    }
    const doubleClick = async (id, releaseOptions = {}, releaseId = id) => {
      calls.length = 0
      await act(async () => {
        mouse(id, 'mousedown', { detail: 1 })
        mouse(id, 'mouseup', { detail: 1 })
        mouse(id, 'click', { detail: 1 })
        mouse(id, 'mousedown')
        mouse(releaseId, 'mouseup', releaseOptions)
        mouse(releaseId, 'click', releaseOptions)
        mouse(releaseId, 'dblclick', releaseOptions)
      })
    }
    const count = (command) => calls.filter((call) => call === `plugin:window|${command}`).length

    for (const [platform, userAgent] of Object.entries(agents)) {
      Object.defineProperty(globalThis, 'navigator', {
        configurable: true, value: { userAgent },
      })
      const isMac = platform === 'mac'
      nativeMaximizable = !isMac

      // Model Tauri 2.11.5's built-in listeners for the empty drag regions we render.
      // Install before mounting: the application capture listener must beat this listener.
      let press = null
      const nativeRegion = (event) => event.target instanceof HTMLElement &&
        event.target.matches('[data-tauri-drag-region=""]')
      const nativeDown = (event) => {
        if (event.button !== 0 || ![1, 2].includes(event.detail) || !nativeRegion(event)) return
        if (isMac && event.detail === 2) {
          press = { x: event.clientX, y: event.clientY }
          return
        }
        event.preventDefault()
        event.stopImmediatePropagation()
        void window.__TAURI_INTERNALS__.invoke(event.detail === 2
          ? 'plugin:window|internal_toggle_maximize'
          : 'plugin:window|start_dragging')
      }
      const nativeUp = (event) => {
        if (isMac && event.button === 0 && event.detail === 2 && nativeRegion(event) &&
          press?.x === event.clientX && press?.y === event.clientY)
          void window.__TAURI_INTERNALS__.invoke('plugin:window|internal_toggle_maximize')
      }
      document.addEventListener('mousedown', nativeDown)
      document.addEventListener('mouseup', nativeUp)
      await mount()

      for (const id of ['filler', 'detached']) {
        const before = maximized
        await doubleClick(id)
        assert.equal(maximized, !before, `${platform}: ${id} maximizes/restores`)
        assert.equal(count('toggle_maximize'), isMac ? 1 : 0, `${platform}: public command count`)
        assert.equal(count('internal_toggle_maximize'), isMac ? 0 : 1,
          `${platform}: exactly one handler owns the gesture`)
        assert.equal(count('start_dragging'), 1, `${platform}: first press still starts dragging`)
        if (isMac) assert.equal(document.getElementById('state').textContent, String(maximized))
      }

      await doubleClick('filler')
      assert.equal(count(isMac ? 'toggle_maximize' : 'internal_toggle_maximize'), 1,
        `${platform}: repeated double click also toggles once`)

      if (isMac) {
        // Even if a future native window has a zoom button, never run both commands.
        nativeMaximizable = true
        const before = maximized
        await doubleClick('filler')
        assert.equal(maximized, !before, 'mac: no double toggle with a native zoom button')
        assert.equal(count('internal_toggle_maximize'), 0, 'mac: native handler is intercepted')

        for (const options of [{ clientX: 101 }, { clientY: 13 }]) {
          const before = maximized
          await doubleClick('filler', options)
          assert.equal(maximized, before, 'mac: movement cancels maximize')
          assert.equal(count('toggle_maximize'), 0)
        }
        await doubleClick('filler', {}, 'outside')
        assert.equal(count('toggle_maximize'), 0, 'mac: release outside cancels maximize')
        await act(async () => mouse('filler', 'mouseup'))
        assert.equal(count('toggle_maximize'), 0, 'mac: canceled press is cleared')

        calls.length = 0
        await act(async () => {
          mouse('action', 'mousedown')
          mouse('filler', 'mouseup')
        })
        assert.equal(count('toggle_maximize'), 0, 'mac: press outside is ignored')
      }

      calls.length = 0
      await act(async () => {
        mouse('filler', 'mousedown', { detail: 1 })
        mouse('filler', 'mouseup', { detail: 1 })
        mouse('filler', 'mousedown', { button: 2 })
        mouse('filler', 'mouseup', { button: 2 })
      })
      assert.equal(count('start_dragging'), 1, `${platform}: single click keeps dragging`)
      assert.equal(count('toggle_maximize'), 0, `${platform}: single/right clicks do not maximize`)
      assert.equal(count('internal_toggle_maximize'), 0)

      await doubleClick('action')
      assert.equal(count('toggle_maximize'), 0, `${platform}: header action stays a button`)
      await doubleClick('fallback-child')
      assert.equal(count('toggle_maximize'), 1, `${platform}: unowned dblclick fallback still works`)

      calls.length = 0
      await act(async () => mouse('zoom', 'click', { detail: 1 }))
      assert.equal(count('toggle_maximize'), 1, `${platform}: zoom control still works`)
      calls.length = 0
      await act(async () => { await toggleMaximize() })
      assert.equal(count('toggle_maximize'), 1, `${platform}: project-tab maximize API still works`)

      // Leave a press pending across detach/remount. No stale release or duplicate listeners.
      mouse('filler', 'mousedown')
      await unmount()
      assert.ok(calls.includes('plugin:event|unlisten'), `${platform}: resize listener is removed`)
      // Test detached document listeners using a region outside React's now-empty root.
      const orphan = document.createElement('div')
      orphan.id = 'orphan'
      orphan.setAttribute('data-tauri-drag-region', '')
      orphan.setAttribute('data-window-drag', 'true')
      document.body.append(orphan)
      calls.length = 0
      await act(async () => {
        mouse('orphan', 'mouseup')
        mouse('orphan', 'dblclick')
      })
      assert.equal(count('toggle_maximize'), 0, `${platform}: app listeners are detached`)
      if (isMac) assert.equal(count('internal_toggle_maximize'), 1,
        'mac: detached capture listener no longer blocks native handler')
      orphan.remove()

      await mount()
      calls.length = 0
      await act(async () => mouse('filler', 'mouseup'))
      assert.equal(count('toggle_maximize'), 0, `${platform}: remount clears pending press`)
      await doubleClick('filler')
      assert.equal(count(isMac ? 'toggle_maximize' : 'internal_toggle_maximize'), 1,
        `${platform}: remount does not duplicate handlers`)
      await unmount()
      document.removeEventListener('mousedown', nativeDown)
      document.removeEventListener('mouseup', nativeUp)
    }
  } finally {
    if (root) await act(async () => root.unmount())
    dom.window.close()
    rmSync(out, { recursive: true, force: true })
  }
}
