import assert from 'node:assert/strict'
import plugin from '../../crates/cide-app/src/decision-opencode.mjs'
const oldFetch=globalThis.fetch
const beforeEndpoint=process.env.CIDE_DECISION_ENDPOINT, beforeToken=process.env.CIDE_DECISION_TOKEN
process.env.CIDE_DECISION_ENDPOINT='http://127.0.0.1:9999/decision';process.env.CIDE_DECISION_TOKEN='test-only'
const hooks=new Map(),calls=[],switches=[],synthetics=[],events=[]
let wake, alive=true, valid=true, nextAction={}
const ctx={
  session:{hook:async(name,fn)=>{hooks.set(name,fn);return{dispose:async()=>{}}},switchModel:async v=>switches.push(v),synthetic:async v=>synthetics.push(v)},
  tool:{hook:async(name,fn)=>{hooks.set(name,fn);return{dispose:async()=>{}}}},
  event:{subscribe:async function*(){while(alive){if(events.length)yield events.shift();else await new Promise(resolve=>wake=resolve)}}},
}
const tick=()=>new Promise(resolve=>setTimeout(resolve,0))
const push=async event=>{events.push(event);wake?.();await tick();await tick()}
globalThis.fetch=async(url,options)=>{
  assert.equal(url,process.env.CIDE_DECISION_ENDPOINT);assert.equal(options.headers.Authorization,'Bearer test-only')
  const input=JSON.parse(options.body);calls.push(input)
  let reply={}
  if(input.op==='valid')reply={valid}
  if(input.op==='event'){reply=nextAction;nextAction={}}
  return {ok:true,json:async()=>reply}
}
let dispose
try {
  dispose=await plugin.setup(ctx)
  nextAction={id:1,generation:1,profile:{model:'local/coder',effort:'low'}}
  const prompt={sessionID:'s',messageID:'u1',prompt:{text:'Fix the bug'}}
  await hooks.get('prompt')(prompt)
  assert.equal(prompt.prompt.text,'Fix the bug')
  assert.deepEqual(switches,[{sessionID:'s',model:{providerID:'local',id:'coder',variant:'low'}}])
  await push({type:'session.model.selected',data:{sessionID:'s',model:{providerID:'local',id:'coder'}}})
  assert.equal(calls.filter(v=>v.op==='pin').length,0)
  await push({type:'session.model.selected',data:{sessionID:'s'}})
  assert.equal(calls.filter(v=>v.op==='pin').length,1)
  nextAction={id:2,generation:1,text:'Inspect the failing test'}
  const tool={sessionID:'s',id:'t1',tool:'bash',status:'error',input:{command:'test'},error:{message:'failure'}}
  await hooks.get('execute.after')(tool)
  const context={sessionID:'s',system:[],messages:[{role:'user',content:'Keep this'}],tools:{bash:{}}}
  await hooks.get('context')(context)
  assert.equal(context.system[0].text,'Cide decision assistance: Inspect the failing test')
  assert.deepEqual(context.messages,[{role:'user',content:'Keep this'}]);assert.equal(tool.error.message,'failure')
  nextAction={id:3,generation:1,text:'Must be cancelled'}
  await hooks.get('execute.after')({...tool,id:'t2'})
  valid=false
  await hooks.get('context')(context)
  assert.equal(context.system.length,1)
  await push({type:'session.execution.interrupted',data:{sessionID:'s',reason:'user'}})
  assert.ok(calls.some(v=>v.event?.kind==='stop'))
  nextAction={id:4,generation:1,text:'No continuation after user stop',continuation:true}
  await push({type:'session.execution.succeeded',id:'done1',data:{sessionID:'s'}})
  assert.equal(synthetics.length,0)
  valid=true
  await hooks.get('prompt')({sessionID:'s',messageID:'u2',prompt:{text:'Continue'}})
  nextAction={id:5,generation:2,text:'Run the outstanding check',continuation:true}
  await push({type:'session.execution.succeeded',id:'done2',data:{sessionID:'s'}})
  assert.equal(synthetics.length,1);assert.equal(synthetics[0].resume,true)
  await push({type:'session.text.ended',id:'answer',data:{sessionID:'s',text:'Blocked: please supply the test fixture'}})
  assert.ok(calls.some(v=>v.event?.kind==='message' && v.event.text.includes('Blocked')))
  await push({type:'session.step.ended',id:'usage1',data:{sessionID:'s',tokens:{input:100,output:20},cost:0.01}})
  await push({type:'session.step.ended',id:'usage1',data:{sessionID:'s',tokens:{input:100,output:20},cost:0.01}})
  assert.equal(calls.filter(v=>v.event?.kind==='usage').length,1)
  assert.equal(calls.find(v=>v.event?.kind==='usage').event.cacheTokens,undefined)
  await push({type:'session.step.ended',id:'usage2',data:{sessionID:'s',tokens:{input:10,output:3}}})
  assert.equal(calls.filter(v=>v.event?.kind==='usage').at(-1).event.inputTokens,110)
  assert.equal(calls.filter(v=>v.event?.kind==='usage').at(-1).event.cost,0.01)
  assert.ok(calls.filter(v=>v.op==='valid').every(v=>v.native==='s'))
  await push({type:'session.model.selected',data:{sessionID:'before-first-prompt',model:{providerID:'local',id:'default'}}})
  assert.equal(calls.filter(v=>v.op==='pin'&&v.native==='before-first-prompt').length,0)
  await push({type:'session.model.selected',data:{sessionID:'before-first-prompt',model:{providerID:'local',id:'chosen'},previous:{providerID:'local',id:'default'}}})
  assert.equal(calls.filter(v=>v.op==='pin'&&v.native==='before-first-prompt').length,1)
  // Research must reach the first model request even with no model-routing profile.
  nextAction={research:{id:6,generation:3,text:'Find the relevant existing code'}}
  await hooks.get('prompt')({sessionID:'research',messageID:'u3',prompt:{text:'Add filtering'}})
  const firstContext={sessionID:'research',system:[],messages:[]}
  await hooks.get('context')(firstContext)
  assert.equal(firstContext.system[0].text,'Cide decision assistance: Find the relevant existing code')
  assert.ok(calls.some(v=>v.op==='ack'&&v.native==='research'&&v.id===6&&v.ok))
  const switchesBefore=switches.length
  nextAction={research:{id:7,generation:4,text:'Stale research'},id:8,generation:4,profile:{model:'local/other'}}
  valid=false
  await hooks.get('prompt')({sessionID:'research',messageID:'u4',prompt:{text:'Different task'}})
  await hooks.get('context')(firstContext)
  assert.equal(firstContext.system.length,1);assert.equal(switches.length,switchesBefore)
  valid=true
  // Observation/no-op/error responses contribute no context and do not switch models.
  await hooks.get('prompt')({sessionID:'research',messageID:'u5',prompt:{text:'One HTML page'}})
  await hooks.get('context')(firstContext)
  assert.equal(firstContext.system.length,1);assert.equal(switches.length,switchesBefore)
  console.log('Decision plugin: native model routing, context delivery, cancellation, completion and usage deduplication passed')
} finally {
  alive=false;wake?.();await dispose?.();globalThis.fetch=oldFetch
  if(beforeEndpoint===undefined)delete process.env.CIDE_DECISION_ENDPOINT;else process.env.CIDE_DECISION_ENDPOINT=beforeEndpoint
  if(beforeToken===undefined)delete process.env.CIDE_DECISION_TOKEN;else process.env.CIDE_DECISION_TOKEN=beforeToken
}
