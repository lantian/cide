/**
 * The panel's public face. (M28)
 *
 * `model.ts` is deliberately **not** re-exported. A barrel entry for it would put React one
 * import away from a module that `check:openspec` compiles standalone with a bare `tsc`, and the
 * first time somebody imported the barrel instead of the file the standalone compile would start
 * failing for a reason with nothing to do with the model. `AgentsPanel/index.ts` says the same.
 */
export { OpenSpecPanel } from './OpenSpecPanelHost'
export { OpenSpecPanelView } from './OpenSpecPanel'
export type { OpenSpecPanelViewProps } from './OpenSpecPanel'
