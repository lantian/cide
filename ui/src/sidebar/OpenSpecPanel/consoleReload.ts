/**
 * Restart the project console after `openspec init`, so the conversation knows the commands
 * the init just installed. (M28)
 *
 * # The defect this replaces
 *
 * Claude Code reads a project's skills **once, at startup**. Setting OpenSpec up writes
 * `.claude/skills/openspec-<name>/SKILL.md` into a project whose console pane came up with the
 * window — so every command the panel could then type was refused by `spec_run_command` until
 * the user restarted the pane by hand. What shipped was a notice *telling* them to, and the
 * report on it was the obvious question: cide knows which pane, and owns its respawn
 * (`panes/paneRestart.ts`), so why is the user the one doing it?
 *
 * # What is restarted, and how
 *
 * The **console pane only** — `tabs[0]`'s Claude pane, the one whose session is the project's
 * `primary_session` and therefore the one every command this panel types lands in
 * (`spec_run_command` sends there and nowhere else). Other Claude panes keep their
 * conversations: killing a fork somebody is mid-thought in, for a skill that pane may never
 * type, would trade one report for a worse one.
 *
 * **Resume when a transcript exists, fresh when none does.** `restart('resume')` hands the old
 * id back to `claude --resume`, which re-reads the skills — a resume *is* a process launch —
 * and keeps the conversation, so the reload costs the user nothing they can see except a few
 * seconds. `canResume` is asked first because a `--resume` for a transcript that was never
 * written is a spawn that fails after the button was pressed; a conversation with no turns on
 * disk loses nothing to `fresh`.
 *
 * # Both doors go through here
 *
 * The absent screen's *Set up OpenSpec* and the config dialog's init wizard are the same
 * gesture behind different buttons, and a reload wired to one of them would leave the other
 * ending at the very refusal this exists to remove. The Rust refusal itself stays: it is the
 * backstop for every road this cannot cover — a project set up from a terminal, an
 * `openspec update` run outside cide, a detached console whose restarter lives in another
 * window's realm.
 */
import { notify } from '@/chrome/notices'
import { errorText } from '@/ipc/errorText'
import type { ProjectId } from '@/ipc/client'
import { consolePaneOfProject } from '@/keys/target'
import { paneRestarter } from '@/panes/paneRestart'
import { useWorkspace } from '@/store/workspace'
import { SETUP_RELOAD_FAILED, setUpNotice } from './model'

/**
 * Restart the console so the just-installed skills are read, and say what happened.
 *
 * Never rejects — the words for every arm, the failed respawn included, are `model.ts`'s, and
 * a caller that had to add its own catch would be a caller that could forget it. The awkward
 * arm is deliberate: `null` project (a panel with no project cannot have run init, but the
 * type allows it) falls through to the manual sentence rather than throwing.
 */
export async function reloadConsoleAfterSetUp(project: ProjectId | null): Promise<void> {
  const pane =
    project === null ? null : consolePaneOfProject(useWorkspace.getState().boot, project)
  const restarter = pane === null ? null : paneRestarter(pane)
  if (restarter === null) {
    notify(setUpNotice('unreachable'), { kind: 'ok' })
    return
  }
  try {
    // A refused answer counts as "no transcript": the safe direction is a fresh spawn, which
    // works for every pane a resume would also have worked for.
    const resumable = await restarter.canResume().catch(() => false)
    await restarter.restart(resumable ? 'resume' : 'fresh')
    notify(setUpNotice(resumable ? 'resumed' : 'restarted'), { kind: 'ok' })
  } catch (error) {
    notify(SETUP_RELOAD_FAILED, { kind: 'warn', detail: errorText(error) })
  }
}
