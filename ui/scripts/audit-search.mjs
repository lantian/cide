/** Real mounted search and bottom tabs with deterministic IPC; requires headless Chromium. */
import assert from 'node:assert/strict'
import { build, preview } from 'vite'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { spawn } from 'node:child_process'
const ui = resolve(import.meta.dirname, '..')
mkdirSync(join(ui, 'node_modules/.cache'), { recursive: true })
const dir = mkdtempSync(join(ui, 'node_modules/.cache/search-audit-'))
writeFileSync(join(dir, 'index.html'), '<html data-theme="light"><body style="margin:0"><div id="root"></div><script type="module" src="./main.tsx"></script></body></html>')
writeFileSync(join(dir, 'main.tsx'), readFileSync(join(ui, 'scripts/search-fixture.tsx')))
let server, child, ws
try {
  await build({ configFile: false, root: dir, publicDir: join(ui, 'public'), logLevel: 'error', resolve: { alias: { '@': join(ui, 'src') } }, build: { outDir: join(dir, 'dist') } })
  server = await preview({ configFile: false, root: dir, logLevel: 'error', build: { outDir: join(dir, 'dist') }, preview: { port: 0 } })
  const chromium = [process.env.CHROMIUM, '/usr/bin/chromium', '/usr/bin/chromium-browser', '/usr/bin/google-chrome'].find((bin) => bin && existsSync(bin))
  assert(chromium, 'Set CHROMIUM to a headless Chromium executable')
  child = spawn(chromium, ['--headless=new', '--remote-debugging-port=0', '--no-first-run', '--no-default-browser-check', '--window-size=1400,1000', '--user-data-dir='+join(dir,'profile'), 'about:blank'], { stdio: ['ignore','ignore','pipe'] })
  const port = await new Promise((resolve, reject) => {
    let stderr = ''
    const timer = setTimeout(() => reject(new Error('browser startup timeout: '+stderr)), 20000)
    child.stderr.on('data', (data) => { stderr += data; const match = /DevTools listening on ws:\/\/[^:]+:(\d+)\//.exec(stderr); if (match) { clearTimeout(timer); resolve(+match[1]) } })
    child.on('exit', (code) => { clearTimeout(timer); reject(new Error('chromium '+code+' '+stderr)) })
  })
  const targets = await (await fetch('http://127.0.0.1:'+port+'/json/list')).json()
  ws = new WebSocket(targets.find((target) => target.type === 'page').webSocketDebuggerUrl)
  await new Promise((resolve,reject) => { ws.onopen=resolve;ws.onerror=reject })
  let id = 0
  const pending = new Map(), errors = []
  ws.onmessage = (event) => {
    const message = JSON.parse(event.data)
    if (message.id) { const request = pending.get(message.id); pending.delete(message.id); message.error ? request.reject(message.error) : request.resolve(message.result) }
    else if (message.method === 'Runtime.exceptionThrown') errors.push(message.params.exceptionDetails)
  }
  const send = (method,params={}) => new Promise((resolve,reject) => { const next=++id;pending.set(next,{resolve,reject});ws.send(JSON.stringify({id:next,method,params})) })
  const evaluate = async (expression) => {
    const result = await send('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true})
    if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails))
    return result.result.value
  }
  const until = async (expression) => {
    for (let attempt=0;attempt<100;attempt++) {
      if (await evaluate('Boolean('+expression+')')) return
      await new Promise((resolve)=>setTimeout(resolve,30))
    }
    const scrollState = await evaluate(`(() => {
      const list = document.querySelector('[role="listbox"][aria-label="Search results"]')
      const heading = document.querySelector('[data-sticky=true]')
      return { top: list?.scrollTop, listTop: list?.getBoundingClientRect().top, height: list?.scrollHeight,
        heading: heading?.title, headingTop: heading?.getBoundingClientRect().top, style: heading?.getAttribute('style') }
    })()`)
    throw new Error('Timed out: '+expression+'\nBrowser errors: '+JSON.stringify(errors)+'\nScroll state: '+JSON.stringify(scrollState))
  }
  const click = async (selector) => {
    await until('document.querySelector('+JSON.stringify(selector)+')')
    return evaluate('document.querySelector('+JSON.stringify(selector)+').click()')
  }
  const key = async (key, modifiers=0) => {
    const codes={PageDown:34,PageUp:33,g:71,b:66,a:65,c:67,ArrowDown:40,ArrowUp:38,ArrowLeft:37,ArrowRight:39,Enter:13,Escape:27,Home:36,End:35,Tab:9,o:79,p:80,f:70,n:78,Shift:16}
    const params={key,modifiers,windowsVirtualKeyCode:codes[key]??0}
    await send('Input.dispatchKeyEvent',{...params,type:'keyDown'})
    await send('Input.dispatchKeyEvent',{...params,type:'keyUp'})
  }
  const type = async (text) => { await send('Input.insertText',{text}) }
  const focused = (label) => evaluate('document.activeElement?.getAttribute("aria-label") === '+JSON.stringify(label))
  const clear = async () => { await evaluate('document.activeElement.select()');await type('') }

  const modal = '[data-audit="searchEverywhere"]'
  const query = '[data-audit="searchInput"]'
  const pin = '[title="Pin code results (Ctrl+Enter)"]'
  const pinned = () => evaluate('window.search.getState().tabs.project ?? []')
  const replace = async text => { await evaluate(`document.querySelector(${JSON.stringify(query)}).focus(); document.activeElement.select()`); await type(text) }
  const chooseTab = async label => { await evaluate(`Array.from(document.querySelectorAll('[role="tab"]')).find(el=>el.textContent.includes(${JSON.stringify(label)})).click()`) }
  await send('Runtime.enable')
  await send('Page.navigate', { url: server.resolvedUrls.local[0] })
  await until('window.ready && document.querySelector("#behind")')
  await evaluate('document.querySelector("#behind").focus()')
  await key('Shift', 8); await key('Shift', 8)
  await until(`document.querySelector('${modal}')`)
  assert(await focused('Search query'), 'double Shift focuses the query immediately')
  await type('alpha')
  await until('document.querySelector("[data-audit=searchEverywhere] [data-audit=searchPreview] .cm-content")')
  assert.equal(await evaluate('window.opened.length'), 0, 'initial preview must not open an editor')
  await click('[data-audit="searchResultRow"][title="beta.ts"]')
  await until('document.querySelector("[data-audit=searchEverywhere] [data-audit=searchPreview]").textContent.includes("const beta")')
  assert.equal(await evaluate('window.opened.length'), 0, 'single click previews without opening')
  assert.equal(await evaluate('window.caret().path'), '/original.ts', 'modal preview leaves caret ownership alone')
  await until('document.querySelector("[data-audit=searchResultRow] img[src*=typescript]")?.naturalWidth > 0')
  assert(await evaluate('!!document.querySelector("[data-heading=provider]") && !!document.querySelector("[data-heading=file]")'), 'provider and file headings have separate levels')
  await evaluate(`document.querySelector('${query}').focus()`)
  const beforePage = await evaluate('window.search.getState().views["modal:project"].selected')
  await key('PageDown')
  const afterPage = await evaluate('window.search.getState().views["modal:project"].selected')
  assert.notEqual(afterPage, beforePage, 'PageDown from the query moves selection')
  await key('PageUp')
  assert.notEqual(await evaluate('window.search.getState().views["modal:project"].selected'), afterPage)
  assert.equal(await evaluate('window.opened.length'), 0, 'page navigation never opens files')

  // Mode shortcuts work from the list, query, filters and preview, without changing the query.
  await evaluate(`document.querySelector('[aria-label="Search results"]').focus()`)
  for (const label of ['Files', 'Symbols', 'Actions', 'Text', 'All']) {
    await key('ArrowRight', 8)
    await until(`document.querySelector('${modal} [role=tab][aria-selected=true]')?.textContent === ${JSON.stringify(label)}`)
    assert(await focused('Search query'))
    assert.equal(await evaluate(`document.querySelector('${query}').value`), 'alpha')
  }
  await evaluate('document.querySelector("[aria-label^=Folder]").focus()')
  await key('ArrowLeft', 8)
  await until(`document.querySelector('${modal} [role=tab][aria-selected=true]')?.textContent === 'Text'`)
  await until(`document.querySelector('${modal} .cm-content')`)
  await evaluate(`document.querySelector('${modal} .cm-content').focus()`)
  await key('ArrowRight', 8)
  await until(`document.querySelector('${modal} [role=tab][aria-selected=true]')?.textContent === 'All'`)
  await until(`document.querySelector('[data-audit=searchResultRow][title="alpha.ts"]')`)
  await evaluate(`document.querySelector('${query}').setSelectionRange(2, 2)`)
  await key('ArrowLeft')
  assert.equal(await evaluate(`document.querySelector('${query}').selectionStart`), 1, 'ordinary query arrows still move the caret on result rows')
  await key('ArrowUp') // First file -> Files provider heading.
  for (const direction of ['ArrowLeft', 'ArrowLeft', 'ArrowRight', 'ArrowRight']) {
    await key(direction)
    const expanded = direction === 'ArrowRight'
    assert.equal(await evaluate('document.querySelector("[data-heading=provider][title=Files]").getAttribute("aria-expanded")'), String(expanded))
    assert.equal(await evaluate(`!!document.querySelector('[data-audit=searchResultRow][title="alpha.ts"]:not([data-heading])')`), expanded)
  }

  await evaluate('window.layout("stacked")')
  await until('document.querySelector("[data-audit=searchEverywhere] [role=separator]").getAttribute("aria-orientation") === "horizontal"')
  assert(await evaluate('(() => { const s = document.querySelector("[data-audit=searchEverywhere] [data-audit=searchResults]"); return s.children[2].getBoundingClientRect().top >= s.children[0].getBoundingClientRect().bottom })()'))
  await evaluate('document.querySelector("[data-audit=searchEverywhere] [role=separator]").focus()')
  await key('ArrowDown')
  assert.equal(await evaluate('window.search.getState().views["modal:project"].stackedRatio'), 55)
  await evaluate('window.layout("sideBySide")')
  await until('document.querySelector("[data-audit=searchEverywhere] [role=separator]").getAttribute("aria-orientation") === "vertical"')

  await evaluate('document.querySelector("[aria-label^=Folder]").focus()')
  await type('ga')
  await until('document.querySelector("[aria-label^=Project] [role=option]")')
  await key('ArrowDown'); await key('Tab')
  assert.equal(await evaluate('document.querySelector("[aria-label^=Folder]").value'), 'game/ui')
  assert(await evaluate(`!!document.querySelector('${modal}')`), 'accepting a folder keeps the search open')
  await clear(); await type('slow'); await new Promise(r => setTimeout(r, 150))
  await clear(); await type('ga')
  await until('document.querySelector("[aria-label^=Project] [role=option]")')
  await new Promise(r => setTimeout(r, 350))
  assert(await evaluate('document.querySelector("[aria-label^=Project]").textContent.includes("game/ui")'), 'stale folder response ignored')
  await key('Escape')
  assert(await evaluate(`!!document.querySelector('${modal}')`), 'first Escape closes only completion')
  await clear()
  await evaluate(`document.querySelector('${query}').focus()`)
  await until('document.querySelector("[data-audit=searchEverywhere] .cm-content")')
  writeFileSync('/tmp/cide-search-modal.png', Buffer.from((await send('Page.captureScreenshot', { format: 'png' })).data, 'base64'))
  await key('Escape')
  await until('document.activeElement.id === "behind"')

  // Text searches finish on the first page, but the other 250 hits must still arrive.
  await key('f', 10)
  await until('document.querySelector("[role=tab][aria-selected=true]")?.textContent === "Text"')
  await until('Object.values(window.search.getState().frames).some(f=>f.session.descriptor.source==="text" && f.text.hits.length===450)')
  await key('Enter', 2)
  await until('!document.querySelector("[data-audit=searchEverywhere]") && document.querySelector("[data-audit=pinnedSearch]")')
  const first = (await pinned())[0]
  assert.equal(first.descriptor.query.pattern, 'alpha')
  assert.equal(await evaluate(`window.search.getState().frames[${JSON.stringify(first.id)}].text.hits.length`), 450)
  assert.equal(await evaluate('window.jobs.size'), 1, 'pin transfers ownership across modal cleanup')

  // A pinned preview is a focused, read-only editor even when no file tab is active.
  await until('document.querySelector("[data-cide-embedded-editor]")')
  assert.equal(await evaluate('window.caret().path'), '/original.ts', 'mounting a pin does not steal the caret')
  await evaluate('window.layout("stacked")')
  assert.equal(await evaluate('document.querySelector("[data-audit=pinnedSearch] [data-audit=searchResults]").dataset.layout'), 'sideBySide', 'modal setting cannot change bottom orientation')
  await evaluate('window.layout("sideBySide")')
  assert(await evaluate('Array.from(document.querySelectorAll("[data-audit=pinnedSearch] mark")).every(mark => mark.parentElement.textContent.startsWith("const "))'), 'snippets trim indentation without moving the highlighted match')
  await evaluate('window.previewView().focus()')
  await until('window.caret().path === "/alpha.ts"')
  assert(await evaluate('window.keyContext().editorFocused && !window.keyContext().terminalFocused'))
  await key('a', 2)
  assert(await evaluate('window.previewView().state.selection.main.to > 100'), 'Select All selects source text')
  const originalText = await evaluate('window.previewView().state.doc.toString()')
  await type('must not edit')
  assert.equal(await evaluate('window.previewView().state.doc.toString()'), originalText, 'typing cannot edit the source')
  await evaluate('(() => { const el = document.querySelector("[data-cide-embedded-editor]"); const b = el.getBoundingClientRect(); el.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, clientX: b.left + 80, clientY: b.top + 15 })) })()')
  await until('document.querySelector("[role=menu]")')
  assert(await evaluate('Array.from(document.querySelectorAll("[role=menuitem]")).some(el => el.textContent.startsWith("Cut") && el.getAttribute("aria-disabled") === "true")'), 'mutating context actions are disabled')
  await evaluate('Array.from(document.querySelectorAll("[role=menuitem]")).find(el => el.textContent.startsWith("Copy")).click()')
  await until('window.clipboard?.length > 100')
  assert.equal(await evaluate('window.clipboard'), originalText, 'the normal Copy menu copies source selection')
  await key('f', 2)
  await until('document.querySelector("[data-audit=pinnedSearch] input[placeholder=Find]")')
  await type('alpha'); await key('Escape')
  await evaluate('window.previewView().focus()')
  await key('g', 2)
  await until('document.querySelector("[data-audit=gotoInput]")')
  await clear(); await type('42:3'); await key('Enter')
  await until('window.caret().line === 42 && window.caret().column === 3')
  assert.equal(await evaluate('window.opened.length'), 0, 'Go to Line stays in the bottom preview')
  assert(await evaluate('window.previewView().hasFocus'))
  const caretBeforeReveal = await evaluate('window.previewView().state.selection.main.head')
  await evaluate('window.requestReveal("/alpha.ts", { line: 100, column: 1, endColumn: 1 })')
  assert.equal(await evaluate('window.previewView().state.selection.main.head'), caretBeforeReveal, 'file-tab reveals cannot be consumed by a preview')
  assert(await evaluate('window.pendingReveals().includes("/alpha.ts")'))
  await key('b', 2)
  await until('window.opened.some(row => row[1] === "/definition.ts")')
  assert.equal(await evaluate('window.definitions.at(-1)[0]'), '/alpha.ts', 'definition uses the real file path')
  await evaluate('window.opened = []')

  await key('f', 10); await until(`document.querySelector('${query}')`); await replace('beta')
  await until('Array.from(window.jobs.values()).some(s=>!s.pinned && s.descriptor.query.pattern==="beta")')
  await until(`!document.querySelector('${pin}').disabled`)
  await key('Enter', 2)
  await until('(window.search.getState().tabs.project?.length ?? 0) === 2 && !document.querySelector("[data-audit=searchEverywhere]")')
  assert.deepEqual((await pinned()).map(s => s.descriptor.query.pattern), ['alpha', 'beta'])
  assert.equal(await evaluate(`window.search.getState().frames[${JSON.stringify(first.id)}].session.descriptor.query.pattern`), 'alpha')

  // Edit is a new modal, and pinning the same descriptor again creates a distinct tab.
  await chooseTab('Text: alpha')
  await evaluate('Array.from(document.querySelectorAll("button")).find(b=>b.textContent==="Edit search").click()')
  await until(`document.querySelector('${query}')?.value === 'alpha'`)
  await until(`!document.querySelector('${pin}').disabled`)
  await key('Enter', 2)
  await until('(window.search.getState().tabs.project?.length ?? 0) === 3 && !document.querySelector("[data-audit=searchEverywhere]")')
  const all = await pinned()
  assert.equal(new Set(all.map(s => s.id)).size, 3)
  writeFileSync('/tmp/cide-search-pinned.png', Buffer.from((await send('Page.captureScreenshot', { format: 'png' })).data, 'base64'))
  await evaluate('Array.from(document.querySelectorAll("button")).find(b=>b.textContent==="Refresh").click()')
  await until(`window.search.getState().tabs.project.find(s=>s.id===${JSON.stringify(all[2].id)}).generation === 1`)
  assert.equal((await pinned()).length, 3, 'refresh replaces a run, not the tab')
  await evaluate('window.panel(false)')
  await until('!document.querySelector("[data-audit=pinnedSearch]")')
  await evaluate('window.panel(true)')
  await until('document.querySelector("[data-audit=pinnedSearch]")')
  assert.equal((await pinned()).length, 3, 'hiding the panel retains pins')

  // A stale list response cannot erase a pin made while the request was in flight.
  await evaluate('window.holdList = true; void window.sync()')
  await until('window.finishList')
  await key('f', 10); await until(`document.querySelector('${query}')`); await replace('gamma')
  await until('Array.from(window.jobs.values()).some(s=>!s.pinned && s.descriptor.query.pattern==="gamma")')
  await until(`!document.querySelector('${pin}').disabled`); await key('Enter', 2)
  await until('(window.search.getState().tabs.project?.length ?? 0) === 4')
  await evaluate('window.finishList()')
  assert.equal((await pinned()).length, 4)

  // File modifiers retain their established behavior.
  for (const [modifier, expected] of [[8, 'split'], [1, 'mention']]) {
    await key('p', 2); await until(`document.querySelector('${query}')`)
    await until(`document.querySelector(${JSON.stringify('[data-audit="searchResultRow"][title="alpha.ts"]')})`)
    await key('Enter', modifier)
    await until('!document.querySelector("[data-audit=searchEverywhere]")')
    assert.equal(await evaluate('window.opened.at(-1)[0]'), expected)
  }
  await key('p', 10); await until(`document.querySelector('${query}')`); await replace('Audit action')
  await key('Enter', 2)
  assert.equal(await evaluate('window.ran.at(-1)'), 'session:audit.action')

  // Preview read failures and validation errors remain in the modal.
  await key('f', 10); await until(`document.querySelector('${query}')`); await replace('unavailable')
  await until('document.querySelector("[data-audit=searchEverywhere] [data-audit=searchPreview]").textContent.includes("Preview unavailable")')
  assert(await evaluate('!document.querySelector("[data-audit=searchEverywhere] [data-audit=searchPreview]").textContent.includes("[object Object]")'))
  assert(await evaluate('document.querySelector("[data-audit=searchEverywhere] [data-audit=searchPreview]").textContent.includes("The file was removed.")'))
  await click('[aria-label="Regular expression"]'); await replace('[')
  await until('document.querySelector("[data-audit=searchEverywhere]").textContent.includes("Invalid regular expression")')
  assert(await evaluate(`document.querySelector('${pin}').disabled`))
  await click('[aria-label="Regular expression"]'); await replace('absent')
  await until('document.querySelector("[data-audit=searchEverywhere]").textContent.includes("No results")')
  await replace('delayed'); await new Promise(r => setTimeout(r, 170)); await key('Escape')
  await new Promise(r => setTimeout(r, 400))
  assert.equal(await evaluate('Array.from(window.jobs.values()).filter(s=>!s.pinned).length'), 0, 'late Start is cancelled after dismissal')

  // A modal unmounted by another overlay while Pin is in flight must not cancel its job.
  await key('f', 10); await until(`document.querySelector('${query}')`); await replace('transfer')
  await until('Array.from(window.jobs.values()).some(s=>!s.pinned && s.descriptor.query.pattern==="transfer")')
  await until(`!document.querySelector('${pin}').disabled`)
  await evaluate('window.holdPin = true'); await key('Enter', 2); await until('window.finishPin')
  await evaluate('window.closeSearch()'); await until(`!document.querySelector('${modal}')`)
  await evaluate('window.finishPin(); window.holdPin = false')
  await until('(window.search.getState().tabs.project?.length ?? 0) === 5')
  assert.equal(await evaluate('Array.from(window.jobs.values()).filter(s=>s.pinned).length'), 5)

  // Small windows and both themes must retain a visible results area and source preview.
  for (const theme of ['light', 'dark']) for (const size of [11, 17]) {
    await evaluate(`document.documentElement.dataset.theme=${JSON.stringify(theme)}`)
    await evaluate(`document.documentElement.style.setProperty('--ui-scale', '${size / 13}'); window.searchUiSize(${size})`)
    await send('Emulation.setDeviceMetricsOverride', { width: 700, height: 700, deviceScaleFactor: 1, mobile: false })
    await key('f', 10); await until(`document.querySelector('${query}')`)
    await until('document.querySelector("[data-audit=searchEverywhere] [data-audit=searchPreview] .cm-content")')
    assert(await evaluate('document.querySelector("[data-audit=searchEverywhere] [data-audit=searchPreview]").getBoundingClientRect().height > 60'))
    assert(await evaluate('document.querySelector("[data-audit=searchEverywhere]").closest("[role=dialog]").getBoundingClientRect().bottom <= innerHeight'))
    writeFileSync(`/tmp/cide-search-${theme}-${size}.png`, Buffer.from((await send('Page.captureScreenshot', { format: 'png' })).data, 'base64'))
    // Tab and Shift+Tab stay inside the modal, including its close control and code preview.
    await click('[role="dialog"] [aria-label="Close"]')
    await until(`!document.querySelector('${modal}')`)
    await key('f', 10); await until(`document.querySelector('${query}')`)
    await until('document.querySelector("[data-audit=searchEverywhere] .cm-content")')
    await evaluate('document.querySelector("[role=dialog] [aria-label=Close]").focus()')
    await key('Tab', 8)
    assert.equal(await evaluate('document.activeElement.getAttribute("aria-label")'), 'Source preview')
    await key('Tab')
    assert.equal(await evaluate('document.activeElement.getAttribute("aria-label")'), 'Close')
    await key('Escape')
  }
  // Close an active search chooses its neighbor; explicitly selecting Log leaves the pins intact.
  await send('Emulation.setDeviceMetricsOverride', { width: 1400, height: 1000, deviceScaleFactor: 1, mobile: false })
  await chooseTab('Text: gamma')
  await click('[aria-label="Close Text: gamma"]')
  await until('(window.search.getState().tabs.project?.length ?? 0) === 4')
  assert(await evaluate('window.toolSelection.getState().byProject.project.startsWith("search:")'))
  await chooseTab('Log')
  await until('!document.querySelector("[data-audit=pinnedSearch]")')
  assert.equal((await pinned()).length, 4)
  await chooseTab('Text: transfer')
  while ((await pinned()).length) {
    const count = (await pinned()).length
    await evaluate('document.querySelector("[data-audit=toolWindowTab]:has([aria-selected=true]) [data-audit=toolWindowTabClose]").click()')
    await until(`(window.search.getState().tabs.project?.length ?? 0) === ${count - 1}`)
  }
  await until('!document.querySelector("[data-audit=pinnedSearch]")')
  assert.equal(await evaluate('document.querySelector("[data-audit=toolWindowTab] [aria-selected=true]").textContent'), 'Log')
  assert.equal(await evaluate('window.caret().path'), '/original.ts', 'closing pins releases their caret claims')

  // The same canonical file heading stays above long groups in the modal and pinned tab.
  await evaluate('document.documentElement.style.setProperty("--ui-scale", "1"); window.searchUiSize(13); window.layout("stacked")')
  await key('f', 10); await until(`document.querySelector('${query}')`)
  await replace('groups')
  await until('Object.values(window.search.getState().frames).some(f=>f.session.descriptor.query.pattern==="groups" && f.text?.hits.length===450)')
  const list = '[role="listbox"][aria-label="Search results"]'
  const fileHeading = '[data-heading="file"][title="groups.ts"]'
  const scrolled = async (offset, name = 'groups.ts') => {
    await evaluate(`document.querySelector('${list}').scrollTop = ${offset}`)
    await until(`document.querySelector('[data-sticky=true]')?.title === ${JSON.stringify(name)}`)
    // Wait for both the scroll event and the virtual range to settle.
    await until(`Math.abs(document.querySelector('${list}').scrollTop - ${offset}) < 1`)
  }
  await key('ArrowUp')
  for (const direction of ['ArrowLeft', 'ArrowLeft', 'ArrowRight', 'ArrowRight']) {
    await key(direction)
    assert.equal(await evaluate(`document.querySelector('${fileHeading}').getAttribute('aria-expanded')`), String(direction === 'ArrowRight'))
  }
  for (const layout of ['stacked', 'sideBySide']) {
    await evaluate(`window.layout(${JSON.stringify(layout)})`)
    await scrolled(2000)
    await until(`Math.abs(document.querySelector('[data-sticky=true]').getBoundingClientRect().top - document.querySelector('${list}').getBoundingClientRect().top) < 1`)
    assert.equal(await evaluate(`document.querySelectorAll('${fileHeading}').length`), 1, 'no duplicate heading or accessibility id')
    assert(await evaluate('document.querySelectorAll("[data-audit=searchResultRow]").length < 70'), 'sticky context preserves virtualization')
  }
  for (const size of [11, 17, 13]) {
    await evaluate(`document.documentElement.style.setProperty('--ui-scale', '${size / 13}'); window.searchUiSize(${size})`)
    await until(`document.querySelector('[data-sticky=true]')?.offsetHeight === ${Math.round(36 * size / 13)}`)
    await scrolled(2000)
    await until(`Math.abs(document.querySelector('[data-sticky=true]').getBoundingClientRect().top - document.querySelector('${list}').getBoundingClientRect().top) < 1`)
  }
  // Next heading pushes the current one away rather than overlapping it.
  const boundary = 36 + 230 * 30
  await scrolled(boundary - 18)
  await until(`Math.abs(document.querySelector('[data-sticky=true]').getBoundingClientRect().top - document.querySelector('${list}').getBoundingClientRect().top + 18) < 1`)
  await scrolled(boundary + 100, 'other.ts')
  await until(`Math.abs(document.querySelector('[data-sticky=true]').getBoundingClientRect().top - document.querySelector('${list}').getBoundingClientRect().top) < 1`)
  await scrolled(2000)
  await click(fileHeading) // A sticky heading is still clickable and keeps keyboard focus.
  assert(await focused('Search results'))
  assert.equal(await evaluate(`document.querySelector('${fileHeading}').getAttribute('aria-expanded')`), 'false')
  await key('ArrowRight')
  assert.equal(await evaluate(`document.querySelector('${fileHeading}').getAttribute('aria-expanded')`), 'true')
  await key('ArrowDown')
  await until(`document.querySelector('${modal} .cm-content')`)
  await key('PageDown'); await key('PageUp')
  await until(`(() => {
    const row = document.querySelector('${list} [aria-selected=true]')
    const heading = document.querySelector('[data-sticky=true]')
    return row && heading && row.getBoundingClientRect().top >= heading.getBoundingClientRect().bottom
  })()`)
  await scrolled(2000)
  writeFileSync('/tmp/cide-search-sticky-modal.png', Buffer.from((await send('Page.captureScreenshot', { format: 'png' })).data, 'base64'))
  await key('Enter', 2)
  await until(`!document.querySelector('${modal}') && document.querySelector('[data-audit=pinnedSearch]')`)
  await scrolled(2000)
  await until(`Math.abs(document.querySelector('[data-sticky=true]').getBoundingClientRect().top - document.querySelector('${list}').getBoundingClientRect().top) < 1`)
  writeFileSync('/tmp/cide-search-sticky-pinned.png', Buffer.from((await send('Page.captureScreenshot', { format: 'png' })).data, 'base64'))
  await click(fileHeading)
  await key('ArrowRight'); await key('ArrowRight')
  assert.equal(await evaluate(`document.querySelector('${fileHeading}').getAttribute('aria-expanded')`), 'true')
  await key('ArrowLeft'); await key('ArrowLeft')
  assert.equal(await evaluate(`document.querySelector('${fileHeading}').getAttribute('aria-expanded')`), 'false')
  await click('[aria-label="Close Text: groups"]')
  await until('(window.search.getState().tabs.project?.length ?? 0) === 0')
  assert.equal(errors.length, 0, JSON.stringify(errors))
  console.log('search browser: group keys, mode arrows, sticky file headings, modal layout, folder completion, paging, read-only bottom editor/navigation, previews, shortcuts, modifiers, complete pages, independent pins, refresh, hide/reopen, stale responses, focus, errors and responsive themes passed')
} finally {
  ws?.close()
  if (child && child.exitCode === null) {
    const exited = new Promise((resolve) => child.once('exit', resolve))
    child.kill();await exited
  }
  if (server) await new Promise((resolve) => server.httpServer.close(resolve))
  // Chromium's child processes can finish profile writes just after the parent exits.
  try { rmSync(dir, { recursive: true, force: true, maxRetries: 20, retryDelay: 100 }) } catch (error) { console.warn("Browser cache cleanup:", error.message) }
}
