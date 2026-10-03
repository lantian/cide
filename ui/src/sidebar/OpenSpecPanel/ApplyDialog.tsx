/**
 * *Apply…*: implement one change in a session of its own. (OpenSpec sessions)
 *
 * The user's gesture, replacing *Start work*: choose the main model or a subagent, and the change
 * is applied in a new tab — **no task is created**. The tab is a mirror onto a run the registry
 * owns (`specRuns.ts`), so it can be closed while the work goes on; the row's chip shows its state
 * and *Open session* brings the tab back.
 *
 * Where it works is the project's `openspec.applyInWorktree` (the panel's gear): said here, before
 * the press, because the answer decides what the finished change offers — Publish and Integrate
 * for a worktree's branch, Archive alone for the project root.
 *
 * Rendered once by `App.tsx`, like `ProposeDialog`, so it works whichever panel is showing.
 */
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { Modal } from '@/overlays/ModalShell'
import { Button } from '@/kit/components/Button'
import { Dialog } from '@/kit/components/Overlay'
import { Field, Textarea } from '@/kit/components/Field'
import { Code } from '@/kit/components/Status'
import { notifyFailure } from '@/chrome/notices'
import { specSessions, type ProjectId } from '@/ipc/client'
import type { SpecLauncher } from '@/ipc/generated'
import { DEFAULT_CHOICE, LauncherPicker, type LauncherChoice } from './LauncherPicker'
import { startSession } from './specRuns'
import { useApplyDialog } from './applyStore'

export { useApplyDialog }

export function ApplyDialog({ project }: { project: ProjectId | null }) {
  const change = useApplyDialog((state) => state.change)
  const close = useApplyDialog((state) => state.close)
  const [choice, setChoice] = useState<LauncherChoice>(DEFAULT_CHOICE)
  const [resolved, setResolved] = useState<{ launcher: SpecLauncher } | { why: string }>({
    why: 'Reading…',
  })
  const [text, setText] = useState('')
  const [busy, setBusy] = useState(false)
  const [worktree, setWorktree] = useState<boolean | null>(null)
  const instructions = useRef<HTMLTextAreaElement>(null)

  // The host stays mounted while closed, so focus follows each opening. Set it before paint
  // so an immediate key reaches Apply; keep any focus a descendant already established.
  useLayoutEffect(() => {
    if (change === null) return
    const target = instructions.current
    const dialog = target?.closest('[role="dialog"]')
    if (dialog && !dialog.contains(document.activeElement)) target?.focus()
  }, [change])

  useEffect(() => {
    if (project === null || change === null) return
    let alive = true
    specSessions.settings(project).then(
      (settings) => alive && setWorktree(settings.applyInWorktree),
      () => alive && setWorktree(null),
    )
    return () => {
      alive = false
    }
  }, [project, change])

  const dismiss = useCallback(() => {
    if (busy) return
    setText('')
    close()
  }, [busy, close])

  const onApply = useCallback(() => {
    if (project === null || change === null || !('launcher' in resolved)) return
    setBusy(true)
    void startSession(project, {
      op: 'apply',
      change,
      ...(text.trim() === '' ? {} : { text }),
      launcher: resolved.launcher,
    })
      .then(() => {
        setBusy(false)
        setText('')
        close()
      })
      .catch((error: unknown) => {
        // The dialog stays up with the choice in it; `Failures` shows the sentence.
        setBusy(false)
        notifyFailure(error)
      })
  }, [project, change, resolved, text, close])

  if (change === null) return null
  const ready = 'launcher' in resolved && !busy
  return (
    <Modal onDismiss={dismiss}>
      <Dialog
        data-audit="openspecApplyDialog"
        onKeyDown={(event) => {
          // A launcher popup consumes Escape by preventing default before it bubbles here.
          if (event.key !== 'Escape' || event.defaultPrevented) return
          event.preventDefault()
          event.stopPropagation()
          dismiss()
        }}
        title={`Apply ${change}`}
        lead="Implement this change in a new tab. Linked Todo tasks move to Doing. You can close the tab and open the session again from OpenSpec."
        onClose={dismiss}
        footNote={
          'why' in resolved && choice.mode === 'role'
            ? resolved.why
            : worktree === null
              ? undefined
              : worktree
                ? (
                    <>
                      In its own worktree, on <Code>{`cide/spec-${change}`}</Code>
                    </>
                  )
                : 'In the project root — the gear on the OpenSpec panel can put it in a worktree'
        }
        actions={
          <>
            <Button data-audit="openspecApplyCancel" disabled={busy} onClick={dismiss}>
              Cancel
            </Button>
            <Button
              variant="primary"
              data-audit="openspecApplyStart"
              data-write="true"
              busy={busy}
              disabled={!ready}
              onClick={onApply}
            >
              Apply
            </Button>
          </>
        }
      >
        <LauncherPicker
          choice={choice}
          onChange={setChoice}
          allowRoles
          onResolved={setResolved}
        />
        <Field label="Instructions" optional hint="Typed after the apply command, on one line.">
          {({ id, describedBy }) => (
            <Textarea
              ref={instructions}
              id={id}
              aria-describedby={describedBy}
              rows={3}
              value={text}
              disabled={busy}
              placeholder="Anything to add — e.g. keep the old API working until the migration lands."
              onChange={(event) => setText(event.target.value)}
            />
          )}
        </Field>
      </Dialog>
    </Modal>
  )
}
