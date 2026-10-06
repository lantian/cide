/** Optional real-browser audit: pnpm --dir ui run audit:user-inputs.
 * Uses local fixtures and headless Chromium; no CLI processes, model calls or live IDE.
 * Screenshots and browser data stay in node_modules/.cache/recap-browser-*.
 */
import assert from 'node:assert/strict'
import { build, preview } from 'vite'
import { existsSync, mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { execFileSync, spawn } from 'node:child_process'
const ui = resolve(import.meta.dirname, '..')
// Feed the real Rust parser's output through the mounted feature hook and terminal painter.
// CIDE_INPUT_TRANSCRIPT can name an existing rollout; default to the captured 0.160 shape.
const parserOutput = execFileSync('cargo', ['test', '--locked', '-p', 'cide-core', '--test', 'user_inputs',
  'existing_transcript', '--', '--ignored', '--nocapture'], {
  cwd: resolve(ui, '..'), encoding: 'utf8',
  env: { ...process.env, CIDE_INPUT_TRANSCRIPT: process.env.CIDE_INPUT_TRANSCRIPT
    ?? resolve(ui, '../crates/cide-core/tests/fixtures/codex-completed-user-messages.jsonl') },
})
const inputPage = JSON.parse(parserOutput.match(/^USER_INPUT_PAGE=(.*)$/m)?.[1] ?? 'null')
assert(inputPage?.total > 0, 'Rust must read submitted inputs before the browser paints them')
mkdirSync(join(ui, 'node_modules/.cache'), { recursive: true })
const dir = mkdtempSync(join(ui, 'node_modules/.cache/recap-browser-'))
writeFileSync(join(dir, 'index.html'), '<html data-theme="light"><body style="margin:0"><div id="root"></div><script type="module" src="./main.tsx"></script></body></html>')
writeFileSync(join(dir, 'main.tsx'), `
import React from 'react'; import {createRoot} from 'react-dom/client';
import { createTerminal, promoteWebgl, releaseWebgl, retheme, ensureSearch, searchOptions, concreteColor } from '@/terminal/xterm';
import { highlightUserInputs } from '@/terminal/userInputHighlight';
import { followUserInputs } from '@/terminal/userInputNavigation';
import { useTerminalFind } from '@/terminal/findStore';
import { Recap } from '@/kit/components/Recap';
import { UserInputRecap, useUserInputs } from '@/panes/UserInputRecap';
import { sessionJournal } from '@/ipc/client';
import { useWorkspace } from '@/store/workspace';
import { getHost, openTerminal, destroyHost } from '@/layout/paneHosts';
import '@/styles/tokens.css'; import '@/styles/fonts.css';
document.body.style.background='var(--panel)';
let handle, painter;
function Fixture() { const [expanded, setExpanded]=React.useState(false); const [text,setText]=React.useState('Привет мир\\nПроверь перенос панели, поиск и тему.\\n\\nИстория относится к разговору.');window.recapText=setText;return <div id="fixture" style={{height:480,width:800,display:'flex',flexDirection:'column'}}>
<Recap text={text} ordinal={2} total={2} expanded={expanded} onFirst={()=>{}} onPrevious={()=>{}} onNext={()=>{}} onLatest={()=>{}} onExpand={()=>setExpanded(!expanded)}/>
<div id="terminal" style={{flex:1,minHeight:0}}/></div>; }
const root = createRoot(document.getElementById('root'));
root.render(<Fixture/>);
const inputPage = ${JSON.stringify(inputPage)};
const resumedPage = {inputs:[{id:'resumed-1',ordinal:1,text:'Prompt from the resumed conversation'}],total:1,hasPrevious:false,available:true,generation:0};
const transcriptPane = {id:'transcript-pane',kind:'claude',harness:'codex',session:'routing-id',conversation:'actual-thread'};
sessionJournal.userInputs=async(project,id)=>{if(project!=='transcript-project'||!['actual-thread','resumed-thread'].includes(id))throw new Error('wrong conversation identity');return id==='actual-thread'?inputPage:resumedPage;};
sessionJournal.onUserInputsChanged=async()=>()=>{};
const transcriptText=page=>page.inputs.map(input=>'› '+input.text.replaceAll('\\n','\\r\\n  ')+'\\r\\n\\r\\n• Response\\r\\n\\r\\n').join('')+'› draft';
function TranscriptFixture({renderer}) {
 const slot=React.useRef(null);
 const [conversation,setConversation]=React.useState('actual-thread');window.resumeConversation=setConversation;
 React.useEffect(()=>{
  const host=getHost(transcriptPane.id);slot.current.appendChild(host.el);
  handle=openTerminal(transcriptPane.id,'claude');handle.fit.fit();
  if(renderer==='webgl')promoteWebgl(handle);else releaseWebgl(handle);
  handle.term.write(transcriptText(inputPage));
  return()=>destroyHost(transcriptPane.id);
 },[]);
 const feature=useUserInputs({...transcriptPane,conversation},'transcript-project');
 return <div data-kind="claude" style={{height:480,width:800,display:'flex',flexDirection:'column'}}>
  <UserInputRecap feature={feature}/><div ref={slot} style={{flex:1,minHeight:0,position:'relative'}}/>
 </div>;
}
window.transcriptProbe=async(theme,renderer)=>{
 painter?.dispose();handle?.dispose();document.documentElement.dataset.theme=theme;
 useWorkspace.setState({boot:{workspace:{settings:{terminal:{showRecap:true,highlightUserInput:true}}}}});
 root.render(<TranscriptFixture key={theme+renderer} renderer={renderer}/>);
 await new Promise(r=>setTimeout(r,650));
 return {recap:document.querySelector('[aria-label="User input recap"]').textContent,
  rails:document.querySelectorAll('[data-user-input-rails] > div').length,
  canvas:!!document.querySelector('.xterm-screen canvas')};
};
window.resumeProbe=async(id)=>{
 const terminal=handle.term;
 window.resumeConversation(id);handle.term.reset();handle.term.write(transcriptText(id==='actual-thread'?inputPage:resumedPage));
 await new Promise(r=>setTimeout(r,650));
 return {recap:document.querySelector('[aria-label="User input recap"]').textContent,
  rails:document.querySelectorAll('[data-user-input-rails] > div').length,stableTerminal:handle.term===terminal};
};
window.toggleHighlight=async(enabled)=>{
 const terminal=handle.term;
 useWorkspace.setState(state=>({boot:{...state.boot,workspace:{...state.boot.workspace,settings:{...state.boot.workspace.settings,
  terminal:{...state.boot.workspace.settings.terminal,highlightUserInput:enabled}}}}}));
 await new Promise(r=>setTimeout(r,100));
 return {rails:document.querySelectorAll('[data-user-input-rails] > div').length,
  recap:document.querySelector('[aria-label="User input recap"]').textContent,stableTerminal:terminal===handle.term};
};
window.navigationProbe=async()=>{
 const terminal=handle.term;
 await window.toggleHighlight(false);
 const pause=()=>new Promise(r=>setTimeout(r,90));
 const counter=()=>document.querySelector('[aria-label="User input recap"] [aria-live="polite"]').textContent;
 const longText=inputPage.inputs.map(input=>'› '+input.text.replaceAll('\\n','\\r\\n  ')+'\\r\\n\\r\\n• Response\\r\\n'+
  Array.from({length:32},(_,i)=>'output '+input.ordinal+' line '+i+'\\r\\n').join('')+'\\r\\n').join('')+'› draft';
 terminal.reset();await new Promise(r=>terminal.write(longText,r));await pause();
 const rows=[];for(let row=0;row<terminal.buffer.active.length;row++){if(terminal.buffer.active.getLine(row)?.translateToString(true).startsWith('› '))rows.push(row)}
 const latest=inputPage.inputs.at(-1).ordinal,second=inputPage.inputs[1].ordinal;
 terminal.scrollToLine(rows[1]+inputPage.inputs[1].text.split('\\n').length);await pause();const below=counter();
 terminal.scrollToLine(rows[1]);await pause();const at=counter();
 terminal.scrollToBottom();await pause();const bottom=counter();
 const previousViewport=terminal.buffer.active.viewportY;
 document.querySelector('[aria-label="First input"]').click();await pause();const manual=counter(),browseStayed=terminal.buffer.active.viewportY===previousViewport;
 document.querySelector('[aria-label="Show input in console"]').click();await pause();const jumped=terminal.buffer.active.viewportY,jumpSelection=counter();
 await new Promise(r=>terminal.write('\\r\\nadditional output',r));await pause();const held=counter();
 terminal.scrollToLine(rows[1]+inputPage.inputs[1].text.split('\\n').length);await pause();
 const wheel=new WheelEvent('wheel',{deltaY:-40,bubbles:true,cancelable:true,view:window});Object.defineProperty(wheel,'wheelDeltaY',{value:120});
 terminal.element.querySelector('.xterm-screen').dispatchEvent(wheel);await pause();const wheeled=counter();
 terminal.scrollToBottom();await pause();await window.toggleHighlight(true);
 return {below,at,bottom,manual,browseStayed,jumped,jumpSelection,held,wheeled,firstRow:rows[0],second,latest,total:inputPage.total,stableTerminal:terminal===handle.term};
};
window.altNavigationProbe=async()=>{
 const terminal=handle.term,entries=[{id:'one',ordinal:1,text:'one'},{id:'two',ordinal:2,text:'two'},{id:'three',ordinal:3,text:'three'}];
 terminal.reset();let offset=38;
 const history=entries.flatMap(input=>['› '+input.text,'• answer '+input.ordinal,...Array.from({length:23},(_,i)=>'output '+input.ordinal+' line '+i)]);
 const draw=()=>new Promise(r=>terminal.write('\\x1b[?1049h\\x1b[?1000h\\x1b[?1006h\\x1b[2J\\x1b[H'+history.slice(offset,offset+terminal.rows-1).join('\\r\\n')+'\\x1b['+terminal.rows+';1H› draft',r));
 await draw();const followed=[],outbound=[];let stopped=false;
 const writer=terminal.onData(data=>{outbound.push(data);if(stopped)return;for(const match of data.matchAll(/\\x1b\\[<(64|65);\\d+;\\d+M/g)){offset=Math.max(0,Math.min(history.length-terminal.rows+1,offset+(match[1]==='64'?-3:3)));void draw();}});
 const nav=followUserInputs(handle,'codex',entries,ordinal=>followed.push(ordinal));
 const moved=await nav.reveal(1),atFirst=offset,encoded=outbound.every(data=>/^\\x1b\\[<(64|65);\\d+;\\d+M$/.test(data));
 const movedForward=await nav.reveal(3),atThird=offset;
 const rect=terminal.element.querySelector('.xterm-screen').getBoundingClientRect();
 const wheel=delta=>terminal.element.querySelector('.xterm-screen').dispatchEvent(new WheelEvent('wheel',{deltaY:delta,clientX:rect.left+rect.width/2,clientY:rect.top+rect.height/2,bubbles:true,cancelable:true,view:window}));
 wheel(-60);await new Promise(r=>setTimeout(r,90));const context=followed.at(-1);
 const pending=nav.reveal(1);terminal.element.dispatchEvent(new KeyboardEvent('keydown',{key:'Shift',bubbles:true}));const cancelled=await pending;
 stopped=true;await new Promise(r=>terminal.write('\\x1b[?1000l\\x1b[2J\\x1b[Hno retained prompt\\x1b['+terminal.rows+';1H› draft',r));
 const count=outbound.length,unsupported=await nav.reveal(2),noTypedKeys=outbound.length===count;
 nav.dispose();writer.dispose();terminal.reset();await new Promise(r=>terminal.write(transcriptText(inputPage),r));await new Promise(r=>setTimeout(r,90));
 return {moved,atFirst,movedForward,atThird,encoded,reports:count,context,cancelled,unsupported,noTypedKeys};
};
window.probe = async (theme, renderer, harness, alt) => {
 painter?.dispose(); handle?.dispose(); document.getElementById('terminal').replaceChildren();
 document.documentElement.dataset.theme=theme;
 handle = createTerminal('claude','test'); handle.term.open(document.getElementById('terminal')); handle.fit.fit();
 if(renderer==='webgl') promoteWebgl(handle); else releaseWebgl(handle);
 const marker=harness==='codex'?'›':'❯'; const answer=harness==='codex'?'•':'⏺';
 const text=(alt?'\\x1b[?1049h':'') + marker+' Привет мир\\r\\n\\r\\n'+answer+' Привет мир — ответ модели\\r\\n\\r\\n'+marker+' Привет мир';
 await new Promise(resolve=>handle.term.write(text,resolve));
 painter=highlightUserInputs(handle,'test',harness,[{text:'Привет мир'}]);
 await new Promise(resolve=>setTimeout(resolve,350));
 window.currentTerm=handle.term;
 const screen=document.querySelector('.xterm-screen');
 return { rails:screen.querySelectorAll('[data-user-input-rails] > div').length, cells: handle.term.buffer.active.getLine(0).translateToString(true),
  active:handle.term.buffer.active.type, canvas:!!screen.querySelector('canvas'), span:[...screen.querySelectorAll('.xterm-rows span')].find(x=>x.textContent.includes('П'))?.getAttribute('style'),
  recap:document.querySelector('[aria-label="User input recap"]').getBoundingClientRect().height, screen:screen.getBoundingClientRect().height };
};
window.changeTheme=async(theme)=>{document.documentElement.dataset.theme=theme;retheme([handle]);await new Promise(r=>setTimeout(r,300));return [...document.querySelectorAll('.xterm-rows span')].find(x=>x.textContent.includes('П'))?.getAttribute('style')};
window.findOn=async()=>{useTerminalFind.getState().openFind('test');ensureSearch(handle).findNext('Привет',searchOptions());await new Promise(r=>setTimeout(r,300));return [...document.querySelectorAll('.xterm-rows span')].find(x=>x.textContent.includes('П'))?.getAttribute('style')};
window.findOff=async()=>{ensureSearch(handle).clearDecorations();useTerminalFind.getState().closeFind('test');handle.term.clearSelection();await new Promise(r=>setTimeout(r,300));return [...document.querySelectorAll('.xterm-rows span')].find(x=>x.textContent.includes('П'))?.getAttribute('style')};
window.inputColors=()=>{
 const hex=value=>{const color=concreteColor(value),rgb=color.match(/^rgb\\((\\d+), (\\d+), (\\d+)\\)$/);return rgb?'#'+rgb.slice(1).map(n=>(+n).toString(16).padStart(2,'0')).join(''):color;};
 const style=getComputedStyle(document.documentElement),token=name=>hex(style.getPropertyValue(name).trim());
 const recap=getComputedStyle(document.querySelector('[aria-label="User input recap"]'));
 return {fill:token('--user-input-bg'),ink:token('--user-input-text'),edge:token('--user-input-edge'),accent:token('--accent'),
  recapFill:hex(recap.backgroundColor),recapEdge:hex(recap.borderLeftColor),
  decoration:[...document.querySelectorAll('.xterm-rows span')].find(x=>x.textContent.includes('П'))?.getAttribute('style')};
};
window.changeAccent=async(custom)=>{
 const root=document.documentElement;
 if(custom){root.dataset.accent='custom';root.style.setProperty('--accent-l','#245ac7');root.style.setProperty('--accent-d','#8babed');}
 else{delete root.dataset.accent;root.style.removeProperty('--accent-l');root.style.removeProperty('--accent-d');}
 retheme([handle]);await new Promise(r=>setTimeout(r,80));return window.inputColors();
};
window.nativeColorProbe=async(alt)=>{
 painter.dispose();const term=handle.term;
 await new Promise(r=>term.write('\\x1b[?1049l',r));term.reset();
 const dark=document.documentElement.dataset.theme==='dark';
 const pair=dark?'\\x1b[38;2;183;236;194;48;2;23;50;25m':'\\x1b[38;2;18;58;148;48;2;231;239;255m';
 const text=(alt?'\\x1b[?1049h':'')+'› \\x1b[34mПривет\\x1b[0m '+pair+'мир\\x1b[0m\\r\\n\\r\\n• Model response\\r\\n› draft';
 await new Promise(r=>term.write(text,r));await new Promise(r=>setTimeout(r,60));
 const colors=()=>{
  const cells=[...term.element.querySelector('.xterm-rows')?.firstElementChild?.querySelectorAll('span')??[]].flatMap(element=>{
   const style=getComputedStyle(element);return [...element.textContent].map(text=>({text,foreground:style.color,background:style.backgroundColor}));
  });
  const text=cells.map(c=>c.text).join('');
  return ['Привет','мир'].map(word=>{const start=text.indexOf(word);if(start<0)return null;return cells.slice(start,start+word.length);});
 };
 const before=colors(),options=[];
 const original=term.registerDecoration;term.registerDecoration=function(opts){options.push(opts);return original.call(this,opts)};
 painter=highlightUserInputs(handle,'test','codex',[{text:'Привет мир'}]);
 await new Promise(r=>setTimeout(r,80));term.registerDecoration=original;
 const result={before,after:colors(),rails:document.querySelectorAll('[data-user-input-rails] > div').length,
  overridesForeground:options.some(o=>o.foregroundColor!==undefined),
  overridesNativeBackground:options.some(o=>{const line=term.buffer.active.getLine(o.marker.line);for(let x=o.x??0;x<(o.x??0)+o.width;x++){const cell=line.getCell(x);if(!cell.isBgDefault()||cell.isInverse())return true;}return false;}),
  canvas:!!term.element.querySelector('canvas')};
 painter.dispose();await new Promise(r=>setTimeout(r,80));
 result.disabled=colors();result.disabledRails=document.querySelectorAll('[data-user-input-rails] > div').length;
 painter=highlightUserInputs(handle,'test','codex',[{text:'Привет мир'}]);await new Promise(r=>setTimeout(r,80));
 return result;
};
window.scrollProbe=async(alt)=>{
 const term=handle.term;
 await new Promise(r=>term.write('\\x1b[?1049l',r));term.reset();
 const content='› Привет мир\\r\\n\\r\\n• Model response\\r\\n'+(alt?'':'model output\\r\\n'.repeat(80))+'› draft';
 const text=(alt?'\\x1b[?1049h':'')+content;
 await new Promise(r=>term.write(text,r));term.scrollToTop();await new Promise(r=>setTimeout(r,80));
 const layer=document.querySelector('[data-user-input-rails]'),rail=layer.firstElementChild;
 let registrations=0,removals=0;
 const original=term.registerDecoration;term.registerDecoration=function(...args){registrations++;return original.apply(this,args)};
 const observer=new MutationObserver(records=>{for(const record of records)removals+=record.removedNodes.length});
 observer.observe(layer.parentElement,{childList:true});observer.observe(layer,{childList:true});
 const pause=()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)));
 let gaps=0,misplaced=0,wheelViewport=0;
 await new Promise(r=>term.write('\\x1b[?25l\\x1b[?25h',r));await pause();
 const cursorRegistrations=registrations;
 const wheel=delta=>{
  const event=new WheelEvent('wheel',{deltaY:delta,bubbles:true,cancelable:true,view:window});
  // Chromium leaves legacy wheelDeltaY zero on a constructed event, unlike an actual
  // wheel event. xterm's wheel normalizer reads that legacy field first.
  Object.defineProperty(event,'wheelDeltaY',{value:-delta*3});
  document.querySelector('.xterm-screen').dispatchEvent(event);
 };
 for(let i=0;i<20;i++){
  if(!alt){
   wheel(40);await pause();
   wheelViewport=Math.max(wheelViewport,term.buffer.active.viewportY);
   wheel(-40);await pause();
   term.scrollToTop();term.scrollLines(1);await pause();term.scrollLines(-1);
  }
  // Alternate-screen programs redraw on wheel input. Repainting the same grid, or merely
  // toggling the cursor on the normal buffer, must not discard the prompt's decoration.
  await new Promise(r=>term.write(alt?'\\x1b[H\\x1b[2J'+content:'\\x1b[?25l\\x1b[?25h',r));await pause();
  if(!layer.isConnected||layer.firstElementChild!==rail)gaps++;
  if(Math.abs(parseFloat(rail.style.top)+term.buffer.active.viewportY*document.querySelector('.xterm-screen').clientHeight/term.rows)>.1)misplaced++;
 }
 term.registerDecoration=original;observer.disconnect();
 return {registrations,cursorRegistrations,removals,gaps,misplaced,wheelViewport,viewport:term.buffer.active.viewportY,
  grounds:[...document.querySelectorAll('.xterm-rows > div')].filter(row=>row.style.backgroundColor).length};
};
window.repaint=async()=>{await new Promise(resolve=>handle.term.write('\\x1b[H\\x1b[2J• Привет мир\\r\\n› черновик',resolve));await new Promise(r=>setTimeout(r,300));return document.querySelectorAll('[data-user-input-rails] > div').length};
window.ready=true;
`)
await build({ configFile:false, root:dir, logLevel:'error', resolve:{alias:{'@':join(ui,'src')}}, build:{outDir:join(dir,'dist')} })
const server=await preview({configFile:false,root:dir,logLevel:'error',build:{outDir:join(dir,'dist')},preview:{port:0}})
const base=server.resolvedUrls.local[0]
const chromium = [process.env.CHROMIUM, '/usr/bin/chromium', '/usr/bin/chromium-browser', '/usr/bin/google-chrome'].find(bin => bin && existsSync(bin))
if (!chromium) throw new Error('Set CHROMIUM to a headless Chromium executable.')
const child=spawn(chromium,['--headless=new','--remote-debugging-port=0','--no-first-run','--no-default-browser-check','--window-size=820,520',`--user-data-dir=${dir}/profile`,'about:blank'],{stdio:['ignore','ignore','pipe']})
let ws
try {
 const port=await new Promise((resolve,reject)=>{let err='';child.stderr.on('data',data=>{err+=data;const m=err.match(/DevTools listening on ws:\/\/[^:]+:(\d+)\//);if(m)resolve(+m[1])});child.on('exit',code=>reject(new Error('chromium '+code+' '+err)));setTimeout(()=>reject(new Error('browser startup timeout '+err)),20000).unref()})
 const targets=await(await fetch('http://127.0.0.1:'+port+'/json/list')).json();ws=new WebSocket(targets.find(t=>t.type==='page').webSocketDebuggerUrl)
 await new Promise((r,j)=>{ws.onopen=r;ws.onerror=j});let id=0;const pending=new Map();const errors=[]
 ws.onmessage=event=>{const msg=JSON.parse(event.data);if(msg.id){const p=pending.get(msg.id);pending.delete(msg.id);msg.error?p.reject(msg.error):p.resolve(msg.result)}else if(msg.method==='Runtime.exceptionThrown')errors.push(msg.params.exceptionDetails)}
 const send=(method,params={})=>new Promise((resolve,reject)=>{const n=++id;pending.set(n,{resolve,reject});ws.send(JSON.stringify({id:n,method,params}))})
 const evaluate=async(expression)=>{const r=await send('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true});if(r.exceptionDetails)throw new Error(JSON.stringify(r.exceptionDetails));return r.result.value}
 await send('Runtime.enable');await send('Page.navigate',{url:base+'?renderer=webgl'});for(let n=0;n<100;n++){if(await evaluate('window.ready'))break;await new Promise(r=>setTimeout(r,100))}
 for(const theme of ['light','dark'])for(const renderer of ['dom','webgl'])for(const harness of ['claude','codex'])for(const alt of [false,true]){
  const result=await evaluate('window.probe('+[theme,renderer,harness,alt].map(x=>JSON.stringify(x)).join(',')+')');
  console.log(theme,renderer,harness,alt?'alt':'normal',JSON.stringify(result));assert.equal(result.rails,1);assert.equal(result.active,alt?'alternate':'normal');assert(result.screen>0);assert(result.recap>0)
  if(renderer==='dom')assert(result.span?.includes('background-color'), 'decorations colour the user cells in DOM including alt')
  if(renderer==='webgl')assert.equal(result.canvas,true,'WebGL must actually be active')
  if(harness==='codex'&&alt){const shot=await send('Page.captureScreenshot',{format:'png'});writeFileSync(join(dir, 'recap-'+theme+'-'+renderer+'.png'),Buffer.from(shot.data,'base64'))}
 }
 // In-place updates exercise the same listeners used by a live pane, not just its initial paint.
 await evaluate("window.probe('light','dom','codex',true)");
 await evaluate("window.changeTheme('dark')");
 const defaultColors=await evaluate('window.inputColors()');
 assert.equal(defaultColors.edge,defaultColors.accent);assert.equal(defaultColors.recapFill,defaultColors.fill);assert.equal(defaultColors.recapEdge,defaultColors.edge);
 assert(defaultColors.decoration.includes('background-color:'+defaultColors.fill));
 const searching=await evaluate('window.findOn()');
 assert(!searching.includes('background-color:'+defaultColors.fill), 'search has priority over the speaker wash');
 assert((await evaluate('window.findOff()')).includes('background-color:'+defaultColors.fill));
 assert.equal(await evaluate("window.currentTerm.select(2,0,10);window.currentTerm.getSelection()"),'Привет мир','clipboard selection still reads original PTY text');
 assert.equal(await evaluate('window.repaint()'),0,'a TUI repaint cannot leave speaker marks on model output');
 for(const theme of ['light','dark'])for(const renderer of ['dom','webgl']){
  await evaluate('window.probe('+[theme,renderer,'codex',false].map(x=>JSON.stringify(x)).join(',')+')');
  const colors=await evaluate('window.changeAccent(true)');assert.equal(colors.edge,colors.accent);
  assert.equal(colors.recapFill,colors.fill);assert.equal(colors.recapEdge,colors.edge);
  if(renderer==='dom')assert(colors.decoration.includes('background-color:'+colors.fill),'custom accent reaches xterm');
  await evaluate('window.changeAccent(false)');
  for(const alt of [false,true]){
   const result=await evaluate('window.nativeColorProbe('+alt+')');console.log('native colours',theme,renderer,alt,JSON.stringify(result));
   assert.equal(result.rails,1);assert.equal(result.overridesForeground,false);assert.equal(result.overridesNativeBackground,true);
   assert.equal(result.disabledRails,0);
   if(renderer==='dom'){
    assert(result.before.every(Boolean));
    assert(result.after.every(Boolean));
    assert.deepEqual(result.after.map(word=>word.map(c=>c.foreground)),result.before.map(word=>word.map(c=>c.foreground)),'preserve ANSI and true-colour text');
    assert.notDeepEqual(result.after[1].map(c=>c.background),result.before[1].map(c=>c.background),'enabled highlighting replaces the CLI background');
    assert.deepEqual(result.disabled,result.before,'disabled highlighting restores native foregrounds and backgrounds exactly');
   }else assert(result.canvas);
  }
  for(const alt of [false,true]){
   const result=await evaluate('window.scrollProbe('+alt+')');console.log('scroll',theme,renderer,alt,JSON.stringify(result));
   assert.equal(result.cursorRegistrations,0,'cursor redraws retain cell decorations');
   // Erase Display automatically retires xterm markers: the replacement must be registered
   // before render, while our rail survives. Normal-buffer scrolling retires no markers.
   assert.equal(result.registrations,alt?20:0,'only xterm-retired markers need replacement');
   assert.equal(result.removals,0,'rails are never removed during scrolling');assert.equal(result.gaps,0);assert.equal(result.misplaced,0);
   if(!alt)assert(result.wheelViewport>0,'wheel events must actually scroll the buffer');
   if(renderer==='dom')assert.equal(result.grounds,1,'DOM backgrounds follow the correct visible row');
  }
 }
 // Native layout decides whether disclosure has anything to show. Test both explicit lines
 // and width-dependent wrapping, plus the collapse control while full text is open.
 assert(await evaluate("!!document.querySelector('[aria-label=\\\"Expand input\\\"]')"));
 await evaluate("document.querySelector('[aria-label=\\\"Expand input\\\"]').click()");
 await new Promise(r=>setTimeout(r,100));
 assert(await evaluate("!!document.querySelector('[aria-label=\\\"Collapse input\\\"]')"));
 await evaluate("document.querySelector('[aria-label=\\\"Collapse input\\\"]').click();window.recapText('short prompt')");
 await new Promise(r=>setTimeout(r,100));
 assert(await evaluate("!document.querySelector('[aria-expanded]')"),'short text has no disclosure icon');
 await evaluate("window.recapText('wrap '.repeat(20));document.getElementById('fixture').style.width='260px'");
 await new Promise(r=>setTimeout(r,100));
 assert(await evaluate("!!document.querySelector('[aria-label=\\\"Expand input\\\"]')"),'wrapping shows disclosure');
 await evaluate("document.getElementById('fixture').style.width='800px'");
 await new Promise(r=>setTimeout(r,100));
 assert(await evaluate("!document.querySelector('[aria-expanded]')"),'widening hides disclosure');
 for(const theme of ['light','dark'])for(const renderer of ['dom','webgl']){
  const result=await evaluate('window.transcriptProbe('+[theme,renderer].map(x=>JSON.stringify(x)).join(',')+')');
  assert(result.recap.includes(inputPage.inputs.at(-1).text));assert(result.recap.includes(inputPage.total+' / '+inputPage.total));
  assert.equal(result.rails,inputPage.inputs.length,'the mounted hook must highlight each Rust-parsed submission');
  assert(!result.recap.includes('Your input'));
  assert(await evaluate("document.querySelector('[aria-label=\\\"First input\\\"]').getBoundingClientRect().left<document.querySelector('[aria-label=\\\"Latest input\\\"]').getBoundingClientRect().left"));
  if(renderer==='webgl')assert(result.canvas);
  const shot=await send('Page.captureScreenshot',{format:'png'});writeFileSync(join(dir,'transcript-'+theme+'-'+renderer+'.png'),Buffer.from(shot.data,'base64'));
  console.log('transcript',theme,renderer,JSON.stringify(result));
  const resumed=await evaluate("window.resumeProbe('resumed-thread')");
  assert(resumed.recap.includes('Prompt from the resumed conversation'));assert(resumed.recap.includes('1 / 1'));
  assert.equal(resumed.rails,1);assert(resumed.stableTerminal);
  const returned=await evaluate("window.resumeProbe('actual-thread')");
  assert(returned.recap.includes(inputPage.inputs.at(-1).text));assert.equal(returned.rails,inputPage.inputs.length);assert(returned.stableTerminal);
  const disabled=await evaluate('window.toggleHighlight(false)');assert.equal(disabled.rails,0);assert(disabled.stableTerminal);assert(disabled.recap.includes(inputPage.inputs.at(-1).text));
  const enabled=await evaluate('window.toggleHighlight(true)');assert.equal(enabled.rails,inputPage.inputs.length);assert(enabled.stableTerminal);
  const navigation=await evaluate('window.navigationProbe()');console.log('navigation',theme,renderer,JSON.stringify(navigation));
  assert.equal(navigation.below,navigation.second+' / '+navigation.total);assert.equal(navigation.at,(navigation.second-1)+' / '+navigation.total);
  assert.equal(navigation.bottom,navigation.latest+' / '+navigation.total);assert(navigation.browseStayed);
  assert.equal(navigation.manual,inputPage.inputs[0].ordinal+' / '+navigation.total);assert.equal(navigation.jumpSelection,navigation.manual);
  assert.equal(navigation.jumped,navigation.firstRow);assert.equal(navigation.held,navigation.manual);assert.equal(navigation.wheeled,navigation.at);assert(navigation.stableTerminal);
  const altNavigation=await evaluate('window.altNavigationProbe()');console.log('alt-navigation',theme,renderer,JSON.stringify(altNavigation));
  assert.equal(altNavigation.moved,'shown');assert(altNavigation.atFirst<25);assert.equal(altNavigation.movedForward,'shown');assert(altNavigation.atThird>25);
  assert(altNavigation.encoded);assert(altNavigation.reports>0);assert.equal(altNavigation.context,2);
  assert.equal(altNavigation.cancelled,'cancelled');assert.equal(altNavigation.unsupported,'unsupported');assert(altNavigation.noTypedKeys);
 }
 assert.equal(errors.length,0,JSON.stringify(errors));console.log('browser: 16 theme/renderer/harness/buffer cases, 4 Rust-transcript feature/resume cases, and 8 normal/alternate navigation cases passed')
}finally{ws?.close();child.kill();await new Promise(r=>server.httpServer.close(r))}
