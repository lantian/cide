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
 * Nothing here touches the disk. Ctrl+X writes a mode and some paths, and that is all a cut is
 * until [`paste`] runs — see `cide_fs::copy` for why that is the whole point.
 */
import { create } from 'zustand'
import { fsClipboard, type PastedEntry, type ProjectId } from '@/ipc/client'
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
   * Paste into `destDir`. Resolves to the sentence the panel should show, or `null` when
   * there is nothing worth saying — see `pastedSummary`.
   *
   * Rejects with the tagged `FsError` on refusal; the caller reports it. A cut clears the
   * clipboard on success and **only** on success: a failed move must leave the clip alone so
   * the user can fix the reason and press Ctrl+V again.
   */
  paste: (project: ProjectId, destDir: string) => Promise<PasteResult>
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
    set({ clip: { mode, project, paths: [...paths] } })
    // Failure is reported by `copyText` itself and does not fail the gesture: the in-app clip
    // is the one Paste needs, and a webview that refused `writeText` must not also cost the
    // user their Ctrl+V.
    void copyText(clipboardText(paths))
  },

  clear() {
    if (get().clip === null) return
    set({ clip: null })
  },

  async paste(project, destDir) {
    const clip = get().clip
    if (clip === null) return { paths: [], note: null }

    const entries = await fsClipboard.paste(project, clip.paths, destDir, clip.mode)
    // A cut is consumed by the paste it was made for. A copy is not: pressing Ctrl+V twice is
    // how anyone makes two copies, and clearing here would make the second press do nothing.
    if (clip.mode === 'cut') set({ clip: null })
    return { paths: entries.map((entry) => entry.dest), note: summarize(entries, clip.mode) }
  },
}))

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
