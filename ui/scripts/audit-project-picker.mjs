/** Mounted browser checks for project search, focus and the actual command/key gate.
 * Run: pnpm --dir ui run audit:project-picker. Uses mocked IPC and headless Chromium.
 */
import assert from 'node:assert/strict'
import { build, preview } from 'vite'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { spawn } from 'node:child_process'

const ui = resolve(import.meta.dirname, '..')
const stripComments = (source) => source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '')
const defaults = stripComments(readFileSync(join(ui, '../crates/cide-core/src/keymap.rs'), 'utf8'))
const shortcut = /\("([^"]+)", "project\.open"\)/.exec(defaults)?.[1]
assert.equal(shortcut, 'ctrl+alt+o')
const app = stripComments(readFileSync(join(ui, 'src/App.tsx'), 'utf8'))
assert.equal(app.match(/overlay === 'projects' && <ProjectPicker onDismiss=\{closeOverlay\}/g)?.length, 2,
  'both window branches must mount the picker independently of an active project')
mkdirSync(join(ui, 'node_modules/.cache'), { recursive: true })
const dir = mkdtempSync(join(ui, 'node_modules/.cache/project-picker-'))
writeFileSync(join(dir, 'index.html'), '<html data-theme="light"><body><div id="root"></div><script type="module" src="./main.tsx"></script></body></html>')
writeFileSync(join(dir, 'main.tsx'), `
import React from 'react'; import {createRoot} from 'react-dom/client';
import {ProjectMenu} from '@/chrome/ProjectMenu';
import {ProjectPicker} from '@/overlays/ProjectPicker';
import {SearchEverywhere} from '@/search/SearchEverywhere';
import {useOverlays,closeOverlay} from '@/overlays/store';
import {useContextMenu} from '@/menus';
import {contextMenuOpen} from '@/menus/menuState';
import {projectMenu} from '@/ipc/client';
import {useWorkspace} from '@/store/workspace';
import {buildKeymap} from '@/keys/keymap';
import {installKeyGate} from '@/keys/gate';
import {createDispatcher} from '@/keys/dispatch';
import '@/styles/tokens.css';
const entry=(path,name,exists=true)=>({project:{path,displayPath:path.replace('/home/dev','~'),name,openedAt:0},exists});
window.entries=[entry('/home/dev/atlas','Atlas app'),entry('/mnt/atlas','Atlas archive'),entry('/gone','Gone',false)];
window.opened=[];window.forgotten=[];window.browsed=[];window.calls=0;window.fail=false;window.defer=false;
projectMenu.recent=async()=>{window.calls++;if(window.defer)return new Promise(resolve=>{window.finishLoad=()=>resolve(window.entries);});if(window.fail)throw new Error('load failed');return window.entries;};
projectMenu.reopen=async(path)=>{window.opened.push(path);return 'project';};
projectMenu.forget=async(path)=>{window.forgotten.push(path);window.entries=path===null?[]:window.entries.filter(e=>e.project.path!==path);return window.entries;};
projectMenu.browse=async()=>['/chosen'];
useWorkspace.setState({boot:null,openProject:async(paths)=>{window.browsed.push(paths);}});
const run=createDispatcher({fallback:(id)=>{throw new Error('unexpected command '+id)},toggleSidebar:()=>{}});
const bindings=[{key:${JSON.stringify(shortcut)},command:'project.open',when:null},{key:'ctrl+shift+p',command:'palette.commands',when:null}];
const commands=[{id:'project.open',title:'Open project…',group:'Project',when:null,unavailable:null,keywords:['recent']}];
installKeyGate({bindings:()=>bindings,context:()=>({overlayOpen:useOverlays.getState().open!==null,contextMenuOpen:contextMenuOpen()}),run});
window.run=run;window.close=closeOverlay;
function Fixture(){const overlay=useOverlays(s=>s.open);const ordinary=useContextMenu({label:'Ordinary menu',items:()=>[{id:'ordinary',label:'Ordinary action',run:()=>{window.ordinaryRan=true}}]});
return <><input id="behind" aria-label="Background editor"/><ProjectMenu/>
<button id="ordinary" onClick={e=>ordinary.openFor(e.currentTarget)}>Ordinary menu</button>{ordinary.menu}
{overlay==='projects'&&<ProjectPicker onDismiss={closeOverlay}/>}
{overlay==='search'&&<SearchEverywhere project={null} commands={commands} keymap={buildKeymap(bindings)} context={{}} onDismiss={closeOverlay} actions={{runCommand:id=>run(id,null),runCommandInNewSession:id=>run(id,null)}}/>}</>;}
createRoot(document.getElementById('root')).render(<Fixture/>);window.ready=true;
`)

let server, child, ws
try {
  await build({ configFile: false, root: dir, logLevel: 'error', resolve: { alias: { '@': join(ui, 'src') } }, build: { outDir: join(dir, 'dist') } })
  server = await preview({ configFile: false, root: dir, logLevel: 'error', build: { outDir: join(dir, 'dist') }, preview: { port: 0 } })
  const chromium = [process.env.CHROMIUM, '/usr/bin/chromium', '/usr/bin/chromium-browser', '/usr/bin/google-chrome'].find((bin) => bin && existsSync(bin))
  assert(chromium, 'Set CHROMIUM to a headless Chromium executable')
  child = spawn(chromium, ['--headless=new', '--remote-debugging-port=0', '--no-first-run', '--no-default-browser-check', '--window-size=820,520', '--user-data-dir='+join(dir,'profile'), 'about:blank'], { stdio: ['ignore','ignore','pipe'] })
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
    throw new Error('Timed out: '+expression)
  }
  const click = async (selector) => {
    await until('document.querySelector('+JSON.stringify(selector)+')')
    return evaluate('document.querySelector('+JSON.stringify(selector)+').click()')
  }
  const key = async (key, modifiers=0) => {
    const codes={ArrowDown:40,ArrowUp:38,ArrowLeft:37,ArrowRight:39,Enter:13,Escape:27,Home:36,End:35,Tab:9,o:79,p:80}
    const params={key,modifiers,windowsVirtualKeyCode:codes[key]??0}
    await send('Input.dispatchKeyEvent',{...params,type:'keyDown'})
    await send('Input.dispatchKeyEvent',{...params,type:'keyUp'})
  }
  const type = async (text) => { await send('Input.insertText',{text}) }
  const focused = (label) => evaluate('document.activeElement?.getAttribute("aria-label") === '+JSON.stringify(label))
  const clear = async () => { await evaluate('document.activeElement.select()');await type('') }
  const dropdownInput = '[aria-label="Filter recent projects"]'
  const modal = '[role="dialog"][aria-label="Open project"]'

  await send('Runtime.enable')
  await send('Page.navigate',{url:server.resolvedUrls.local[0]})
  await until('window.ready && document.querySelector("#behind")')
  for (const theme of ['light','dark']) {
    await evaluate('document.documentElement.dataset.theme='+JSON.stringify(theme))
    await click('[aria-label="Recent projects"]')
    await until('document.activeElement?.getAttribute("aria-label")==="Filter recent projects"')
    await type('ATLAS')
    await until('document.querySelectorAll("[data-item^=\\"recent:\\"]").length===2')
    await evaluate('document.querySelector("[data-item=\\"recent:/mnt/atlas\\"]").dispatchEvent(new PointerEvent("pointerover",{bubbles:true}))')
    assert(await focused('Filter recent projects'), 'hover must retain input focus')
    await clear();await type('Atlas app')
    await until('document.querySelectorAll("[data-item^=\\"recent:\\"]").length===1')
    await key('ArrowDown')
    assert(await focused('Filter recent projects'), 'navigation must retain input focus')
    await key('Home');await type(' ')
    assert.equal(await evaluate('document.activeElement.value'), ' Atlas app', 'spaces and Home edit the input')
    await key('ArrowLeft')
    assert(await focused('Filter recent projects'), 'Left edits text rather than dismissing')
    await clear();await type('absent')
    await until('document.querySelector("[data-item=no-matches]")')
    assert.equal(await evaluate('document.querySelector("[data-item=forget-missing]").textContent'), 'Remove 1 missing project')
    await key('Escape')
    assert.equal(await evaluate('document.querySelector("[data-audit=contextMenu]")'), null)
    await click('[aria-label="Recent projects"]')
    await until('document.querySelector('+JSON.stringify(dropdownInput)+')')
    assert.equal(await evaluate('document.querySelector('+JSON.stringify(dropdownInput)+').value'), '')
    await click('[data-item="recent:/mnt/atlas"]')
    await until('!document.querySelector("[data-audit=contextMenu]")')
    assert.equal(await evaluate('window.opened.at(-1)'), '/mnt/atlas')

    // A real key event reaches the actual dispatcher with a null workspace bootstrap.
    await evaluate('document.querySelector("#behind").focus()')
    await key('o', 3)
    await until('document.querySelector('+JSON.stringify(modal)+') && document.activeElement?.getAttribute("aria-label")==="Open project"')
    await type('atlas')
    await until('document.querySelectorAll("[data-project-path]").length===2')
    await key('ArrowDown')
    assert.equal(await evaluate('document.querySelector("[data-project-path][aria-selected=true]").dataset.projectPath'), '/mnt/atlas')
    assert(await focused('Open project'))
    await key('Enter')
    await until('!document.querySelector('+JSON.stringify(modal)+')')
    assert.equal(await evaluate('window.opened.at(-1)'), '/mnt/atlas')

    await key('p', 10)
    await until('document.querySelector("[role=dialog][aria-label=\\"Search Everywhere\\"]")')
    await type('Open project');await key('Enter')
    await until('document.querySelector('+JSON.stringify(modal)+')')
    assert(await focused('Open project'), 'palette transition must focus the project input')
    assert.equal(await evaluate('document.activeElement.value'), '')
    await type('Gone');await key('Enter')
    assert(await evaluate('!!document.querySelector('+JSON.stringify(modal)+')'), 'a missing directory cannot be accepted')
    assert.equal(await evaluate('document.querySelector("[data-project-path]").getAttribute("aria-disabled")'), 'true')
    await key('Escape')
  }

  await evaluate('window.defer=true;window.run("project.open",null)')
  await until('window.finishLoad')
  assert(await focused('Open project'), 'input focuses before a slow load finishes')
  await type('/mnt');await evaluate('window.finishLoad();window.defer=false')
  await until('document.querySelectorAll("[data-project-path]").length===1')
  assert(await focused('Open project'))
  await key('Escape')
  await evaluate('window.defer=true;window.run("project.open",null)')
  await until('document.querySelector('+JSON.stringify(modal)+')')
  await key('Escape');await evaluate('window.finishLoad();window.defer=false')
  assert.equal(await evaluate('document.querySelector('+JSON.stringify(modal)+')'), null, 'late load cannot reopen a dismissed modal')

  await evaluate('window.defer=true;window.finishLoad=null')
  await click('[aria-label="Recent projects"]')
  await until('window.finishLoad && document.activeElement?.getAttribute("aria-label")==="Filter recent projects"')
  assert(await evaluate('document.body.textContent.includes("Loading projects")'))
  await type('archive')
  await evaluate('window.finishLoad();window.defer=false')
  await until('document.querySelectorAll("[data-item^=\\"recent:\\"]").length===1')
  assert(await focused('Filter recent projects'), 'dropdown stays focused when a slow load finishes')
  // Replacing the dropdown must not leave it over the modal or restore focus to its button.
  await key('o',3)
  await until('document.querySelector('+JSON.stringify(modal)+') && !document.querySelector("[data-audit=contextMenu]")')
  assert(await focused('Open project'))
  await key('Escape')

  await evaluate('window.defer=true;window.finishLoad=null')
  await click('[aria-label="Recent projects"]')
  await until('window.finishLoad')
  await key('Escape');await evaluate('window.finishLoad();window.defer=false')
  assert.equal(await evaluate('document.querySelector("[data-audit=contextMenu]")'), null, 'late dropdown load cannot reopen a dismissed menu')

  await evaluate('window.fail=true;window.run("project.open",null)')
  await until('document.body.textContent.includes("Could not load projects")')
  await evaluate('window.fail=false;[...document.querySelectorAll("button")].find(b=>b.textContent==="Retry").click()')
  await until('document.querySelectorAll("[data-project-path]").length===3')
  await evaluate('document.querySelector("[aria-label=\\"Open project\\"] button").click()')
  await until('window.browsed.length===1')
  assert.deepEqual(await evaluate('window.browsed[0]'), ['/chosen'])

  await click('[aria-label="Recent projects"]');await until('document.querySelector('+JSON.stringify(dropdownInput)+')')
  await type('Atlas');await click('[data-item="forget-missing"]')
  await until('window.forgotten.length===1')
  assert.equal(await evaluate('window.forgotten[0]'), '/gone', 'cleanup includes hidden missing projects')
  await click('[aria-label="Recent projects"]');await until('document.querySelector('+JSON.stringify(dropdownInput)+')')
  await type('no matches');await click('[data-item="clear"]')
  await until('window.entries.length===0')
  assert.equal(await evaluate('window.forgotten.at(-1)'), null)
  await key('o',3);await until('document.body.textContent.includes("No recent projects")');await key('Escape')

  await evaluate('window.entries=Array.from({length:100},(_,i)=>({project:{path:"/project"+i,displayPath:"/project"+i,name:"Project "+i,openedAt:0},exists:true}))')
  await click('[aria-label="Recent projects"]');await until('document.querySelectorAll("[data-item^=\\"recent:\\"]").length===100')
  await evaluate('document.querySelector("[data-item=\\"recent:/project99\\"]").dispatchEvent(new PointerEvent("pointerover",{bubbles:true}))')
  await until('document.querySelector("[data-audit=contextMenu]").scrollTop>0')
  assert(await focused('Filter recent projects'), 'scrolling a long menu retains input focus')
  assert(await evaluate('(()=>{const input=document.querySelector('+JSON.stringify(dropdownInput)+').getBoundingClientRect();const menu=document.querySelector("[data-audit=contextMenu]").getBoundingClientRect();return input.top>=menu.top && input.bottom<=menu.bottom})()'), 'search stays visible while scrolling a hundred projects')
  await key('Escape')
  await key('o',3);await until('document.querySelectorAll("[data-project-path]").length===100')
  await key('End')
  assert.equal(await evaluate('document.querySelector("[data-project-path][aria-selected=true]").dataset.projectPath'), '/project99')
  assert(await focused('Open project'))
  const point = await evaluate('(()=>{const rect=document.querySelector("[data-project-path=\\"/project99\\"]").getBoundingClientRect();return {x:rect.left+8,y:rect.top+8}})()')
  await send('Input.dispatchMouseEvent',{type:'mousePressed',button:'left',clickCount:1,...point})
  await send('Input.dispatchMouseEvent',{type:'mouseReleased',button:'left',clickCount:1,...point})
  await until('!document.querySelector('+JSON.stringify(modal)+')')
  assert.equal(await evaluate('window.opened.at(-1)'), '/project99', 'click opens the row under the pointer')

  // The opt-in input must not change ordinary context-menu focus or keyboard activation.
  await click('#ordinary');await until('document.activeElement?.getAttribute("role")==="menu"')
  await key('ArrowDown');await key('Enter');await until('window.ordinaryRan')
  assert.equal(errors.length, 0, JSON.stringify(errors))
  console.log('project picker: focus, filtering, command/shortcut, async loading, cleanup and ordinary menu checks passed')
} finally {
  ws?.close()
  if (child && child.exitCode === null) {
    const exited = new Promise((resolve) => child.once('exit', resolve))
    child.kill();await exited
  }
  if (server) await new Promise((resolve) => server.httpServer.close(resolve))
  // Chromium's child processes can finish profile writes just after the parent exits.
  rmSync(dir, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 })
}
