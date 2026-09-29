/**
 * *Propose a change* and *Explore first*, as a modal. (M28)
 *
 * # Why it left the panel
 *
 * It was a box that grew inside the sidebar, under the toolbar and above the tree. Two things
 * were wrong with that and the second is the one that decided it.
 *
 * The sidebar is a **column somebody has dragged to the width they want their tree at**, and this
 * is free prose — the description of a change, which is the most consequential sentence a user of
 * this feature ever types. Three rows of a 320px column is the wrong place to write it.
 *
 * And the gesture had exactly one door. Proposing a change is the single most common thing anyone
 * does with OpenSpec, and it was reachable only by opening the sidebar, switching it to OpenSpec,
 * and finding a button — so it is a *command* now (`spec.propose`, `spec.explore`), which puts it
 * in the palette on Ctrl+Shift+P beside everything else. A command needs somewhere to draw that
 * does not assume the panel is on screen, and that is a modal.
 *
 * # What did not change
 *
 * The line that will be typed is still shown verbatim, still **read from the project** and never
 * spelled here: OpenSpec moved `/opsx:propose` to `/openspec-propose`, and a surface with the old
 * one baked in previewed a line no project had. It is also still the only place a user can see
 * that their three-line description arrives as one turn.
 *
 * # Where it goes now (OpenSpec sessions)
 *
 * Not into the project's console any more. Send starts a **session of its own** — an agent run on
 * the harness picked here (Default is Settings → Harness, the MR reviewer's choice), in a new tab
 * that can be closed while the work goes on; the OpenSpec panel shows its state and opens it
 * again. `specRuns.ts` and `cmd::spec_sessions` carry the rest.
 *
 * And the box still opens rather than the button just sending, for the reason it always did: both
 * commands take free text — *"the change name, OR a description of what the user wants to
 * build"* — and a bare one is legal and useless, because Claude answers by asking what to
 * propose, which is a round trip we had the user's attention for and gave away.
 */
import { create } from 'zustand'
import { useCallback, useState } from 'react'
import { OverlayCard } from '@/overlays/ModalShell'
import { notifyFailure } from '@/chrome/notices'
import type { ProjectId } from '@/ipc/client'
import type { SpecLauncher } from '@/ipc/generated'
import { useSpec } from '../specStore'
import { ProposeFormView } from './ProposeForm'
import { askTitle, invocation } from './model'
import { DEFAULT_CHOICE, LauncherPicker, type LauncherChoice } from './LauncherPicker'
import { startSession } from './specRuns'

/**
 * Which command the composer is open on, or `null`.
 *
 * A store and not panel state, because there are now three doors — the panel's two buttons and
 * the palette — and two of them exist whether or not the panel is mounted. `App.tsx` renders the
 * dialog once, at top level, so the command works from a window whose sidebar is showing Files.
 */
interface ProposeStore {
  command: string | null
  open: (command: string) => void
  close: () => void
}

export const useProposeDialog = create<ProposeStore>((set) => ({
  command: null,
  open: (command) => set({ command }),
  close: () => set({ command: null }),
}))

/**
 * The modal, and the send.
 *
 * Rendered by `App.tsx` at top level so the palette command works with the sidebar shut or
 * showing something else. Draws nothing until a command is chosen.
 */
export function ProposeDialog({ project }: { project: ProjectId | null }) {
  const command = useProposeDialog((state) => state.command)
  const close = useProposeDialog((state) => state.close)
  const board = useSpec((state) => state.board)
  const [text, setText] = useState('')
  const [busy, setBusy] = useState(false)

  const [choice, setChoice] = useState<LauncherChoice>(DEFAULT_CHOICE)
  const [resolved, setResolved] = useState<{ launcher: SpecLauncher } | { why: string }>({
    why: 'Reading…',
  })

  /*
   * A session of its own, in a new tab — no longer a line typed into the project's console.
   *
   * The console road (`spec_run_command`) put every proposal into the one conversation the user
   * is also driving, and the answer arrived on a tab behind the one on screen. A session is an
   * agent run (`cmd::spec_sessions`): the tab `startSession` opens is a mirror onto it, so it can
   * be closed while the proposal is written, and the panel shows its state and opens it again.
   * The harness is the user's choice here, as in the MR reviewer — Default is Settings → Harness.
   */
  const onSend = useCallback(() => {
    if (project === null || command === null || !('launcher' in resolved)) return
    if (command !== 'propose' && command !== 'explore') return
    setBusy(true)
    void startSession(project, {
      op: command,
      ...(text.trim() === '' ? {} : { text }),
      launcher: resolved.launcher,
    })
      .then(() => {
        setBusy(false)
        setText('')
        close()
      })
      .catch((error: unknown) => {
        // The box stays up with the typing in it. A refusal that closed the composer would throw
        // away the description it failed to send — `RequirementEditor`'s rule, one file over.
        setBusy(false)
        notifyFailure(error)
      })
  }, [project, command, text, close, resolved])

  if (command === null) return null
  return (
    <OverlayCard label={askTitle(command)} onDismiss={busy ? () => {} : close}>
      <ProposeFormView
        command={command}
        line={invocation(board, command)}
        text={text}
        busy={busy}
        onText={setText}
        onSend={'launcher' in resolved ? onSend : () => {}}
        picker={
          <LauncherPicker
            choice={choice}
            onChange={setChoice}
            allowRoles={false}
            onResolved={setResolved}
          />
        }
        onCancel={() => {
          if (busy) return
          setText('')
          close()
        }}
      />
    </OverlayCard>
  )
}
