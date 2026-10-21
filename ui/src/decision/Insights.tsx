import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { decisionApi } from '@/ipc/client'
import type { DecisionRecord, DecisionScenario, DecisionSnapshot } from '@/ipc/generated'
import { useWorkspace } from '@/store/workspace'
import { Modal } from '@/overlays/ModalShell'
import { Dialog } from '@/kit/components/Overlay'
import { Button } from '@/kit/components/Button'
import { Badge } from '@/kit/components/Status'
import { Card, CodeBlock, Disclosure, Summary, Table, Tabs, Toolbar } from '@/kit/components/Surface'
import { Banner, Progress } from '@/kit/components/Feedback'
import { Select } from '@/settings/controls'
import { SCENARIOS, comparisons, mergeHistory } from './model'
import styles from './Insights.module.css'

const EMPTY: DecisionSnapshot = {revision:0,dropped:0,sessions:[],scenarios:[],history:[],total:0}
export function Insights({onDismiss}:{onDismiss:()=>void}) {
  const [snapshot,setSnapshot] = useState(EMPTY)
  const [history,setHistory] = useState<DecisionRecord[]>([])
  const [scenario,setScenario] = useState<DecisionScenario|'all'>('all')
  const [project,setProject] = useState('all')
  const [tab,setTab] = useState<'history'|'comparison'|'sessions'>('history')
  const [selected,setSelected] = useState<number|null>(null)
  const [error,setError] = useState('')
  const [loading,setLoading] = useState(false)
  const close = useRef<HTMLButtonElement>(null)
  const scroll = useRef<HTMLDivElement>(null)
  const view = useRef(0)
  const projects = useWorkspace(s=>s.boot?.workspace.projects ?? {})
  useLayoutEffect(()=>{close.current?.focus()},[])
  useEffect(()=>{
    let live=true, busy=false
    view.current++
    setHistory([]);setSelected(null)
    const refresh=async()=>{
      if(busy)return;busy=true
      const version=view.current
      try { const s=await decisionApi.snapshot(0,200,scenario==='all'?null:scenario,project==='all'?null:project)
        if(live&&view.current===version){setSnapshot(s);setHistory(h=>mergeHistory(h,s.history));setError('')}
      } catch(e){if(live)setError(String(e))} finally{busy=false}
    }
    void refresh();const timer=setInterval(()=>void refresh(),1500)
    return()=>{live=false;view.current++;clearInterval(timer)}
  },[scenario,project])
  const virtual = useVirtualizer({count:history.length,getScrollElement:()=>scroll.current,estimateSize:()=>56,overscan:6,initialRect:{width:480,height:400}})
  const detail=history.find(r=>r.id===selected)
  const groups=useMemo(()=>comparisons(snapshot.sessions),[snapshot.sessions])
  const loadMore=async()=>{
    const version=view.current
    setLoading(true)
    try{const s=await decisionApi.snapshot(history.length,200,scenario==='all'?null:scenario,project==='all'?null:project);if(view.current===version)setHistory(h=>mergeHistory(h,s.history))}catch(e){if(view.current===version)setError(String(e))}finally{setLoading(false)}
  }
  return <Modal onDismiss={onDismiss}>
    <Dialog title="Decision Insights" width="wide" lead="Experimental · this window only" onClose={onDismiss}
      onKeyDown={e=>{if(e.key==='Escape'){e.stopPropagation();onDismiss()}}}
      footNote={`${snapshot.dropped.toLocaleString()} older records evicted · cards cover this window · p50/p95 from the last 2,048 decisions per scenario`}
      actions={<><Button onClick={()=>{void decisionApi.clear().then(()=>{view.current++;setSnapshot(EMPTY);setHistory([]);setSelected(null)}).catch(e=>setError(String(e)))}}>Clear history</Button><Button ref={close} variant="primary" onClick={onDismiss}>Close</Button></>}>
      {error&&<Banner tone="bad">{error}</Banner>}
      <div className={styles.cards}>
        {SCENARIOS.map(s=>{const v=snapshot.scenarios.find(v=>v.scenario===s.id);return <Card key={s.id} title={s.title}>
          <div className={styles.total}>{v?.decisions??0}</div>
          <span className={styles.muted}>{v?.applied??0} applied · {v?.observed??0} observed · {v?.errors??0} errors</span>
          <span className={styles.muted}>{v?.latencyP50Ms??0} / {v?.latencyP95Ms??0} ms</span>
          <span className={styles.muted}>{s.hint}</span>
        </Card>})}
      </div>
      <Tabs label="Decision insights views" value={tab} onChange={setTab} tabs={[{value:'history',label:'Decisions'},{value:'comparison',label:'Comparison'},{value:'sessions',label:'Sessions'}]} />
      {tab==='history'&&<>
        <Toolbar><Select label="Scenario filter" value={scenario} options={[{value:'all',label:'All scenarios'},...SCENARIOS.map(s=>({value:s.id,label:s.title}))]} onChange={setScenario} />
          <Select label="Project filter" value={project} options={[{value:'all',label:'All projects'},...Object.values(projects).map(p=>({value:p.id,label:p.name}))]} onChange={setProject} />
          <span className={styles.muted}>{history.length} / {snapshot.total} records</span>
        </Toolbar>
        <div className={styles.split}>
          <div ref={scroll} className={styles.history} aria-label="Decision history">
            {history.length===0?<Banner tone="info">No decisions match this view. Check the project and scenario filters, then adapter availability in Sessions. With the experiment and Research route enabled, a new user request triggers a decision; the other scenarios require the conditions shown above.</Banner>:
              <div style={{height:virtual.getTotalSize(),position:'relative'}}>{virtual.getVirtualItems().map(v=>{const r=history[v.index]!;return <div key={r.id} data-index={v.index} ref={virtual.measureElement} className={styles.virtualRow} style={{transform:`translateY(${v.start}px)`}}>
                <Button variant="secondary" onClick={()=>setSelected(r.id)} aria-pressed={selected===r.id}>
                  <span>{new Date(r.atMs).toLocaleTimeString()} · {r.scenario} · {r.choice||'Unavailable'}</span>
                  <Badge soft tone={r.status==='applied'?'green':r.status==='error'?'red':'neutral'}>{r.status}</Badge>
                </Button>
              </div>})}</div>}
          </div>
          <div className={styles.details}>
            {detail?<>
              <Summary rows={[{label:'Project / harness',value:`${projects[detail.project]?.name??detail.project} · ${detail.harness}`},{label:'Session',value:detail.session},{label:'Period / group',value:`${detail.period} · ${detail.group}`},{label:'Status',value:detail.status},{label:'Policy / delivery',value:detail.reason},{label:'Observed outcome',value:detail.outcome||'Not observed yet'},{label:'Request',value:`${detail.latencyMs} ms · ${detail.inputTokens??'—'} input tokens`}]} />
              <p>{detail.question}</p>
              {detail.intervention ? <>
                <Card title="1. Decide whether to intervene">
                  <Progress label="Intervene" value={detail.intervention.probability} />
                  <Progress label="No intervention" value={1-detail.intervention.probability} />
                  <Summary rows={[
                    {label:'Threshold',value:`${detail.intervention.threshold}%`},
                    {label:'Gate',value:detail.intervention.passed?'Passed · delivery status shown above':'Not passed · no intervention'},
                  ]} />
                  <p className={styles.muted}>Inspect and change approach are combined from the same model response.</p>
                </Card>
                {Object.keys(detail.intervention.choices).length>0&&<Card title="2. Choose a response">
                  {Object.entries(detail.intervention.choices).sort((a,b)=>b[1]-a[1]).map(([label,p])=><Progress key={label} label={label} value={p} />)}
                  <p className={styles.muted}>Shares among intervention options; no second threshold. The preferred response is used only if the gate and delivery policy allow it.</p>
                </Card>}
                <Disclosure title="Original model distribution">
                  {Object.entries(detail.probabilities).sort((a,b)=>b[1]-a[1]).map(([label,p])=><Progress key={label} label={label} value={p} />)}
                </Disclosure>
              </> : Object.entries(detail.probabilities).sort((a,b)=>b[1]-a[1]).map(([label,p])=><Progress key={label} label={label} value={p} />)}
              <p className={styles.muted}>Probabilities describe the model’s distribution, not measured correctness. The policy reason above comes from cide.</p>
              {detail.action&&<Disclosure title="Action"><CodeBlock>{detail.action}</CodeBlock></Disclosure>}
              <Disclosure title="State sent to the decision model"><CodeBlock>{detail.state}</CodeBlock></Disclosure>
              <Toolbar><span>Was this useful?</span>{[true,false].map(helpful=><Button key={String(helpful)} aria-pressed={detail.feedback===helpful} onClick={()=>{void decisionApi.feedback(detail.id,helpful).then(()=>setHistory(h=>h.map(r=>r.id===detail.id?{...r,feedback:helpful}:r))).catch(e=>setError(String(e)))}}>{helpful?'Helpful':'Not helpful'}</Button>)}</Toolbar>
            </>:<Banner tone="info">Select a decision to inspect its input, probabilities and delivery outcome.</Banner>}
          </div>
        </div>
        {history.length<snapshot.total&&<Button disabled={loading} onClick={()=>void loadMore()}>{loading?'Loading…':'Load older decisions'}</Button>}
      </>}
      {tab==='comparison'&&<>
        <Banner tone="info">These are observed totals, not estimated savings. Different projects and tasks are not directly comparable. Passive sessions have partial turn/tool coverage; compare native A/B groups for consistent instrumentation. Missing usage is shown as —.</Banner>
        <div className={styles.table}><Table rows={groups} rowKey={r=>[r.project,r.harness,r.group,r.period].join(':')} columns={[
          {key:'project',label:'Project',render:r=>projects[r.project]?.name??r.project},
          {key:'group',label:'Group / period',render:r=>`${r.harness} · ${r.group} · ${r.period}`},
          {key:'sessions',label:'Sessions',numeric:true,render:r=>r.sessions},
          {key:'coverage',label:'Coverage',render:r=>r.native===r.sessions?'Native events':`${r.native} native / ${r.sessions-r.native} passive`},
          {key:'turns',label:'Observed turns',numeric:true,render:r=>r.turns},
          {key:'tools',label:'Tools / failed',numeric:true,render:r=>`${r.tools} / ${r.failures}`},
          {key:'time',label:'Active time',numeric:true,render:r=>`${(r.elapsed/1000).toFixed(1)} s`},
          {key:'tokens',label:'Input / output tokens',numeric:true,render:r=>`${r.input??'—'} / ${r.output??'—'}`},
          {key:'cost',label:'Reported cost',numeric:true,render:r=>r.cost===null?'—':r.cost.toFixed(4)},
        ]} /></div>
      </>}
      {tab==='sessions'&&<div className={styles.table}><Table rows={snapshot.sessions} rowKey={r=>`${r.session}:${r.period}`} columns={[
        {key:'project',label:'Project / session',render:r=>`${projects[r.project??'']?.name??'Project'} · ${r.session.slice(0,8)}`},
        {key:'harness',label:'Harness',render:r=>r.harness},
        {key:'enabled',label:'Experiment',render:r=>r.enabled?r.group:'Off / baseline'},
        {key:'available',label:'Adapter',render:r=>r.note},
        {key:'auto',label:'Model selection',render:r=><Button disabled={!r.available||!r.enabled} onClick={()=>{void decisionApi.auto(r.session,r.pinned).catch(e=>setError(String(e)))}}>{r.pinned?'Pinned · select Auto':'Auto · pin current'}</Button>},
      ]} /></div>}
    </Dialog>
  </Modal>
}
