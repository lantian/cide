/** Optional real-browser layout check. Run npm run bench:openspec-browser in ui.
 * Requires Chromium (override its path with CHROMIUM); uses only a temporary browser profile
 * and a localhost fixture. The regular check:openspec-performance suite runs without a browser.
 */
import { createServer } from 'vite'
import { spawn } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { resolve } from 'node:path'
const ui = resolve(import.meta.dirname, '..')
const profile = mkdtempSync(`${tmpdir()}/openspec-chromium-`)
const html = `<!doctype html><html><body><div id="root" style="width:320px;height:780px"></div><script type="module">
import React from 'react';
import { createRoot } from 'react-dom/client';
import { flushSync } from 'react-dom';
import { OpenSpecPanelView } from '/src/sidebar/OpenSpecPanel/OpenSpecPanel.tsx';
import { SPEC_STORIES } from '/src/sidebar/OpenSpecPanel/fixture.ts';
import '/src/styles/tokens.css';
import '/src/icons/icon.css';
const result = {};
const wait = () => new Promise(r => setTimeout(r, 100));
const assert = (condition, text) => { if (!condition) throw Error(text) };
try {
const root = createRoot(document.getElementById('root'));
const board = { kind: 'ready', root:'/repo', commands:[], changes: Array.from({length:5000},(_,i)=>({name:'change-'+String(i).padStart(4,'0')+'-a-long-label-that-wraps',completed:1,total:2,status:''})), specs: Array.from({length:5000},(_,i)=>({id:'spec-'+String(i).padStart(4,'0'), requirements:5})) };
root.render(React.createElement(OpenSpecPanelView,{...SPEC_STORIES.board,board}));
await wait();
const scroller = document.querySelector('[data-audit="virtualList"]');
assert(scroller && scroller.clientHeight > 300,'list must receive a scroll viewport');
result.viewport = [scroller.clientWidth,scroller.clientHeight];
result.mounted = document.querySelectorAll('[data-index]').length;
assert(result.mounted > 0 && result.mounted < 100,'rows must be bounded');
flushSync(() => { scroller.scrollTop = scroller.scrollHeight; scroller.dispatchEvent(new Event('scroll')); });
await wait(); await wait();
result.lastSpecVisible = !!document.querySelector('[data-spec="spec-4999"]');
assert(document.querySelector('[data-spec="spec-4999"]'),'last spec must be reachable');
document.documentElement.style.setProperty('--w-sidebar-agents','220px');
await wait();
flushSync(() => { scroller.scrollTop=0; scroller.dispatchEvent(new Event('scroll')); });
await wait(); await wait();
const measured = [...document.querySelectorAll('[data-index]')].map(n=>n.getBoundingClientRect());
assert(measured.every((r,i)=>i===0 || r.top>=measured[i-1].bottom-1),'measured rows must not overlap after resizing');
result.resized = scroller.clientWidth;
result.ok=true;
} catch(error) {result.error=String(error.stack)}
document.body.innerHTML='<pre id="result"></pre>';
document.getElementById('result').textContent=JSON.stringify(result);
</script></body></html>`
const server = await createServer({ root: ui, server: { host: '127.0.0.1', port: 0 }, plugins: [{ name: 'openspec-browser-fixture', configureServer(server) { server.middlewares.use('/openspec-browser-check', async (_req, res) => { res.setHeader('Content-Type', 'text/html'); res.end(await server.transformIndexHtml('/openspec-browser-check', html)) }) } }] })
await server.listen().catch(async (error) => { await server.close(); rmSync(profile,{recursive:true,force:true}); throw error })
const address = server.httpServer.address()
let child;
try {
 const output = await new Promise((resolve, reject) => {
  child = spawn(process.env.CHROMIUM ?? '/usr/bin/chromium', ['--headless=new', '--disable-gpu', '--no-first-run', '--no-default-browser-check', `--user-data-dir=${profile}`, '--dump-dom', '--virtual-time-budget=15000', `http://127.0.0.1:${address.port}/openspec-browser-check`]);
  let out='',err='';
  child.stdout.on('data',d=>out+=d); child.stderr.on('data',d=>err+=d);
  const timeout=setTimeout(()=>{child.kill();reject(Error('browser timed out: '+err.slice(-1500)))},45000);
  child.on('exit',code=>{clearTimeout(timeout);code===0?resolve(out):reject(Error(err.slice(-1500)))})
 });
 const result=output.match(/<pre id="result">(.*?)<\/pre>/s)?.[1];
 console.log(result ?? 'Browser produced no fixture result');
 if(!result || !JSON.parse(result.replaceAll('&quot;','"').replaceAll('&amp;','&')).ok) process.exitCode=1;
} finally {child?.kill();await server.close();rmSync(profile,{recursive:true,force:true})}
