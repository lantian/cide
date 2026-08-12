/**
 * Mounts whichever overlay is open, and nothing when none is.
 *
 * One component so `App` has a single line to add and one place to hand the actions to.
 * Neither overlay is kept mounted-but-hidden: the picker holds a live Rust-side session with
 * a walker behind it, and keeping that alive for a modal nobody is looking at is exactly the
 * kind of quiet resource leak `paneHosts.ts` exists to avoid for terminals. Unmounting runs
 * `picker_close`.
 */
import { useEffect } from 'react'
import { BranchPopup } from '@/chrome/BranchSelector'
import { CommandPalette } from './CommandPalette'
import { FilePicker } from './FilePicker'
import { useOverlays } from './store'
import type { KeyContext, Keymap } from '@/keys/keymap'
import type { Command, ProjectId } from '@/ipc/client'

export interface OverlayActions {
  /** ⏎ in the picker. */
  openFile: (path: string) => void
  /** ⇧⏎ in the picker. */
  openFileInSplit: (path: string) => void
  /** ⌥⏎ in the picker — `claude.mention.file` against the focused Claude pane. */
  mentionFile: (path: string) => void
  /** ⏎ in the palette. The same dispatcher the key gate calls. */
  runCommand: (id: string) => void
  /** ⌃⏎ in the palette. */
  runCommandInNewSession: (id: string) => void
}

export interface OverlayHostProps {
  /** `null` when no project is open: the picker has nothing to index, so it is not offered. */
  project: ProjectId | null
  commands: readonly Command[]
  keymap: Keymap
  context: KeyContext
  actions: OverlayActions
}

export function OverlayHost({ project, commands, keymap, context, actions }: OverlayHostProps) {
  const open = useOverlays((s) => s.open)
  const close = useOverlays((s) => s.close)

  /*
   * A picker with no project would open a session against nothing and show an empty list with
   * no explanation, so it is closed rather than merely not rendered.
   *
   * Rendering `null` and leaving the store open was the obvious alternative and it is wrong
   * twice: `overlayOpen()` would report a modal nobody can see — gating every `!overlayOpen`
   * binding off invisibly — and the next Ctrl+P would *toggle* that phantom shut, so the user
   * has to press the key twice before anything can happen.
   */
  useEffect(() => {
    if (open === 'files' && project === null) close()
  }, [open, project, close])

  if (open === null) return null

  if (open === 'files') {
    // Until the effect above runs. One frame, and it draws nothing.
    if (project === null) return null
    return (
      <FilePicker
        project={project}
        onDismiss={close}
        onOpen={actions.openFile}
        onOpenInSplit={actions.openFileInSplit}
        onMention={actions.mentionFile}
      />
    )
  }

  /*
   * The branch popup takes no props: it reads the window's project from the workspace mirror
   * and its rows from its own store, because the same popup is opened from the status bar
   * widget and from `git.branch.switch` in the palette — and the palette dispatches from
   * outside React, where there is nothing to hand props from.
   */
  if (open === 'branches') return <BranchPopup onDismiss={close} />

  return (
    <CommandPalette
      commands={commands}
      keymap={keymap}
      context={context}
      onDismiss={close}
      onRun={actions.runCommand}
      onRunInNewSession={actions.runCommandInNewSession}
    />
  )
}
