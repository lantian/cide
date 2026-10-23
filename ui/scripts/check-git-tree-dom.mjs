/** Exercise disclosure clicks through the real tree and drag hook, including prior gestures. */
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { JSDOM } from 'jsdom'
import React, { act, useState } from 'react'

mkdirSync('node_modules/.cache', { recursive: true })
const out = mkdtempSync('node_modules/.cache/cide-git-tree-dom-')
const dom = new JSDOM('<div id="root"></div>', {
  url: 'https://cide.test', pretendToBeVisual: true,
})
for (const name of ['window', 'document', 'location', 'HTMLElement', 'Element', 'Node'])
  globalThis[name] = dom.window[name]
globalThis.IS_REACT_ACT_ENVIRONMENT = true

// JSDOM has no layout or pointer capture. Hit testing is supplied per gesture, and the
// release is sent to the captured source explicitly; the production event handlers run.
HTMLElement.prototype.setPointerCapture = () => {}
HTMLElement.prototype.hasPointerCapture = () => false
HTMLElement.prototype.releasePointerCapture = () => {}

let root
try {
  const entry = join(out, 'audit.ts')
  writeFileSync(entry, [
    `export { ChangesTree } from ${JSON.stringify(resolve('src/sidebar/GitPanel/ChangesTree.tsx'))}`,
    `export * from ${JSON.stringify(resolve('src/sidebar/GitPanel/model.ts'))}`,
    `export * from ${JSON.stringify(resolve('src/sidebar/GitPanel/rowSelection.ts'))}`,
  ].join('\n'))
  execFileSync('node', [
    'node_modules/vite/bin/vite.js', 'build', '--ssr', entry,
    '--outDir', join(out, 'bundle'), '--logLevel', 'error',
  ], { stdio: 'inherit' })
  const {
    ChangesTree, normalizeStatus, buildRows, defaultExpanded, toggleRow,
    carriedIds, pressSelect, releaseSelect, keySelect, NO_ROWS,
  } = await import(`file://${resolve(out, 'bundle/audit.js')}`)
  const { createRoot } = await import('react-dom/client')

  const change = (path, changelist = 'default') => ({
    path, origPath: null, index: 'unmodified', worktree: 'modified', staged: false,
    binary: false, submodule: false, changelist,
  })
  const viewFor = (emptyChanges = false) => normalizeStatus({ repos: [{
    repo: { id: 'repo', name: 'repo', root: '/test/repo', parent: null, isSubmodule: false },
    branch: { head: 'main', detached: false, upstream: null, ahead: 0, behind: 0,
      operation: null, unborn: false },
    changelists: [
      { id: 'default', name: 'Changes', active: true, comment: '',
        changes: emptyChanges ? [] : [change('src/a.ts'), change('src/b.ts')] },
      { id: 'fixes', name: 'fixes', active: false, comment: '',
        changes: [change('other.ts', 'fixes')] },
    ],
    unversioned: [], ignored: [], conflicts: [], indexChangedExternally: false,
    useStagingArea: false,
  }] })
  let view = viewFor()
  const calls = { toggles: [], checks: [], releases: [], moves: [], opens: [] }
  const noop = () => {}
  function Harness({ view }) {
    const [expanded, setExpanded] = useState(() => defaultExpanded(view))
    const [selection, setSelection] = useState(NO_ROWS)
    const [selected, setSelected] = useState(new Set())
    const [current, setCurrent] = useState(null)
    const rows = buildRows(view, expanded)
    return React.createElement(ChangesTree, {
      rows, view, expanded, selection, selected, current, carried: carriedIds(rows, selection),
      onCurrent: setCurrent,
      onPress: (id, mods) => {
        const plan = pressSelect(rows, selection, id, mods)
        setCurrent(id)
        if (!plan.deferred) setSelection(plan.next)
        return plan.deferred
      },
      onRelease: id => {
        calls.releases.push(id)
        setSelection(prev => releaseSelect(rows, prev, id))
      },
      onKeyTo: (id, mods) => {
        setCurrent(id)
        setSelection(prev => keySelect(rows, prev, id, mods))
      },
      onSelectAll: noop, onCollapseSelection: noop, onToggleCheckSelected: noop,
      diffOpen: false, iconTheme: 'light',
      onToggleCheck: row => {
        calls.checks.push(row.id)
        setSelected(prev => toggleRow(row, prev))
      },
      onToggleExpand: row => {
        calls.toggles.push(row.id)
        setExpanded(prev => {
          const next = new Set(prev)
          if (!next.delete(row.id)) next.add(row.id)
          return next
        })
      },
      onOpenDiff: (...args) => calls.opens.push(args),
      onOpenFile: (...args) => calls.opens.push(args),
      onMovePaths: (...args) => calls.moves.push(args),
      onTrackPaths: (...args) => calls.moves.push(args),
    })
  }
  root = createRoot(document.getElementById('root'))
  let mountId = 0
  const render = async () => act(async () => root.render(
    React.createElement(Harness, { key: mountId, view }),
  ))
  const mount = async () => {
    mountId++
    view = viewFor()
    for (const list of Object.values(calls)) list.length = 0
    await render()
  }
  const rowsOf = kind => [...document.querySelectorAll(`[data-audit="gitRow"][data-kind="${kind}"]`)]
  const group = () => rowsOf('group')[0]
  const directory = () => rowsOf('dir')[0]
  const arrow = row => row.querySelector(
    '[data-icon="chevron-down"], [data-icon="chevron-right"]',
  ).firstElementChild
  const checkbox = row => row.querySelector('[data-audit="gitCheck"]')
  const ghost = () => document.querySelector('[data-audit="gitDragGhost"]')
  const expanded = row => row.getAttribute('aria-expanded')
  const selectedRows = () => [...document.querySelectorAll('[aria-selected="true"]')]
  const mouse = async (target, type, options = {}) => {
    const event = new window.MouseEvent(type, {
      bubbles: true, cancelable: true, button: 0, detail: 1, clientX: 20, clientY: 20, ...options,
    })
    Object.defineProperty(event, 'pointerId', { value: options.pointerId ?? 1 })
    await act(async () => target.dispatchEvent(event))
  }
  const click = async (target, options) => {
    for (const type of ['pointerdown', 'mousedown', 'pointerup', 'mouseup', 'click'])
      await mouse(target, type, options)
  }
  const key = async (target, key) => act(async () => target.dispatchEvent(
    new window.KeyboardEvent('keydown', { bubbles: true, cancelable: true, key }),
  ))
  const drag = async (ending, source = rowsOf('file')[0], options = {}, during = noop) => {
    const movesBefore = calls.moves.length
    const togglesBefore = calls.toggles.length
    const releasesBefore = calls.releases.length
    let ownGroup = source
    while (ownGroup.dataset.kind !== 'group') ownGroup = ownGroup.previousElementSibling
    const otherGroup = rowsOf('group').find(row => row !== ownGroup)
    document.elementFromPoint = () => ending === 'noop' ? ownGroup : otherGroup
    await mouse(source, 'pointerdown', options)
    await mouse(source, 'mousedown', options)
    await mouse(window, 'pointermove', { ...options, clientX: 30 })
    assert.ok(ghost(), 'movement past the threshold starts a real drag')
    const outcome = ending === 'noop' ? 'noop' : otherGroup === group() ? 'track' : 'move'
    assert.equal(ghost().dataset.outcome, outcome, ghost().textContent)
    await during()
    if (ending === 'escape') await key(window, 'Escape')
    if (ending === 'cancel') await mouse(source, 'pointercancel', options)
    await mouse(source, 'pointerup', options)
    await mouse(source, 'mouseup', options)
    assert.equal(ghost(), null, 'ending the gesture removes its ghost')
    assert.equal(calls.moves.length - movesBefore, ending === 'move' ? 1 : 0)
    assert.equal(calls.toggles.length, togglesBefore, 'drag release never folds a row')
    assert.equal(calls.releases.length, releasesBefore, 'drag release preserves the selection')
  }

  // Without the fix, the very first post-drag arrow click below is ignored. Use both
  // directions and nested SVG targets, with no intervening row-body click to clear the bug.
  for (const ending of ['move', 'noop', 'escape', 'cancel']) {
    for (const target of [group, directory]) {
      for (const initiallyOpen of [true, false]) {
        await mount()
        if (!initiallyOpen) await click(arrow(target()))
        // The other changelist stays visible even when Changes is collapsed.
        await drag(ending, rowsOf('file').at(-1))
        const before = calls.toggles.length
        await click(arrow(target()))
        assert.equal(expanded(target()), String(!initiallyOpen), `${ending}: first arrow click toggles`)
        await click(arrow(target()))
        assert.equal(expanded(target()), String(initiallyOpen), `${ending}: next arrow click toggles back`)
        assert.equal(calls.toggles.length, before + 2)
      }
    }
  }

  await mount()
  await drag('move', directory(), { detail: 2 })
  assert.equal(expanded(directory()), 'true', 'a directory double-press that drags does not fold')
  view = viewFor(true)
  await render()
  assert.match(group().textContent, /Changes0$/, 'the refreshed Changes group has zero files')
  await click(arrow(group()))
  assert.equal(expanded(group()), 'false', 'an empty Changes group can still collapse after a drag')
  await click(arrow(group()))
  assert.equal(expanded(group()), 'true', 'an empty Changes group can expand again')

  await mount()
  await click(rowsOf('file')[0])
  await click(rowsOf('file')[1], { ctrlKey: true })
  const selectedBefore = selectedRows().map(row => row.dataset.rowId)
  await drag('move')
  assert.deepEqual(calls.moves, [['repo', 'fixes', ['src/a.ts', 'src/b.ts']]])
  assert.deepEqual(selectedRows().map(row => row.dataset.rowId), selectedBefore)
  await click(rowsOf('file')[0])
  assert.equal(selectedRows().length, 1, 'the next plain click releases the deferred selection')
  assert.equal(calls.releases.length, 1)

  // Controls stop the bubbling pointer press. Moving on either control must never drag.
  for (const control of [arrow, checkbox]) {
    await mount()
    await drag('noop')
    const target = control(group())
    await mouse(target, 'pointerdown')
    await mouse(target, 'mousedown')
    await mouse(window, 'pointermove', { clientX: 30 })
    assert.equal(ghost(), null, 'a control cannot become a drag handle')
    await mouse(target, 'pointerup')
    await mouse(target, 'mouseup')
    await mouse(target, 'click')
    assert.equal(calls.toggles.length, control === arrow ? 1 : 0)
    assert.equal(calls.checks.length, control === checkbox ? 1 : 0)
    assert.equal(calls.opens.length, 0)
    if (control === checkbox) {
      assert.equal(group().getAttribute('aria-checked'), 'true')
      await click(arrow(group()))
      assert.equal(expanded(group()), 'false', 'checkbox clicks do not leave the arrow stuck')
    }
  }

  await mount()
  await drag('noop')
  await click(arrow(group()))
  await click(arrow(group()), { detail: 2 })
  assert.equal(calls.toggles.length, 1, 'an arrow double-click toggles once')
  await key(group(), 'ArrowRight')
  assert.equal(expanded(group()), 'true')
  await key(group(), 'ArrowLeft')
  assert.equal(expanded(group()), 'false')

  await mount()
  await drag('move', directory(), { detail: 2 }, async () => {
    await mouse(arrow(group()), 'pointerdown', { pointerId: 2 })
  })
  assert.equal(expanded(directory()), 'true', 'another pointer cannot clear an active drag verdict')

  console.log('git tree interactions: ok')
} finally {
  if (root) await act(async () => root.unmount())
  dom.window.close()
  rmSync(out, { recursive: true, force: true })
}
