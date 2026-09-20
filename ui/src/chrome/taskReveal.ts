/**
 * Open a task's card, from anywhere. (M60)
 *
 * # One producer for one gesture
 *
 * There are two roads to this now — a task chip in the Agents panel, and a `t-503` in a terminal
 * pane's output — and there will be more (a code in an editor buffer, in a diff, in an OpenSpec
 * document) because the id is printed everywhere. Two roads to one gesture that disagree is the
 * split `cide_git::push` already paid for; see `openPushDialog`. What would drift here first is
 * not the action, which is one line, but the *refusals*: the wording of a sentence a user sees
 * once a month, and — much worse — which of them are checked at all.
 *
 * The card itself is mounted by `App.tsx` on `tasksStore.selected`, so nothing here has to reveal
 * the Tasks panel or know whether the sidebar is open.
 */
import { useTasks } from '@/sidebar/tasksStore'
import { notifyFailure } from './notices'

/**
 * What a task link says when its task cannot be opened. Three sentences, because the three
 * failures are different facts: a board belonging to a project that is no longer on screen, a
 * board that cannot be read (the Tasks panel's `absent` and `unreadable` screens say why), and a
 * task that was deleted after the link was drawn — the Agents panel's row keeps drawing the id
 * on purpose, and a terminal's scrollback keeps it for ever, so the click can land on a task
 * that is gone. Either way the refusal goes on screen: a link that silently does nothing is the
 * failure this project has paid for most often.
 */
const BOARD_NOT_READABLE =
  'The task board cannot be read, so the task cannot be opened — the Tasks panel says why.'
const taskGone = (task: string) =>
  `${task} is not on the board any more — it was deleted after the link to it was drawn.`
/**
 * A task id is unique **within one project**, and every project can have a `t-503`.
 *
 * The link was offered against the board that was on screen when the pointer crossed the code.
 * Clicking it after a project switch — or clicking one in a parked pane belonging to another
 * project — would open *this* project's t-503: a different task, correctly rendered, with
 * nothing whatever to show it was the wrong one. So the caller names the project it drew the
 * link from and a mismatch is refused rather than resolved.
 */
const taskElsewhere = (task: string) =>
  `${task} belongs to a project that is not the one on screen. Switch to it and the task will open.`

/**
 * Open `task`'s card, or say why it cannot be opened.
 *
 * `project` is the project the caller drew the link from, and it is not optional: see
 * [`taskElsewhere`]. The store is read **fresh** rather than taking a board argument, because
 * one caller is a DOM callback on a link that may have been drawn minutes ago and has no render
 * to have captured one — and a caller passing its own board is exactly how the two roads would
 * come to disagree about the guard.
 */
export function revealTask(task: string, project: string | null): void {
  const state = useTasks.getState()
  if (project === null || project === '' || state.project !== project) {
    notifyFailure(taskElsewhere(task))
    return
  }
  if (state.board.kind !== 'ready') {
    notifyFailure(BOARD_NOT_READABLE)
    return
  }
  if (!state.board.tasks.some((candidate) => candidate.id === task)) {
    notifyFailure(taskGone(task))
    return
  }
  state.select(task)
}
