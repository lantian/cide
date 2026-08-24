/**
 * Telling CodeMirror which polarity it is being drawn in. (M24)
 *
 * # The bug this exists to prevent, which shipped
 *
 * `@codemirror/view` puts one of two generated classes on the editor element — `baseLightID` or
 * `baseDarkID` — chosen from the `EditorView.darkTheme` facet, and its base theme is written in
 * terms of them (`"&light .cm-content"`, `"&dark .cm-gutters"`, and about forty more across
 * view, autocomplete, lint, search and merge). Nothing in this application ever set that facet,
 * so **every editor in cide has been a light-mode CodeMirror**, including under the dark theme.
 *
 * Most of it was masked: our own stylesheets override the same properties at equal or greater
 * specificity, so a reader would never know. One rule is not masked, and it is the one that was
 * reported twice as *"I select the code and I can't see the text"*:
 *
 * ```
 * "&light.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground":
 *   { background: "#d7d4f0" }
 * ```
 *
 * `&` and `&light` are *generated classes*, so that selector is six classes — specificity
 * (0,6,0) — against the (0,3,0) of `.body :global(.cm-editor .cm-selectionBackground)` in
 * `EditorSurface.module.css`. Ours never applied. Selecting code in the dark theme painted
 * CodeMirror's **light-mode lavender**, `#d7d4f0`, and `--tk-fg` is `#d7d7dd` — a contrast ratio
 * of 1.02:1. The text was not dim; it was gone.
 *
 * Two rounds of retuning `--tk-sel` went into that report before anyone read the pixels, and
 * neither could have worked: the token being tuned was not reaching the screen.
 *
 * # What this fixes and what it does not
 *
 * Setting the facet stops `&light` matching under the dark theme, so every one of those forty
 * rules falls back to a sensible dark default instead of a light one — panels, tooltips, the
 * autocomplete popup, lint marks, the search bar. It does **not** settle the selection colour:
 * `&dark.cm-focused …` is the same (0,6,0) selector with `#233` in it, so cide's own colour still
 * has to win on specificity, and it does that in `editor/highlight.css`. Both halves are needed;
 * neither is sufficient.
 *
 * # Why a compartment
 *
 * The facet is read at configuration time, so a theme switch has to reconfigure it. Rebuilding
 * the view instead would take scrollback, selection, undo history and any in-flight composition
 * with it — the same argument `EditorSurface`'s language, lint and blame compartments each make.
 */
import { Compartment } from '@codemirror/state'
import { EditorView } from '@codemirror/view'

import { useWorkspace } from '@/store/workspace'

/** Whether this window is currently dark, read from the attribute that decides it. */
function isDark(): boolean {
  return document.documentElement.dataset.theme === 'dark'
}

/**
 * A compartment holding the polarity, plus the extension to seed it with.
 *
 * Seeded from the DOM rather than from the store: `public/theme-boot.js` writes `data-theme` in
 * `<head>` from the `?theme=` parameter, so it is correct a whole module graph before the store
 * has hydrated — the same reason `installThemeSync` reads it there.
 */
export function polarityExtension(slot: Compartment) {
  return slot.of(EditorView.darkTheme.of(isDark()))
}

/** A fresh compartment for one view. */
export function polaritySlot(): Compartment {
  return new Compartment()
}

/**
 * Keep `view`'s polarity in step with the window's, and stop when the returned function is called.
 *
 * Subscribes to the store rather than to a `MutationObserver` on `data-theme`, because the store
 * is what actually changes — `installThemeSync` writes the attribute *from* it — and one
 * subscription per view is cheaper than one observer per view.
 *
 * The guard is on the resolved boolean, not on the theme: this listener sees every snapshot, and
 * dispatching a reconfigure on each one would be a transaction per keystroke in another window.
 */
export function watchPolarity(view: EditorView, slot: Compartment): () => void {
  let showing = isDark()
  return useWorkspace.subscribe((state) => {
    const next = state.theme === 'dark'
    if (next === showing) return
    showing = next
    // The view can outlive its subscription by a frame during teardown; a dispatch into a
    // destroyed view throws and would take the effect's cleanup with it.
    if (view.dom.isConnected) {
      view.dispatch({ effects: slot.reconfigure(EditorView.darkTheme.of(next)) })
    }
  })
}
