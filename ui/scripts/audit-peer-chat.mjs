/** Browser audit of the paired terminals. Requires Chromium; no harness or paid turn runs.
 * Unlike check:peer-chat's picker fixtures, this mounts real TerminalPane, PaneSlot and
 * xterm with PaneFrame's actual CSS. Only Tauri is faked, at the IPC boundary.
 */
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { existsSync, mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { createServer } from 'vite'
import react from '@vitejs/plugin-react'

const ui = resolve(import.meta.dirname, '..')
mkdirSync(join(ui, 'node_modules/.cache'), { recursive: true })
const cache = mkdtempSync(join(ui, 'node_modules/.cache/peer-browser-'))
const binary = [process.env.CHROMIUM, '/usr/bin/chromium', '/usr/bin/google-chrome'].find(p => p && existsSync(p))
assert(binary, 'Set CHROMIUM to a Chromium executable')
writeFileSync(join(cache, 'index.html'), '<!doctype html><div id="root"></div><script type="module" src="./fixture.tsx"></script>')
writeFileSync(join(cache, 'fixture.tsx'), `
import React from 'react'
import { createRoot } from 'react-dom/client'
import { PeerChatPane } from '@/panes/PeerChatPane'
import { peekHost } from '@/layout/paneHosts'
import { installFakeTauri } from '@/demo/fakeTauri'
import frame from '@/layout/PaneTitleBar.module.css'
import '@/styles/tokens.css'
import '@/styles/fonts.css'
window.calls = []
const handlers = new Map([
  ['peer_chat_plan', () => []], ['peer_chat_receipts', () => []],
  ['session_exit', () => ({kind:'running'})],
  ['session_in_alternate_screen', () => false],
  ['session_attach', args => { window.calls.push({command:'attach', ...args}); return new TextEncoder().encode(args.pane + ' session ready\\r\\n> ').buffer }],
  ['session_write', args => { window.calls.push({command:'write', ...args}) }],
  ['session_resize', args => { window.calls.push({command:'resize', ...args}) }],
  ['session_spawn', () => { throw new Error('Configured live panes must adopt their sessions') }],
])
installFakeTauri('shell:audit', handlers, () => null)
const chat = {main:'main', peer:'peer', mainHarness:'codex', selection:{harness:'opencode', model:null}, paused:false, starting:false}
const tab = {id:'pair', peerChat:chat}
window.host = id => peekHost(id)
window.show = (paused=false) => createRootOnce.render(<div style={{height:'100vh', display:'flex'}}>{['main','peer'].map(id => <div key={id} data-panel={id} className={frame.frame} style={{flex:1}}><div className={frame.body}><PeerChatPane project="audit" cwd="/tmp" pane={{id,kind:'claude',session:id+'-session',role:'auxiliary'}} tab={{...tab,peerChat:{...chat,paused}}}/></div></div>)}</div>)
const createRootOnce = createRoot(document.getElementById('root'))
document.body.style.margin = '0'
window.show()
`)

let server, child, socket
try {
  server = await createServer({ configFile: false, root: cache, logLevel: 'error',
    resolve: { alias: { '@': join(ui, 'src') } }, plugins: [react()],
    server: { host: '127.0.0.1', port: 0, fs: { allow: [ui] } },
  })
  await server.listen()
  child = spawn(binary, ['--headless=new', '--remote-debugging-port=0', `--user-data-dir=${join(cache, 'profile')}`, '--no-first-run', '--no-default-browser-check', 'about:blank'], {stdio:['ignore','ignore','pipe']})
  const port = await new Promise((ok, fail) => {
    let err = ''
    const timer = setTimeout(() => fail(new Error('Chromium startup timed out')), 20000)
    child.stderr.on('data', bytes => { err += bytes; const match = err.match(/DevTools listening on ws:\/\/[^:]+:(\d+)\//); if (match) {clearTimeout(timer); ok(Number(match[1]))} })
    child.once('exit', code => {clearTimeout(timer); fail(new Error(`Chromium exited ${code}: ${err}`))})
  })
  const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()
  socket = new WebSocket(targets.find(t => t.type === 'page').webSocketDebuggerUrl)
  await new Promise((ok, fail) => {socket.onopen=ok; socket.onerror=fail})
  let sequence = 0
  const pending = new Map(), errors = []
  socket.onmessage = message => {
    const data = JSON.parse(message.data)
    if (data.id) { const entry = pending.get(data.id); pending.delete(data.id); if (data.error) entry.reject(data.error); else entry.resolve(data.result) }
    if (data.method === 'Runtime.exceptionThrown') errors.push(data.params.exceptionDetails.exception?.description ?? data.params.exceptionDetails.text)
  }
  const send = (method, params={}) => new Promise((resolve, reject) => {const id=++sequence; pending.set(id,{resolve,reject}); socket.send(JSON.stringify({id,method,params}))})
  const evaluate = async expression => {
    const answer = await send('Runtime.evaluate', {expression,returnByValue:true,awaitPromise:true})
    assert(!answer.exceptionDetails, JSON.stringify(answer.exceptionDetails))
    return answer.result.value
  }
  await send('Runtime.enable')
  await send('Page.navigate', {url:server.resolvedUrls.local[0]})
  const deadline = Date.now()+20000
  while (!await evaluate(`['main','peer'].every(id=>window.host?.(id)?.hydrated)`)) {
    assert(Date.now()<deadline, 'Terminals never attached: '+errors.join('\n'))
    await new Promise(resolve => setTimeout(resolve,100))
  }
  // Reproduce the reported failure in this isolated page: a flex child of a block
  // body has no inherited height. This also proves the geometry assertion below
  // would reject the old layout; JSDOM/picker mocks cannot observe this collapse.
  const collapsed = await evaluate(`['main','peer'].map(id=>{const host=window.host(id); const pane=host.el.parentElement.parentElement.parentElement; pane.style.height='auto'; return host.el.getBoundingClientRect().height})`)
  assert(collapsed.every(height => height === 0), 'Could not reproduce the original collapsed layout')
  await evaluate(`['main','peer'].forEach(id=>window.host(id).el.parentElement.parentElement.parentElement.style.removeProperty('height'))`)
  // Match the reported two-panel layout, then resize and include the pause banner.
  for (const [width,height,paused] of [[2048,917,false],[900,600,false],[1200,800,true]]) {
    await send('Emulation.setDeviceMetricsOverride', {width,height,deviceScaleFactor:1,mobile:false})
    await evaluate(`window.show(${paused})`)
    await new Promise(resolve => setTimeout(resolve,200))
    const panels = await evaluate(`['main','peer'].map(id=>{const host=window.host(id); const box=host.el.getBoundingClientRect(); const frame=document.querySelector('[data-panel="'+id+'"]').getBoundingClientRect(); return {id,height:box.height,width:box.width,bottom:box.bottom,frameBottom:frame.bottom,cols:host.terminal.term.cols,rows:host.terminal.term.rows,text:host.terminal.term.buffer.active.getLine(0).translateToString(true)}})`)
    for (const panel of panels) {
      assert(panel.height>height/2 && panel.width>width/3, `Terminal collapsed: ${JSON.stringify(panel)}`)
      assert(panel.bottom<=panel.frameBottom+1, `Terminal escapes its panel: ${JSON.stringify(panel)}`)
      assert(panel.cols>=20 && panel.rows>=5, `Fallback geometry: ${JSON.stringify(panel)}`)
      assert.equal(panel.text, `${panel.id} session ready`)
    }
  }
  // A browser input event goes through xterm's textarea and the real session sink.
  await evaluate(`window.host('main').terminal.term.focus()`)
  await send('Input.insertText', {text:'Review this task'})
  await new Promise(resolve => setTimeout(resolve,100))
  const writes = await evaluate(`window.calls.filter(c=>c.command==='write')`)
  assert.equal(writes.map(c=>c.data).join(''), 'Review this task')
  assert(writes.every(c=>c.session==='main-session'), 'Input reached the wrong session')
  assert.equal(await evaluate(`window.calls.filter(c=>c.command==='attach').length`), 2, 'Resize/render reattached terminals')
  assert.deepEqual(errors, [])
  console.log('peer chat browser: both real terminals visible, bounded and fitted; live sessions adopted; left keyboard input delivered')
} finally {
  socket?.close()
  if (child && child.exitCode === null) {const exited = new Promise(resolve => child.once('exit',resolve)); child.kill(); await exited}
  await server?.close()
  rmSync(cache, {recursive:true,force:true,maxRetries:5,retryDelay:200})
}
