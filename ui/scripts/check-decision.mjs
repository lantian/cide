import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { build } from 'vite'
import { JSDOM } from 'jsdom'
import React, { act } from 'react'
const ui=resolve(import.meta.dirname,'..')
mkdirSync(join(ui,'node_modules/.cache'),{recursive:true})
const out=mkdtempSync(join(ui,'node_modules/.cache/decision-'))
const dom=new JSDOM('<!doctype html><div id="root"></div>',{url:'https://cide.test',pretendToBeVisual:true})
for(const k of ['window','document','location','HTMLElement','Element','Node','KeyboardEvent','MouseEvent'])globalThis[k]=dom.window[k]
globalThis.IS_REACT_ACT_ENVIRONMENT=true
globalThis.requestAnimationFrame=dom.window.requestAnimationFrame.bind(dom.window)
globalThis.cancelAnimationFrame=dom.window.cancelAnimationFrame.bind(dom.window)
// jsdom has no layout. Give the real virtualizer a viewport and row dimensions
// so this check actually opens a history record, rather than only mounting tabs.
Object.defineProperties(dom.window.HTMLElement.prototype,{
 offsetHeight:{get(){return this.getAttribute('aria-label')==='Decision history'?400:56}},
 offsetWidth:{get(){return 480}},
})
const calls=[]
const record={id:7,atMs:100,project:'p',session:'s',harness:'codex',scenario:'loop',period:'a',group:'treatment',mode:'automatic',question:'Change approach?',state:'Three failures',probabilities:{inspect:0.38,change:0.32,keep:0.30},intervention:{probability:0.70,threshold:50,passed:true,choices:{'Inspect diagnostics and source':38/70,'Change the search or debugging approach':32/70}},choice:'inspect',status:'applied',reason:'Native acknowledgement',action:'Inspect the source',outcome:'',latencyMs:20,inputTokens:12,feedback:null}
const snapshot={revision:1,dropped:0,sessions:[],scenarios:[],history:[record],total:1}
window.__TAURI_INTERNALS__={invoke:async(command,args)=>{calls.push({command,args});if(command==='decision_snapshot')return snapshot},metadata:{currentWindow:{label:'shell:test'}}}
let root
try {
 await build({configFile:false,root:ui,logLevel:'error',resolve:{alias:[{find:'@/store/workspace',replacement:'\0decision-store'},{find:'@',replacement:join(ui,'src')}]},
 plugins:[{name:'decision-fixtures',resolveId(id){if(id==='\0decision-store')return id},load(id){if(id==='\0decision-store')return 'export const useWorkspace=fn=>fn({boot:{workspace:{projects:{p:{id:"p",name:"Project"}}}}});'}}],
 build:{ssr:true,outDir:out,rollupOptions:{input:{settings:join(ui,'src/settings/DecisionSection.tsx'),insights:join(ui,'src/decision/Insights.tsx'),model:join(ui,'src/decision/model.ts')},output:{entryFileNames:'[name].mjs'}}}})
 const {DecisionSection}=await import(pathToFileURL(join(out,'settings.mjs')))
 const {Insights}=await import(pathToFileURL(join(out,'insights.mjs')))
 const {projectDefaults,comparisons,mergeHistory}=await import(pathToFileURL(join(out,'model.mjs')))
 const {createRoot}=await import('react-dom/client');root=createRoot(document.getElementById('root'))
 const writes=[],globalWrites=[]
 const settings={url:'http://127.0.0.1:11434',model:'d1',timeoutMs:2000,profiles:[]}
 await act(async()=>root.render(React.createElement(DecisionSection,{settings,project:{},edit:e=>writes.push(e),patch:e=>globalWrites.push(e)})))
 const enable=document.querySelector('[role="switch"][aria-label="Enable decision assistance"]')
 assert.equal(enable.getAttribute('aria-checked'),'false')
 await act(async()=>enable.click())
 assert.equal(writes.at(-1).field,'decision');assert.equal(writes.at(-1).value.enabled,true);assert.equal(globalWrites.length,0)
 assert.equal(Object.keys(projectDefaults().scenarios).length,6)
 const rows=mergeHistory(Array.from({length:10000},(_,id)=>({...record,id})),[{...record,id:10000}]);assert.equal(rows.length,10000);assert.equal(rows[0].id,10000)
 const base={session:'s',project:'p',harness:'codex',group:'baseline',period:'one',turns:1,tools:2,failures:0,elapsedMs:10,inputTokens:null,outputTokens:null,cost:null}
 assert.equal(comparisons([base,{...base,session:'t',inputTokens:100}])[0].input,null)
 assert.equal(comparisons([base,{...base,period:'two'}]).length,2)
 let resolveProbe, rejectProbe
 const probes=[]
 const probe=s=>{probes.push(s);return new Promise((resolve,reject)=>{resolveProbe=resolve;rejectProbe=reject})}
 await act(async()=>root.render(React.createElement(DecisionSection,{settings,project:{},edit:e=>writes.push(e),patch:e=>globalWrites.push(e),probe})))
 const testButton=[...document.querySelectorAll('button')].find(b=>b.textContent==='Test connection')
 await act(async()=>testButton.click())
 assert.equal(testButton.disabled,true)
 assert.ok(document.body.textContent.includes('120 seconds'))
 assert.equal(probes[0].timeoutMs,2000)
 await act(async()=>rejectProbe(new Error('Endpoint unavailable')))
 assert.ok(document.body.textContent.includes('Endpoint unavailable'))
 await act(async()=>testButton.click())
 assert.equal(document.body.textContent.includes('Endpoint unavailable'),false)
 await act(async()=>resolveProbe('System One ready · initial request 20370 ms · warm probe 13 ms · runtime budget 2000 ms'))
 assert.equal(testButton.disabled,false)
 assert.ok(document.body.textContent.includes('warm probe 13 ms'))
 assert.equal(globalWrites.length,0)
 for(const theme of ['dark','light'])for(const size of ['11','17']){
  document.documentElement.dataset.theme=theme;document.documentElement.style.setProperty('--ui-scale',String(Number(size)/13))
  let dismissed=false
  await act(async()=>root.render(React.createElement(Insights,{onDismiss:()=>dismissed=true})))
  await act(async()=>new Promise(r=>setTimeout(r,10)))
  const dialog=document.querySelector('[role="dialog"][aria-label="Decision Insights"]');assert.ok(dialog)
  assert.ok(document.body.textContent.includes('At each user request'))
  assert.ok(document.body.textContent.includes('Requires at least one unfinished check'))
  assert.ok(document.activeElement?.textContent.includes('Close'))
  const history=document.querySelector('[aria-label="Decision history"]')
  assert.equal(history.querySelectorAll('button').length,1)
  await act(async()=>history.querySelector('button').click())
  assert.ok(document.body.textContent.includes('1. Decide whether to intervene'))
  assert.ok(document.body.textContent.includes('2. Choose a response'))
  assert.ok(document.body.textContent.includes('70%'))
  assert.ok(document.body.textContent.includes('50%'))
  assert.ok(document.body.textContent.includes('54%'))
  assert.ok(document.body.textContent.includes('46%'))
  assert.ok(document.body.textContent.includes('Passed · delivery status shown above'))
  const distribution=[...document.querySelectorAll('summary')].find(e=>e.textContent.includes('Original model distribution'))
  assert.ok(distribution)
  await act(async()=>distribution.click())
  assert.ok(document.body.textContent.includes('38%'))
  assert.ok(document.body.textContent.includes('32%'))
  const compare=[...document.querySelectorAll('[role="tab"]')].find(e=>e.textContent==='Comparison')
  await act(async()=>compare.click());assert.ok(document.body.textContent.includes('observed totals'))
  await act(async()=>dialog.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true})));assert.equal(dismissed,true)
  await act(async()=>root.render(null))
 }
 // A gate can pass in Observe mode without an intervention being delivered.
 for(const variant of [
  {...record,status:'belowThreshold',action:'',intervention:{...record.intervention,threshold:80,passed:false}},
  {...record,mode:'observe',status:'observed',reason:'Observation mode; no action applied',action:''},
  {...record,intervention:null},
 ]){
  snapshot.history=[variant]
  await act(async()=>root.render(React.createElement(Insights,{onDismiss:()=>{}})))
  await act(async()=>new Promise(r=>setTimeout(r,10)))
  await act(async()=>document.querySelector('[aria-label="Decision history"] button').click())
  assert.ok(document.body.textContent.includes(variant.reason))
  if(variant.status==='belowThreshold')assert.ok(document.body.textContent.includes('Not passed · no intervention'))
  if(variant.intervention===null)assert.equal(document.body.textContent.includes('Decide whether to intervene'),false)
  await act(async()=>root.render(null))
 }
 assert.ok(calls.some(c=>c.command==='decision_snapshot'))
 console.log('Decision UI: grouped intervention gate, conditional advice, raw probabilities, legacy records, project-local enable, bounded history, themes, scales and keyboard dismissal passed')
} finally {if(root)await act(async()=>root.unmount());rmSync(out,{recursive:true,force:true});dom.window.close()}
