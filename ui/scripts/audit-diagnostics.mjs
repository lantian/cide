/** Mounted diagnostics controls and bottom tabs with deterministic IPC; requires headless Chromium. */
import assert from 'node:assert/strict'
import { build, preview } from 'vite'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { spawn } from 'node:child_process'
const ui = resolve(import.meta.dirname, '..')
mkdirSync(join(ui, 'node_modules/.cache'), { recursive: true })
const dir = mkdtempSync(join(ui, 'node_modules/.cache/diagnostics-audit-'))
writeFileSync(join(dir, 'index.html'), '<html data-theme="light"><body style="margin:0"><div id="root"></div><script type="module" src="./main.tsx"></script></body></html>')
writeFileSync(join(dir, 'main.tsx'), readFileSync(join(ui, 'scripts/diagnostics-fixture.tsx')))
let server, child, ws
try {
  await build({ configFile: false, root: dir, logLevel: 'error', resolve: { alias: { '@': join(ui, 'src') } }, build: { outDir: join(dir, 'dist') } })
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
    throw new Error('Timed out: '+expression+'\nBrowser errors: '+JSON.stringify(errors))
  }
  const click = async (selector) => {
    await until('document.querySelector('+JSON.stringify(selector)+')')
    return evaluate('document.querySelector('+JSON.stringify(selector)+').click()')
  }
  const key = async (key, modifiers=0) => {
    const codes={ArrowDown:40,ArrowUp:38,ArrowLeft:37,ArrowRight:39,Enter:13,Escape:27,Home:36,End:35,Tab:9,o:79,p:80,f:70,n:78,Shift:16,m:77,c:67,F4:115}
    const params={key,modifiers,windowsVirtualKeyCode:codes[key]??0}
    await send('Input.dispatchKeyEvent',{...params,type:'keyDown'})
    await send('Input.dispatchKeyEvent',{...params,type:'keyUp'})
  }
  const type = async (text) => { await send('Input.insertText',{text}) }
  const focused = (label) => evaluate('document.activeElement?.getAttribute("aria-label") === '+JSON.stringify(label))
  const clear = async () => { await evaluate('document.activeElement.select()');await type('') }

  await send('Runtime.enable')
  await send('Page.navigate', { url: server.resolvedUrls.local[0] })
  await until(`window.ready && document.querySelector('[aria-label="Language Servers"]')`)
  const status = '[data-audit="diagnosticsStatus"]'
  const problems = '[data-audit="problemsToolWindow"]'
  assert.equal(await evaluate(`!!document.querySelector('[data-view="gitlab"]')`), false)
  await evaluate('window.connect(true)')
  await until(`document.querySelector('[aria-label="GitLab merge requests"]')`)
  await evaluate('window.connect(false)')
  await until(`!document.querySelector('[aria-label="GitLab merge requests"]')`)
  assert.equal(await evaluate(`document.querySelector('${status}').textContent.trim()`), '12', 'project error/warning counts')
  const divider = '[data-audit="diagnosticsNavigationDivider"]'
  await until(`document.querySelector('${divider}')`)
  await evaluate('window.setEditorFocused(false)')
  await until(`!document.querySelector('${divider}')`)
  await evaluate('window.setFile(null); window.setEditorFocused(true)')
  await until(`!document.querySelector('[data-audit="statusBar"]').textContent.includes('main.rs')`)
  assert.equal(await evaluate(`!!document.querySelector('${divider}')`), false, 'no divider without a breadcrumb claim')
  await evaluate('window.setFile("/project/src/main.rs")')
  await until(`document.querySelector('${divider}')`)
  await evaluate(`window.layoutDelay = 150; window.logMounted = false; new MutationObserver(records => {
    for (const record of records) for (const node of record.addedNodes) {
      if (node instanceof Element && (node.matches('[data-audit="log"]') || node.querySelector('[data-audit="log"]'))) window.logMounted = true
    }
  }).observe(document.getElementById('root'), { childList: true, subtree: true })`)
  await key('m', 2|8)
  await until(`document.querySelector('${problems}')`)
  assert.equal(await evaluate('window.logMounted'), false, 'opening Problems must not mount the Git log while waiting for the layout acknowledgement')
  assert.equal(await evaluate(`document.querySelectorAll('${problems} [aria-level="2"]').length`), 3)
  await click(`${problems} [aria-level="2"]`)
  await key('F4')
  await until('window.opened.length === 1')
  assert.equal(await evaluate('window.sidebarToggled === true'), false, 'F4 does not toggle the sidebar')
  await key('c', 2)
  await until('window.copied.includes("unused variable")')
  await evaluate(`Array.from(document.querySelectorAll('${problems} [role="tab"]')).find(el=>el.textContent==='Current File').click()`)
  await until(`document.querySelectorAll('${problems} [aria-level="2"]').length === 2`)
  assert.equal(await evaluate(`document.querySelector('${problems} [aria-level="2"]').textContent.includes('unused import')`), true, 'position sorting')
  await evaluate(`const el=document.querySelector('[aria-label="Sort problems"]');el.value='severity';el.dispatchEvent(new Event('change',{bubbles:true}))`)
  await until(`document.querySelector('${problems} [aria-level="2"]').textContent.includes('expected i32')`)
  await click('[aria-label="Show error problems"]')
  await until(`document.querySelectorAll('${problems} [aria-level="2"]').length === 1`)
  assert.equal(await evaluate(`document.querySelector('${status}').textContent.trim()`), '12', 'view filtering does not change project totals')
  await evaluate('window.setFile(null)')
  await until(`document.querySelector('${problems}').textContent.includes('No current file')`)
  await evaluate('window.setFile("/project/src/main.rs")')
  await click('[aria-label="Show error problems"]')
  await click('[aria-label="Language Servers"]')
  await until(`document.querySelector('[data-item="rustAnalyzer"]')`)
  await click('[data-item="rustAnalyzer"]')
  await until(`document.body.textContent.includes('512 MiB')`)
  await evaluate('window.servers[0].memoryBytes = 1024 ** 3')
  await until(`document.body.textContent.includes('1.0 GiB')`)
  await click('[data-item="stop"]')
  await until('window.actions.some(a=>a[2]==="stop")')
  await click('[aria-label="Language Servers"]')
  await click('[data-item="rustAnalyzer"]')
  await until(`document.querySelector('[data-item="stop"]').getAttribute('aria-disabled') === 'true'`)
  await click('[data-item="restart"]')
  await until('window.actions.some(a=>a[2]==="restart")')
  await click('[aria-label="Language Servers"]')
  await click('[data-item="rustAnalyzer"]')
  await click('[data-item="logs"]')
  await until(`document.querySelector('[aria-label="rust-analyzer logs"]')?.textContent.includes('Initialization completed')`)
  await click('[aria-label="Close rust-analyzer Logs"]')
  await until(`document.querySelector('${problems}')`)
  await evaluate('new Promise(resolve => setTimeout(resolve, 200))')
  assert.equal(await evaluate(`!!document.querySelector('${problems}')`), true, 'late layout acknowledgements do not steal the newer tab selection')
  assert.equal(await evaluate('window.logMounted'), false, 'opening and closing server logs does not mount the Git log')
  await evaluate(`Array.from(document.querySelectorAll('${problems} [role="tab"]')).find(el=>el.textContent==='Project').click()`)
  await evaluate('window.setSnapshot({...window.snapshot, items: []})')
  await until(`document.querySelector('${problems}').textContent.includes('No problems found')`)
  assert.equal(await evaluate(`document.querySelector('${status}').textContent.trim()`), '')
  await evaluate('window.setSnapshot({...window.snapshot, kind: "scanning", items: []})')
  await until(`document.querySelector('${problems}').textContent.includes('Analysis in progress')`)
  await evaluate('window.setSnapshot(window.snapshot)')
  await click('[aria-label="Language Servers"]')
  await click('[data-item="rustAnalyzer"]')
  for (const theme of ['light', 'dark']) {
    await evaluate(`document.documentElement.dataset.theme=${JSON.stringify(theme)}`)
    const image = await send('Page.captureScreenshot', { format: 'png' })
    writeFileSync('/tmp/cide-diagnostics-'+theme+'.png', Buffer.from(image.data,'base64'))
  }
  const geometry = await evaluate(`Array.from(document.querySelectorAll('[role="menu"]')).map(el=>{const r=el.getBoundingClientRect();return {left:r.left,top:r.top,right:r.right,bottom:r.bottom}})`)
  assert(geometry.every(r=>r.left>=0 && r.top>=0 && r.bottom<=1000), 'menus fit above status bar')
  await key('Escape')
  await send('Emulation.setDeviceMetricsOverride',{width:720,height:520,deviceScaleFactor:1,mobile:false})
  await evaluate('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))')
  await until(`!document.querySelector('[role="menu"]')`)
  await click('[aria-label="Language Servers"]')
  await click('[data-item="rustAnalyzer"]')
  assert(await evaluate(`Array.from(document.querySelectorAll('[role="menu"]')).every(el=>{const r=el.getBoundingClientRect();return r.right<=innerWidth && r.bottom<=innerHeight && r.top>=0 && r.left>=0})`), 'narrow-window menus stay visible')
  await key('Escape')
  await until(`!document.querySelector('[role="menu"]')`)
  await click('[aria-label="Language Servers"]')
  await click('[data-item="stop-all"]')
  await until('window.actions.some(a=>a[1] === null && a[2] === "stop")')
  await click('[aria-label="Language Servers"]')
  await click('[data-item="restart-all"]')
  await until('window.actions.some(a=>a[1] === null && a[2] === "restart")')
  await evaluate(`Array.from(document.querySelectorAll('${problems} [role="tab"]')).find(el=>el.textContent==='Current File').click()`)
  await evaluate('window.holdControl = true')
  await click('[aria-label="Language Servers"]')
  await click('[data-item="restart-all"]')
  await until('window.finishControl')
  await evaluate('window.setProject("second"); window.open("second")')
  await until(`document.querySelector('${problems} [aria-selected="true"]').textContent === 'Project'`)
  await click('[aria-label="Language Servers"]')
  await until(`document.querySelector('[data-item="restart-all"]').getAttribute('aria-disabled') !== 'true'`)
  await evaluate('window.finishControl()')
  await until(`document.querySelector('[data-item="rustAnalyzer"]')`)
  await key('Escape')
  await evaluate('window.setProject("project")')
  await until(`document.querySelector('${problems} [aria-selected="true"]').textContent === 'Current File'`)

  // A full wire snapshot, with one file group per finding: DOM and keyboard work must stay
  // bounded, and unrelated shell/search updates must not rebuild this tree.
  await evaluate(`Array.from(document.querySelectorAll('${problems} [role="tab"]')).find(el=>el.textContent==='Project').click()`)
  await evaluate(`window.messageReads = 0; window.large = { ...window.snapshot, items: Array.from({ length: 1000 }, (_, index) => ({
    path: 'src/file'+String(index).padStart(4,'0')+'.rs', absPath: '/project/src/file'+String(index).padStart(4,'0')+'.rs', line: index+1, column: 1, severity: 'warning', source: 'rust-analyzer',
    get message() { window.messageReads++; return 'Problem '+index }
  })) }; window.setSnapshot(window.large)`)
  await until(`document.querySelector('${problems}').textContent.includes('1000 problems')`)
  const mounted = await evaluate(`document.querySelectorAll('${problems} [role="treeitem"]').length`)
  assert(mounted > 0 && mounted < 100, 'a full snapshot mounts only visible rows: '+mounted)
  await evaluate('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))')
  const reads = await evaluate('window.messageReads')
  await evaluate('window.setNoise(1)')
  await until(`document.querySelector('[data-noise="1"]')`)
  assert.equal(await evaluate('window.messageReads'), reads, 'unrelated shell updates do not rebuild Problems')
  await evaluate(`document.querySelector('${problems} [role="tree"]').focus()`)
  await key('End')
  await until(`document.querySelector('${problems} [aria-selected="true"][role="treeitem"]')?.textContent.includes('Problem 999')`)
  await key('F4')
  assert.equal(await evaluate('window.opened.at(-1)[0]'), '/project/src/file0999.rs', 'keyboard navigation reaches unmounted rows')
  await key('c', 2)
  await until('window.copied.includes("Problem 999")')
  await key('Home')
  await key('ArrowDown')
  await key('ArrowDown', 8)
  await key('ArrowDown', 8)
  await key('c', 2)
  await until('window.copied.includes("Problem 0") && window.copied.includes("Problem 1")')
  await click('[aria-label="Show warning problems"]')
  await until(`document.querySelector('${problems}').textContent.includes('No problems match this view')`)
  await click('[aria-label="Show warning problems"]')
  await until(`document.querySelectorAll('${problems} [role="treeitem"]').length > 0`)
  assert.equal(await evaluate(`!!document.getElementById(document.querySelector('${problems} [role="tree"]').getAttribute('aria-activedescendant'))`), true, 'active descendant names a mounted row')

  assert.equal(errors.length, 0, JSON.stringify(errors))
  console.log('diagnostics browser audit: ok (opening race, large snapshots, navigation dividers, keyboard, filters, server controls, logs, themes and narrow viewport)')
} finally {
  ws?.close(); child?.kill(); await server?.httpServer.close()
  await new Promise(resolve=>setTimeout(resolve,150))
  rmSync(dir,{recursive:true,force:true})
}
