import type { DecisionScenario, ProjectDecisionSettings, DecisionRecord, DecisionMetrics } from '@/ipc/generated'
export const SCENARIOS: { id: DecisionScenario; title: string; hint: string }[] = [
  { id: 'loop', title: 'Repeated failures', hint: 'Suggest a different approach after three failures without progress.' },
  { id: 'context', title: 'Context ranking', hint: 'Runs when cide_context retrieves candidates; ordinary searches do not trigger ranking.' },
  { id: 'research', title: 'Research route', hint: 'At each user request, choose a research step or proceed without extra research. Also routes cide_context queries.' },
  { id: 'verification', title: 'Verification', hint: 'Evaluate once per observed revision and settings. Requires at least one unfinished check.' },
  { id: 'completion', title: 'Completion', hint: 'Unfinished required or accepted checks, or plan items; at most two extra turns.' },
  { id: 'model', title: 'Model and effort', hint: 'Choose only permitted profiles when the session is set to Auto.' },
]
export function projectDefaults(): ProjectDecisionSettings {
  return { enabled: false, scenarios: Object.fromEntries(SCENARIOS.map(s => [s.id, { mode: 'automatic', threshold: s.id === 'completion' ? 90 : 80 }])),
    profiles: [], checks: [], discoverChecks: true, abEnabled: false, abScenario: null, maxContinuations: 2, cooldownSecs: 30, maxInputBytes: 24576, contextLimit: 8 }
}
export function mergeHistory(current: DecisionRecord[], page: DecisionRecord[]): DecisionRecord[] {
  const map = new Map(current.map(r => [r.id, r]))
  for (const r of page) map.set(r.id, r)
  return [...map.values()].sort((a,b) => b.id-a.id).slice(0,10000)
}
export function comparisons(sessions: DecisionMetrics[]) {
  const groups = new Map<string, { project: string; harness: string; group: string; period: string; sessions: number; native: number; turns: number; tools: number; failures: number; elapsed: number; input: number | null; output: number | null; cost: number | null }>()
  for (const s of sessions) {
    const key = [s.project,s.harness,s.group,s.period].join(':')
    const v = groups.get(key) ?? { project: s.project ?? '', harness:s.harness, group:s.group, period:s.period, sessions:0, native:0, turns:0, tools:0, failures:0, elapsed:0, input:0, output:0, cost:0 }
    v.sessions++; v.turns+=s.turns; v.tools+=s.tools; v.failures+=s.failures; v.elapsed+=s.elapsedMs
    if (s.available) v.native++
    v.input=v.input===null||s.inputTokens===null?null:v.input+s.inputTokens
    v.output=v.output===null||s.outputTokens===null?null:v.output+s.outputTokens
    v.cost=v.cost===null||s.cost===null?null:v.cost+s.cost
    groups.set(key,v)
  }
  return [...groups.values()]
}
