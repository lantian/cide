/**
 * What the file tree is holding, and the one call that acts on it.
 *
 * A store of its own rather than three fields on `treeStore`, because the two have different
 * lifetimes and the difference is load-bearing: `treeStore.attach` throws its whole cache away
 * on every project switch, and a clipboard that did the same would lose a copy the moment the
 * user looked at another project — which is precisely when people copy things. This survives a
 * project switch and refuses to paste into the wrong project instead (`pasteRefusal`).
 *
 * It also survives the panel unmounting, which happens every time the activity rail moves to
 * Search or Git. A `useState` in `FileTree` would not, and "my copy disappeared because I
 * clicked something else" is indistinguishable from a copy that never worked.
 *
 * **It is a mirror, and Rust holds the clip** (`cmd::fs::FileClipState`). Each webview used to
 * own its own, so a Copy in one window was invisible to every other — and a paste into a project
 * drawn by another window (detached, or in per-project window mode) had nothing to paste. Now
 * `take` and `clear` write through to Rust and every window follows `cide://fs-clip-changed`
 * ([`followFileClip`]). The local write in `take` is optimistic, so the strip and the faded
 * row appear on the keystroke rather than one round trip later; the event that follows carries
 * the same clip. A paste into another project sends the clip's project as `sourceProject`, and
 * Rust checks the sources against it — the old "paste them where they came from" refusal is gone.
 *
 * Nothing here touches the disk. Ctrl+X writes a mode and some paths, and that is all a cut is
 * until [`paste`] runs — see `cide_fs::copy` for why that is the whole point.
 */
import { create } from 'zustand'
import {
  fileClip,
  fsClipboard,
  type PasteCollision,
  type PasteDecision,
  type FileClip as WireClip,
  type PastedEntry,
  type ProjectId,
} from '@/ipc/client'
import { copyText } from './copyText'
import {
  clipboardText,
  pastedSummary,
  type ClipMode,
  type FileClip,
  type PasteOutcome,
} from './clipboardModel'

interface FileClipboardStore {
  /** What is on the clipboard, or `null`. */
  clip: FileClip | null
  /**
   * Take paths onto the clipboard, and put them on the *system* clipboard as text too.
   *
   * Both, deliberately: the in-app clip is what Paste reads, and the text is what makes a Copy
   * in the tree useful in a terminal pane or anywhere else. Neither is a substitute for the
   * other — see `clipboardModel`'s header for what cide does not support (the desktop's own
   * file-clipboard flavours) and why.
   */
  take: (mode: ClipMode, project: ProjectId, paths: readonly string[]) => void
  /** Drop the clipboard. Escape in the tree, and every successful cut-paste. */
  clear: () => void
  /**
   * Which names this paste would land on that are already taken. **Writes nothing.**
   *
   * Always called before [`paste`], and the empty answer — the common one — means the paste can
   * go straight through with no dialog. Anything in it is a question for `PasteConfirm`, whose
   * answers come back as the `decisions` argument below. Asking first is what makes cancelling
   * free: there is no half-finished paste to describe, because none has started.
   *
   * Rejects with the same `FsError` a paste would; the caller reports it and asks nothing.
   */
  plan: (project: ProjectId, destDir: string) => Promise<PasteCollision[]>
  /**
   * Paste into `destDir`. Resolves to the sentence the panel should show, or `null` when
   * there is nothing worth saying — see `pastedSummary`.
   *
   * `decisions` are the dialog's answers, and the only thing that can make this overwrite a
   * file. An empty list is the ordinary call: every collision is then renamed, exactly as
   * before this dialog existed.
   *
   * Rejects with the tagged `FsError` on refusal; the caller reports it. A cut clears the
   * clipboard on success and **only** on success: a failed move must leave the clip alone so
   * the user can fix the reason and press Ctrl+V again.
   */
  paste: (
    project: ProjectId,
    destDir: string,
    decisions: readonly PasteDecision[],
  ) => Promise<PasteResult>
}

export interface PasteResult {
  /** Where the pasted things landed, in the order they were given. */
  paths: string[]
  /** What to tell the user, or `null` when the tree already shows it. */
  note: string | null
}

export const useFileClipboard = create<FileClipboardStore>((set, get) => ({
  clip: null,

  take(mode, project, paths) {
    if (paths.length === 0) return
    const clip = { mode, project, paths: [...paths] }
    set({ clip })
    // Every other window learns of it from Rust's event. A failure leaves this window's paste
    // working and costs only the cross-window half, which is not worth a toast on a Ctrl+C.
    void fileClip.set(clip).catch(() => {})
    // Failure is reported by `copyText` itself and does not fail the gesture: the in-app clip
    // is the one Paste needs, and a webview that refused `writeText` must not also cost the
    // user their Ctrl+V.
    void copyText(clipboardText(paths))
  },

  clear() {
    if (get().clip === null) return
    set({ clip: null })
    void fileClip.clear().catch(() => {})
  },

  async plan(project, destDir) {
    const clip = get().clip
    if (clip === null) return []
    return planEntries(project, clip.paths, destDir, clip.mode, sourceOf(clip, project))
  },

  async paste(project, destDir, decisions) {
    const clip = get().clip
    if (clip === null) return { paths: [], note: null }

    // Rust clears its own copy of a consumed cut and tells every other window; clearing here
    // too is only so this window does not wait a round trip to stop drawing the strip.
    const from = sourceOf(clip, project)
    const result = await pasteEntries(project, clip.paths, destDir, clip.mode, decisions, from)
    // A cut is consumed by the paste it was made for. A copy is not: pressing Ctrl+V twice is
    // how anyone makes two copies, and clearing here would make the second press do nothing.
    if (clip.mode === 'cut') set({ clip: null })
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
): Promise<PasteCollision[]> {
  return fsClipboard.plan(project, sources, destDir, mode, sourceProject)
}

/** Send it, with whatever the user answered about the names that were taken. */
export async function pasteEntries(
  project: ProjectId,
  sources: readonly string[],
  destDir: string,
  mode: ClipMode,
  decisions: readonly PasteDecision[],
  sourceProject: ProjectId | null = null,
): Promise<PasteResult> {
  const entries = await fsClipboard.paste(project, sources, destDir, mode, decisions, sourceProject)
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

/**
 * Mirror Rust's clip in this window: seed it once, then follow `cide://fs-clip-changed`.
 *
 * Once per window and never torn down, like `openDiffTabs`' subscription — the tree panel it
 * serves mounts and unmounts every time the activity rail moves, and the clip has to be right
 * the moment it comes back. Called from the panel's effect rather than at module load so that a
 * server render (the `check-*-render` scripts) never reaches IPC.
 *
 * The seed is ordered *behind* the subscription and is ignored if an event already arrived:
 * a Copy landing between the two would otherwise be overwritten by the older answer to `get`.
 */
export function followFileClip(): void {
  if (following) return
  following = true
  let heard = false
  void fileClip
    .onChanged((clip) => {
      heard = true
      useFileClipboard.setState({ clip: clip === null ? null : asClip(clip) })
    })
    .then(() => fileClip.get())
    .then((clip) => {
      if (!heard) useFileClipboard.setState({ clip: clip === null ? null : asClip(clip) })
    })
    // A build without these commands keeps the old per-window clipboard, which still pastes
    // within this window — the honest fallback, and not worth a notice.
    .catch(() => {})
}

/**
 * The generated `FileClip` read through the model's structural one — the line that stops
 * compiling if `cargo xtask codegen` renames a field, as `summarize` does for `PastedEntry`.
 */
function asClip(clip: WireClip): FileClip {
  return clip
}
