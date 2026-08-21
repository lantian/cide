/**
 * The Extensions panel's barrel. (M22)
 *
 * The **Host** is exported under the plain name, so `App.tsx` renders the wired component and
 * cannot accidentally render the pure view with no props.
 *
 * `model.ts` is deliberately **not** re-exported here. `ui/scripts/check-ext.mjs` compiles
 * it standalone with a bare `tsc`, and a barrel that pulled it in beside a `.tsx` would make that
 * impossible — the same rule the git, problems, tasks and agents barrels all state.
 */
export { ExtensionsPanel } from './ExtensionsPanelHost'
export { ExtensionsPanelView } from './ExtensionsPanel'
