/**
 * The Docker panel's barrel. (M41)
 *
 * The **host** is exported under the plain name, so `App.tsx` imports `DockerPanel` and gets the
 * wired one; the pure view is `DockerPanelView`, which is what the render check drives.
 * `adapt.ts` is deliberately not re-exported — it is the one module allowed to import the
 * generated wire types, and `model.ts` must stay compilable standalone.
 */
export { DockerPanel } from './DockerPanelHost'
export { DockerPanel as DockerPanelView, type DockerPanelProps } from './DockerPanel'
export {
  BOARD_UNKNOWN,
  actionsFor,
  groupByCompose,
  isRunning,
  newerBoard,
  runningCount,
  type Action,
  type Board,
  type Container,
  type Image,
} from './model'
