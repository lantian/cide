/**
 * The ☰ Tasks sidebar view. (M18)
 *
 * ## What is here now
 *
 * The pure view only. `TasksPanelView` and `TaskDetail` read no store, call no IPC and do not
 * read the clock — every fact and every gesture arrives by prop, `nowMs` included. That is what
 * lets `ui/scripts/check-agents-render.mjs` render them through `react-dom/server` with nothing
 * stubbed but `window`, which is the only gate that can see a `styles.x` naming a rule the
 * stylesheet does not define: it type-checks, evaluates to `undefined`, and React drops the
 * attribute in silence.
 *
 * The stateful half is `TasksPanelHost`, exported here as **`TasksPanel`** — the name a host
 * imports, the way `GitPanel` comes from `GitPanelHost`. It reads `sidebar/tasksStore.ts` and
 * hands the view everything as props, `nowMs` included, so the property above stays true.
 * `adapt.ts` is the seam allowed to import the generated wire types and is not re-exported: it
 * has one caller, the store, and a barrel entry would only offer a second way to reach it.
 *
 * The `cide://tasks-changed` subscription is **not** here and not in the host. It lives in
 * `App.tsx`, for `gitCountStore`'s stated reason: the activity rail's ☑ badge has to stay live
 * while the sidebar is shut or showing Files, and a listener registered inside a panel goes
 * stale the moment that panel unmounts.
 *
 * ## This panel does not depend on subagents being enabled
 *
 * Said in `model.ts` and in `TasksPanel.tsx`, and worth saying here too because this is the file
 * a wiring change is made in: a committed task list is useful on its own, orchestration is off
 * by default, and gating the tracker on it would make the first thing a curious user clicks say
 * "turn on a feature you have not read about".
 *
 * ## `model.ts` is deliberately not re-exported
 *
 * `ui/scripts/check-agents.mjs` compiles that module **alone**, with a bare `tsc` and no
 * tsconfig, and imports the emitted JS under node. Re-exporting it here would put React one
 * import away from it and let a consumer drag the whole component graph — and the DOM — into a
 * check whose entire value is that it needs neither. Import from `'./model'` or
 * `'@/sidebar/TasksPanel/model'` directly.
 */
export { TasksPanel, type TasksPanelProps } from './TasksPanelHost'
export { TasksPanelView, type TasksPanelViewProps } from './TasksPanel'
export { TaskDetail, type TaskDetailProps } from './TaskDetail'
