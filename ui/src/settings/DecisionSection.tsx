import { useState } from 'react'
import type { DecisionSettings, ProjectDecisionSettings, ProjectHarnessEdit, ProjectHarnessSettings } from '@/ipc/generated'
import { Group, Row, ToggleRow, TextField, TextArea, NumberField, Select, InlineControls, ActionButton } from './controls'
import { Banner } from '@/kit/components/Feedback'
import { SCENARIOS, projectDefaults } from '@/decision/model'
import { showOverlay } from '@/overlays/store'

export function DecisionSection({ settings, patch, project, edit, probe }: {
  settings: DecisionSettings; patch: (settings: DecisionSettings) => void;
  project?: ProjectHarnessSettings | undefined; edit?: ((edit: ProjectHarnessEdit) => void) | undefined;
  probe?: ((settings: DecisionSettings) => Promise<string>) | undefined;
}) {
  const config = project?.decision ?? projectDefaults()
  const set = (value: Partial<ProjectDecisionSettings>) => edit?.({ field:'decision', value:{...config,...value} })
  const [status,setStatus] = useState('')
  const [testing,setTesting] = useState(false)
  return <>
    <Banner tone="info">Experimental assistance for Codex and OpenCode 2. Each project starts disabled. New launches and resumed sessions receive the adapter.</Banner>
    <Group title="This project">
      {project === undefined ? <Banner tone="info">Open a project to configure this experiment.</Banner> : <>
        <ToggleRow label="Enable decision assistance" hint="Applies only to this project, including its agent worktrees." checked={config.enabled} onChange={enabled=>set({enabled})} />
        {SCENARIOS.map(s=><Row key={s.id} label={s.title} hint={s.hint} info={s.id==='loop'?'The threshold applies to the combined probability of inspecting the error or changing approach. The more likely response is then selected from the same model result.':undefined} control={<InlineControls>
          <Select label={`${s.title} mode`} value={config.scenarios[s.id]?.mode ?? 'automatic'} options={[{value:'off',label:'Off'},{value:'observe',label:'Observe'},{value:'automatic',label:'Automatic'}]} onChange={mode=>set({scenarios:{...config.scenarios,[s.id]:{mode,threshold:config.scenarios[s.id]?.threshold ?? (s.id==='completion'?90:80)}}})} />
          <NumberField label={`${s.title} probability threshold (%)`} value={config.scenarios[s.id]?.threshold ?? (s.id==='completion'?90:80)} min={50} max={100} onChange={threshold=>set({scenarios:{...config.scenarios,[s.id]:{mode:config.scenarios[s.id]?.mode ?? 'automatic',threshold}}})} />
        </InlineControls>} />)}
        <Row label="Decision Insights" hint="History and metrics live only for the lifetime of this window." control={<ActionButton label="Open insights" onClick={()=>showOverlay('decisionInsights')} />} />
      </>}
    </Group>
    <Group title="Ollama connection · all projects">
      <Row label="API URL" hint="System One endpoint; Ollama 0.35 or later is required." control={<TextField label="Decision API URL" value={settings.url} onCommit={url=>patch({...settings,url})} />} />
      <Row label="Decision model" control={<TextField label="Decision model" value={settings.model} onCommit={model=>patch({...settings,model})} />} />
      <Row label="Timeout (ms)" hint="Time budget for runtime decisions. The connection test allows longer for model loading." control={<NumberField label="Decision timeout" value={settings.timeoutMs} min={100} max={30000} onChange={timeoutMs=>patch({...settings,timeoutMs})} />} />
      <Row label="Connection test" hint="Allows 120 seconds for model loading, then checks warm latency. No model downloads." control={<ActionButton label={testing?'Testing…':'Test connection'} disabled={testing||!probe} onClick={()=>{if(!probe)return;setStatus('');setTesting(true);void probe(settings).then(setStatus,e=>setStatus(String(e))).finally(()=>setTesting(false))}} />} />
      {status && <Banner tone="info">{status}</Banner>}
    </Group>
    <Group title="Permitted model profiles">
      <Banner tone="info">Profiles are shared; permission to use each profile is local to this project. OpenCode models use provider/model. An explicit session model stays pinned until you select Auto in Insights.</Banner>
      {settings.profiles.map((p,i)=><Group key={i} title={p.id||'New profile'}>
        <Row label="Name" control={<TextField label={`Profile ${i+1} name`} value={p.id} onCommit={id=>patch({...settings,profiles:settings.profiles.map((v,j)=>j===i?{...v,id}:v)})} />} />
        <Row label="Harness" control={<Select label={`Profile ${i+1} harness`} value={p.harness} options={[{value:'codex',label:'Codex'},{value:'opencode',label:'OpenCode'}]} onChange={harness=>patch({...settings,profiles:settings.profiles.map((v,j)=>j===i?{...v,harness}:v)})} />} />
        <Row label="Model" control={<TextField label={`Profile ${i+1} model`} value={p.model} onCommit={model=>patch({...settings,profiles:settings.profiles.map((v,j)=>j===i?{...v,model}:v)})} />} />
        <Row label="Effort / variant" control={<TextField label={`Profile ${i+1} effort`} value={p.effort??''} onCommit={effort=>patch({...settings,profiles:settings.profiles.map((v,j)=>j===i?{...v,effort:effort||null}:v)})} />} />
        <Row label="When to use" hint="Describe the tasks and relative cost of this profile." control={<TextField label={`Profile ${i+1} description`} value={p.description} onCommit={description=>patch({...settings,profiles:settings.profiles.map((v,j)=>j===i?{...v,description}:v)})} />} />
        {project && <ToggleRow label="Allow in this project" checked={config.profiles.includes(p.id)} onChange={enabled=>set({profiles:enabled?[...config.profiles,p.id]:config.profiles.filter(id=>id!==p.id)})} />}
        <ActionButton label="Remove profile" onClick={()=>patch({...settings,profiles:settings.profiles.filter((_,j)=>j!==i)})} />
      </Group>)}
      <ActionButton label="Add profile" disabled={settings.profiles.length>=24} onClick={()=>patch({...settings,profiles:[...settings.profiles,{id:`profile-${Date.now()}`,harness:'codex',model:'default',effort:null,description:''}]})} />
    </Group>
    {project && <>
      <Group title="Verification and limits">
        <ToggleRow label="Discover checks" hint="Read package scripts, Cargo, Go and Python project markers." checked={config.discoverChecks} onChange={discoverChecks=>set({discoverChecks})} />
        <Row label="Additional checks" hint="One command per line. Prefix with ! to keep a required check." info="Required checks remain outstanding after edits until they succeed, regardless of the Verification result. Completion evaluates them using its own mode and threshold." control={<TextArea label="Verification commands" value={config.checks.map(c=>`${c.required?'! ':''}${c.command}`).join('\n')} onCommit={text=>set({checks:text.split('\n').filter(s=>s.trim()).map(s=>({command:s.replace(/^!\s*/,''),required:s.startsWith('!'),description:''}))})} />} />
        <Row label="Maximum continuations" control={<NumberField label="Maximum continuations" value={config.maxContinuations} min={0} max={2} onChange={maxContinuations=>set({maxContinuations})} />} />
        <Row label="Loop cooldown (seconds)" control={<NumberField label="Loop cooldown" value={config.cooldownSecs} min={5} max={600} onChange={cooldownSecs=>set({cooldownSecs})} />} />
        <Row label="Context candidates retained" control={<NumberField label="Context limit" value={config.contextLimit} min={1} max={32} onChange={contextLimit=>set({contextLimit})} />} />
        <Row label="Decision input limit (bytes)" control={<NumberField label="Decision input limit" value={config.maxInputBytes} min={1024} max={24576} onChange={maxInputBytes=>set({maxInputBytes})} />} />
      </Group>
      <Group title="A/B comparison">
        <ToggleRow label="Assign sessions to control or treatment" hint="50/50 assignment; related sessions stay in the same group." checked={config.abEnabled} onChange={abEnabled=>set({abEnabled})} />
        <Row label="Scenario to compare" hint="Configuration changes begin a new measurement period." control={<Select label="A/B scenario" value={config.abScenario??'all'} options={[{value:'all',label:'All scenarios'},...SCENARIOS.map(s=>({value:s.id,label:s.title}))]} onChange={v=>set({abScenario:v==='all'?null:v})} />} />
      </Group>
    </>}
  </>
}
