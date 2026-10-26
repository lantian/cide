/** Real mounted clipboard comparison with deterministic IPC; requires headless Chromium. */
import assert from 'node:assert/strict'
import { build, preview } from 'vite'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { spawn } from 'node:child_process'
const ui = resolve(import.meta.dirname, '..')
mkdirSync(join(ui, 'node_modules/.cache'), { recursive: true })
const dir = mkdtempSync(join(ui, 'node_modules/.cache/clipboard-audit-'))
writeFileSync(join(dir, 'index.html'), '<html data-theme="light"><body style="margin:0"><div id="root"></div><script type="module" src="./main.tsx"></script></body></html>')
writeFileSync(join(dir, 'main.tsx'), readFileSync(join(ui, 'scripts/clipboard-diff-fixture.tsx')))
let server, child, ws
try {
  await build({ configFile: false, root: dir, publicDir: join(ui, 'public'), logLevel: 'error', resolve: { alias: { '@': join(ui, 'src') } }, build: { minify: false, outDir: join(dir, 'dist') } })
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
    const state = await evaluate('document.body.innerText.slice(0,2000)')
    throw new Error('Timed out: '+expression+'\nBrowser errors: '+JSON.stringify(errors)+'\nPage: '+state)
  }
  const key = async (key, modifiers=0) => {
    const codes={z:90}
    const params={key,modifiers,windowsVirtualKeyCode:codes[key]??0}
    await send('Input.dispatchKeyEvent',{...params,type:'keyDown'})
    await send('Input.dispatchKeyEvent',{...params,type:'keyUp'})
  }
  const url = server.resolvedUrls.local[0]
  await send('Runtime.enable')
  await send('Page.navigate', { url })
  await until("document.querySelector('#original .cm-editor')")
  await evaluate("window.fileView().dispatch({changes:{from:0,insert:'// unsaved\\n'}})")
  await evaluate("document.querySelector('#original .cm-content').dispatchEvent(new MouseEvent('contextmenu',{bubbles:true,clientX:120,clientY:80}))")
  await until("[...document.querySelectorAll('[role=menuitem]')].some(el=>el.textContent.includes('Compare to Clipboard'))")
  await evaluate("[...document.querySelectorAll('[role=menuitem]')].find(el=>el.textContent.includes('Compare to Clipboard')).click()")
  await until("document.querySelectorAll('#comparison .cm-editor').length===2")
  await until("document.querySelector('#comparison .cm-merge-b .cm-content')?.textContent.includes('unsaved')")
  assert.equal(await evaluate("window.model().dirty"), true)
  await until("document.querySelector('#comparison button[aria-label=\"Copy from Clipboard\"]')")
  const geometry = await evaluate(`(() => {
    const host=document.querySelector('#comparison'), editors=[...host.querySelectorAll('.cm-editor')];
    const bounds=host.getBoundingClientRect();
    return editors.map(el=>({width:el.getBoundingClientRect().width, height:el.getBoundingClientRect().height, max:bounds.bottom}));
  })()`)
  assert(geometry.every(x=>x.width>200 && x.height>400), JSON.stringify(geometry))
  await evaluate("document.querySelector('#comparison button[aria-label=\"Copy from Clipboard\"]').click()")
  await until("window.fileView().state.doc.toString()===window.model().state.doc.toString()")
  await evaluate('window.fileView().focus()')
  await key('z',2)
  await until("window.fileView().state.doc.toString().startsWith('// unsaved')")
  await evaluate("[...document.querySelectorAll('#comparison button')].find(el=>el.textContent==='Unified').click()")
  await until("document.querySelectorAll('#comparison .cm-editor').length===1")
  assert.equal(await evaluate("window.fileView().state.doc.toString().startsWith('// unsaved')"), true)
  await evaluate('window.refreshClipboard()')
  await until("document.querySelector('#comparison').textContent.includes('Hi')")
  await evaluate("[...document.querySelectorAll('#comparison button')].find(el=>el.textContent==='Save').click()")
  await until('!window.model().dirty')
  assert.equal(await evaluate('window.saved()===window.fileView().state.doc.toString()'),true)
  await evaluate("[...document.querySelectorAll('#comparison button')].find(el=>el.textContent==='Split').click()")
  await until("document.querySelectorAll('#comparison .cm-editor').length===2")
  for(const theme of ['light','dark']) {
    await evaluate('window.theme('+JSON.stringify(theme)+')')
    await new Promise(r=>setTimeout(r,120))
    const shot=await send('Page.captureScreenshot',{format:'png'})
    writeFileSync('/tmp/cide-clipboard-'+theme+'.png',Buffer.from(shot.data,'base64'))
  }
  assert.deepEqual(errors,[])
  console.log('clipboard browser audit: context menu, unsaved text, hunk copy, shared Undo/Save, refresh and layouts passed')
} finally {
  ws?.close()
  if (child && child.exitCode === null) {
    const exited = new Promise(resolve => child.once('exit', resolve))
    child.kill(); await exited
  }
  await server?.httpServer.close()
  rmSync(dir,{recursive:true,force:true,maxRetries:5,retryDelay:100})
}
