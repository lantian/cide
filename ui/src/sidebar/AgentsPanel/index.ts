/**
 * The ◍ Agents sidebar view. (M18)
 *
 * ## What is here now
 *
 * The pure view only. `AgentsPanelView` reads no store, calls no IPC and does not read the
 * clock — every fact and every gesture arrives by prop, `nowMs` included. That is what lets
 * `ui/scripts/check-agents-render.mjs` render it through `react-dom/server` and assert on the
 * markup, which is the only gate that can see a `styles.x` naming a rule the stylesheet does
 * not define: it type-checks, evaluates to `undefined`, and React drops the attribute in
 * silence.
 *
 * The stateful half is `AgentsPanelHost`, exported here as **`AgentsPanel`** — the name a host
 * imports, the way `GitPanel` comes from `GitPanelHost` and `TasksPanel` from `TasksPanelHost`.
 * It reads `sidebar/agentsStore.ts` and `sidebar/tasksStore.ts` (the agent→task link needs both)
 * and hands the view everything as props, `nowMs` included, so the property above stays true.
 * `adapt.ts` is the seam allowed to import the generated wire types and is **not** re-exported:
 * it has one caller, the store, and a barrel entry would only offer a second way to reach it.
 *
 * The `cide://agents-changed` subscription is **not** here and not in the host. It lives in
 * `App.tsx`, for `gitCountStore`'s stated reason: the activity rail's ⌬ badge has to stay live
 * while the sidebar is shut or showing Files, and a listener registered inside a panel goes
 * stale the moment that panel unmounts.
 *
 * ## `model.ts` is deliberately not re-exported
 *
 * `ui/scripts/check-agents.mjs` compiles that module **alone**, with a bare `tsc` and no
 * tsconfig, and imports the emitted JS under node. Re-exporting it through this barrel would
 * put React on the far side of one import away from it, and any consumer that reached for a
 * model function through `@/sidebar/AgentsPanel` would drag the whole component graph — and
 * the DOM — into a check whose entire value is that it needs neither. `ProblemsPanel/index.ts`
 * states the same rule for the same reason; import from `'./model'` or
 * `'@/sidebar/AgentsPanel/model'` directly.
 */
export { AgentsPanel, type AgentsPanelProps } from './AgentsPanelHost'
export { AgentsPanelView, type AgentsPanelViewProps } from './AgentsPanel'
/*
 * `RunRow` and not `ActivityRow`. `RunRow.tsx` draws a run two ways — as a record, which is what
 * Recent lists, and as a line under the role that is running it — and only the first is a
 * component anything outside this directory could want. The second is meaningless without the
 * role row above it, which is `AgentsPanel.tsx`'s and is not exported either.
 */
export { RunRow, type RunRowProps } from './RunRow'
