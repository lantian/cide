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
import { AboutCard } from './AboutCard'
import { PoolStateCard } from './PoolStateCard'
import { CommandPalette } from './CommandPalette'
import { FilePicker } from './FilePicker'
import { GoToLine } from './GoToLine'
import { ScratchType } from './ScratchType'
import { StructurePicker } from './StructurePicker'
import { SymbolPicker } from './SymbolPicker'
import { UsagesPopup } from './UsagesPopup'
import { focusedCaret } from '@/editor/caretTrack'
import { useOverlays } from './store'
import { useUsages } from './usagesStore'
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
  /**
   * ⏎ in either symbol picker: put the caret on a declaration.
   *
   * One callback for both, because the gesture is the same — `requestReveal` *then* `file.open`,
   * in that order, so the request is parked and spent by the mount the open causes.
   */
  goToSymbol: (path: string, line: number, column: number, endColumn: number) => void
  /**
   * ⏎ in the Go to line popup: put the caret on a line of the file that is already open.
   *
   * Separate from [`goToSymbol`] rather than folded into it, because the two differ in both
   * halves. A symbol jump *selects* the declaration's identifier, so it carries an `endColumn`
   * and has to open a file that is very likely not open yet; this one places an empty caret in
   * the buffer the user is already looking at — the popup will not open without a caret in one.
   * One callback covering both would need a comment at every argument saying which case it was
   * for.
   */
  goToLine: (path: string, line: number, column: number) => void
  /**
   * ⏎ in the scratch type picker: create a scratch of this extension, open it, and select its
   * row in the tree.
   *
   * The extension rather than a language name, because the extension *is* the language here —
   * see `editor/languages.ts::SCRATCH_TYPES`. All three steps belong to the host: this overlay
   * knows which type was chosen and nothing about projects, tabs or the file tree.
   */
  createScratch: (ext: string) => void
}

export interface OverlayHostProps {
  /** `null` when no project is open: the picker has nothing to index, so it is not offered. */
  project: ProjectId | null
  commands: readonly Command[]
  keymap: Keymap
  context: KeyContext
  actions: OverlayActions
}

/** Which project the live Find usages belongs to, read outside React for the guard effect. */
function usagesProject(): string | null {
  return useUsages.getState().project
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
    if (open === 'symbols' && project === null) close()
    // The File Structure popup needs a *caret*, not merely a project: it preselects the member
    // the caret is in and jumps from there. Closed rather than rendered empty, for the reason
    // above — a store left open with nothing on screen makes `overlayOpen()` lie.
    if (open === 'structure' && focusedCaret() === null) close()
    // Go to line needs a caret for the same reason and one more: it prefills with the line the
    // caret is on and reports how many lines the file has, and neither question has an answer
    // without one. The caret is the *only* thing it needs — it asks Rust nothing — so unlike its
    // three neighbours there is no `project === null` arm below. `App` still only mounts this
    // host with a project, which is why that costs nothing today; it is written this way so the
    // popup does not acquire a dependency it does not have.
    if (open === 'goto' && focusedCaret() === null) close()
    // The drawer is keyed by the project's primary root, so there is nothing to create in
    // without one. Closed rather than rendered `null`, for the reason above: a store left open
    // with nothing on screen makes `overlayOpen()` lie, and the next ⇧⌥S toggles a phantom shut.
    if (open === 'scratch' && project === null) close()
    /*
     * Find usages needs a project to have been searched against, and `usagesStore` only holds one
     * while a search is live or its rows are up. Closed rather than rendered empty, for the reason
     * above — a store left open with nothing on screen makes `overlayOpen()` lie, and every
     * `!overlayOpen` binding is silently gated off behind an invisible modal.
     *
     * This is also the arm that catches a project closing under a running search: the popup
     * unmounts, and its unmount effect cancels the request rather than leaving rust-analyzer
     * searching a workspace nobody is looking at.
     */
    if (open === 'usages' && (project === null || usagesProject() === null)) close()
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

  if (open === 'structure') {
    // Until the effect above runs. One frame, and it draws nothing.
    if (project === null || focusedCaret() === null) return null
    return (
      <StructurePicker project={project} onDismiss={close} onGoTo={actions.goToSymbol} />
    )
  }

  if (open === 'symbols') {
    if (project === null) return null
    return <SymbolPicker project={project} onDismiss={close} onGoTo={actions.goToSymbol} />
  }

  if (open === 'scratch') {
    // Until the effect above runs. One frame, and it draws nothing.
    if (project === null) return null
    return <ScratchType onDismiss={close} onCreate={actions.createScratch} />
  }

  if (open === 'goto') {
    // Until the effect above runs. One frame, and it draws nothing. No `project` guard: see the
    // note in the effect.
    if (focusedCaret() === null) return null
    return <GoToLine onDismiss={close} onGoTo={actions.goToLine} />
  }

  if (open === 'usages') {
    // Until the effect above runs. One frame, and it draws nothing.
    if (project === null) return null
    // `goToSymbol`, deliberately: a usage jump and a symbol jump are the same gesture — reveal
    // with a selection over the identifier, then open — and the reveal flags (`focus`, `align`)
    // are decisions this popup has no business making differently from the other three pickers.
    return <UsagesPopup onDismiss={close} onGoTo={actions.goToSymbol} />
  }

  // No arm in the effect above, on `goto`'s reasoning: the card reads `capabilities` off the
  // boot this host was mounted with and asks Rust nothing, so there is no state in which it
  // cannot function once it is on screen.
  if (open === 'about') return <AboutCard onDismiss={close} />
  if (open === 'pools') return <PoolStateCard onDismiss={close} />

  // GitLab dialogs have their own host. Only the explicit commands state owns the palette.
  if (open !== 'commands') return null

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
