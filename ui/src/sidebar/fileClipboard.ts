/**
 * Explorer's mirror of the native file clipboard. Rust owns source roots and cut consumption;
 * this store orders gestures and follows process-wide events plus desktop focus changes.
 */
import { create } from 'zustand'
import {
  diag, fileClip, fsClipboard,
  type PasteCollision, type PasteDecision, type PastedEntry, type ProjectId,
} from '@/ipc/client'
import { pastedSummary, type ClipMode, type FileClip, type PasteOutcome } from './clipboardModel'

interface FileClipboardStore {
  clip: FileClip | null
  take: (mode: ClipMode, project: ProjectId, paths: readonly string[]) => Promise<void>
  clear: () => Promise<void>
  refresh: () => Promise<FileClip | null>
  paste: (
    project: ProjectId,
    destDir: string,
    decisions: readonly PasteDecision[],
    snapshot: FileClip,
  ) => Promise<PasteResult>
}

export interface PasteResult {
  paths: string[]
  note: string | null
}

// A paste issued immediately after Copy must wait for the native write, including failures.
let pending: Promise<unknown> = Promise.resolve()
let revision = 0
function ordered<T>(action: () => Promise<T>): Promise<T> {
  const result = pending.then(action, action)
  pending = result.catch(() => {})
  return result
}

export const useFileClipboard = create<FileClipboardStore>((set, get) => ({
  clip: null,
  take(mode, project, paths) {
    if (paths.length === 0) return Promise.resolve()
    return ordered(async () => {
      const before = revision
      const clip = await fileClip.set({ mode, project, paths: [...paths] })
      if (revision === before) set({ clip })
    })
  },
  clear() {
    const id = get().clip?.id
    if (id === undefined) return Promise.resolve()
    return ordered(async () => {
      const before = revision
      await fileClip.clear(id)
      const clip = await fileClip.get()
      if (revision === before) set({ clip })
    })
  },
  refresh() {
    return ordered(async () => {
      const before = revision
      const clip = await fileClip.get()
      if (revision === before) set({ clip })
      return get().clip
    })
  },
  async paste(project, destDir, decisions, clip) {
    const result = await pasteEntries(
      project, clip.paths, destDir, clip.mode, decisions, sourceOf(clip, project), clip.id,
    )
    if (clip.mode === 'cut' && get().clip?.id === clip.id) set({ clip: null })
    return result
  },
}))

/*
 * The two calls below are free functions rather than methods, and that is the seam a **drag** uses.
 *
 * A drop is a move of paths the user is pointing at — it has nothing to do with what is on the
 * clipboard, and routing it through the store's methods would have been actively destructive:
 * they read their sources from `clip`, so a drop would first have to `take()` the dragged rows,
 * which overwrites both the in-app clipboard *and* the system clipboard, and then `paste()` would
 * clear the clip on a cut. Dragging one file would silently throw away the four the user had
 * copied ten seconds earlier, with nothing on screen to say so.
 *
 * Split out here rather than called straight from the panel so that the *summary* stays in one
 * place: `pastedSummary` is the only thing that knows a rename must be reported and an ordinary
 * paste must say nothing, and a drag with its own copy of that would be the second version that
 * drifts.
 */

/**
 * The project to send as `sourceProject`: the clip's own, when it is not the one being pasted
 * into, and `null` otherwise — the ordinary paste, which checks both ends against one project.
 */
export function sourceOf(clip: FileClip, project: ProjectId): ProjectId | null {
  return clip.project === project ? null : clip.project
}

/** Which names this transfer would land on that are already taken. Writes nothing. */
export function planEntries(
  project: ProjectId,
  sources: readonly string[],
  destDir: string,
  mode: ClipMode,
  sourceProject: ProjectId | null = null,
  clipId: string | null = null,
): Promise<PasteCollision[]> {
  return fsClipboard.plan(project, sources, destDir, mode, sourceProject, clipId)
}

/** Send it, with whatever the user answered about the names that were taken. */
export async function pasteEntries(
  project: ProjectId,
  sources: readonly string[],
  destDir: string,
  mode: ClipMode,
  decisions: readonly PasteDecision[],
  sourceProject: ProjectId | null = null,
  clipId: string | null = null,
): Promise<PasteResult> {
  const entries = await fsClipboard.paste(project, sources, destDir, mode, decisions, sourceProject, clipId)
  return { paths: entries.map((entry) => entry.dest), note: summarize(entries, mode) }
}

/**
 * `PastedEntry` (generated from Rust) read through the model's structural `PasteOutcome`.
 *
 * The one line in the frontend where the two meet, and therefore the one line that fails to
 * compile if `cargo xtask codegen` renames a field — which is the point of writing it out
 * instead of passing `entries` straight through.
 */
function summarize(entries: readonly PastedEntry[], mode: ClipMode): string | null {
  const outcomes: readonly PasteOutcome[] = entries
  return pastedSummary(outcomes, mode)
}

let following = false

/** Seed once, follow other windows, and import another process's clip on desktop focus. */
export function followFileClip(): void {
  if (following) return
  following = true
  const refresh = () => {
    void useFileClipboard.getState().refresh().catch((error: unknown) => {
      void diag.log(`[cide] file clipboard refresh failed: ${String(error)}`)
    })
  }
  void fileClip.onChanged((clip) => {
    revision++
    useFileClipboard.setState({ clip })
  }).then(refresh).catch((error: unknown) => {
    void diag.log(`[cide] file clipboard subscription failed: ${String(error)}`)
  })
  window.addEventListener('focus', refresh)
}
