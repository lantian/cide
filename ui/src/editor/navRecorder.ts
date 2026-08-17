/**
 * The pointer, on the Back stack. (M16)
 *
 * Twenty lines of fact-gathering and no decisions: `navHistory.ts::recordsClick` is the rule and
 * this hands it four booleans-worth of `ViewUpdate`. The split is the same one `viewTracker.ts`
 * and `position.ts` make, for the same reason — a `ViewUpdate` cannot exist without a laid-out
 * DOM, so anything a check script has to run has to be on the other side of it.
 *
 * # Why a `ViewPlugin` and not `EditorSurface`'s update listener
 *
 * Three reasons, and the third is the one that would have been found late:
 *
 * * **Placement.** The listener in `EditorSurface` is already the app's hottest path — it writes
 *   the `Ln 7, Col 48` readout straight into a DOM node specifically to keep React out of it.
 *   Adding a fifth concern to it would put the navigation history inside the component whose
 *   header promises no knowledge of tabs; a sibling extension keeps that promise, exactly as
 *   `ctrlLink(project, path)` and `viewTracker(path, …)` on the lines either side of it do.
 * * **Cost.** The first thing `update()` does is ask whether any transaction in the batch carries
 *   `select.pointer`. Typing and arrow keys fail on that one predicate and never reach a
 *   `doc.lineAt`, so the per-keystroke cost of this whole feature is one `Array.some` over a
 *   one-element array.
 * * **Ordering.** CodeMirror runs plugin updates *before* update listeners (`EditorView.update`:
 *   `updatePlugins(update)`, then `docView.update`, then the `updateListener` loop). That is what
 *   lets this read the caret the user is coming **from** when the click lands in a different pane:
 *   `EditorSurface`'s listener is what calls `caret.focus()` and moves `caretTrack`'s slot onto
 *   the editor being clicked into, and it has not run yet. A listener here would read the
 *   destination as its own origin, `near` would merge them, and Back would land nowhere — the
 *   failure `jump.ts`'s `pendingJump` header describes for the terminal path, arriving by a
 *   different route.
 *
 * The click-into-an-unfocused-pane case has a second ordering inside it, and it was checked rather
 * than assumed because it is the one that would have made this whole feature half-dead. CodeMirror's
 * `mousedown` handler takes focus **before** it dispatches the selection, so the obvious worry is
 * that a `focusChanged` update arrives first and the listener moves the slot before this ever runs.
 * It does not, twice over: the DOM `focus` observer defers through `setTimeout(…, 10)`, and the
 * dispatch that follows finds `hasFocus !== notifiedFocused` and folds the focus flag into **that
 * same update** — a separate focus transaction is only cut when a `focusChangeEffect` exists, and
 * this app registers none. So focus and selection arrive as one update, and one update means
 * plugins before listeners.
 *
 * All of that is an assumption about somebody else's library, so it is worth stating exactly what
 * it costs if a version bump ever breaks it. [`originOf`] falls back to `update.startState`
 * whenever the live caret is already in *this* file, so a reordering degrades the cross-file clause
 * into the same-file one: a click from another pane would take its origin from this buffer's own
 * previous caret, and the entry becomes a same-file one or — far more often — no entry at all,
 * since two carets in one pane are usually near each other. **It can lose an entry or narrow one.
 * It cannot invent a place the caret was never at**, because both sources are real carets, which
 * is what makes the degradation safe to ship without a runtime guard.
 */
import { ViewPlugin, type ViewUpdate } from '@codemirror/view'
import type { EditorState, Extension } from '@codemirror/state'
import { focusedCaret } from './caretTrack'
import { recordsClick } from './navHistory'
import { recordClick } from './jump'
import type { FilePosition } from './position'

/**
 * Put far pointer clicks in this buffer on the project's Back stack.
 *
 * `project` is `undefined` for a pane outside every project, and then nothing is recorded — a
 * history is keyed by project and there is nothing to key it on. `path` is fixed for the life of
 * the view, for the reason `viewTracker` states: `EditorSurface` rebuilds the whole `EditorView`
 * when the file changes, so a path that could change under a live plugin would be a plugin
 * outliving the editor it belongs to.
 */
export function navRecorder(project: string | undefined, path: string): Extension {
  return ViewPlugin.define(() => new NavRecorder(project ?? null, path))
}

class NavRecorder {
  constructor(
    private readonly project: string | null,
    private readonly path: string,
  ) {}

  update(update: ViewUpdate): void {
    /*
     * The cost gate, and it is deliberately the same predicate the rule's first line tests.
     *
     * Everything below reads the document, and this runs on every selection change — thirty
     * times a second under a held arrow key, on the path `caretTrack.ts` and `EditorSurface.tsx`
     * both go out of their way to keep off React. So the answer is computed once, used to leave
     * immediately, and then handed to `recordsClick` rather than being re-derived there: the
     * decision stays whole and drivable in the pure module, and a keystroke still pays for one
     * `Array.some`.
     */
    const pointer = update.transactions.some((tr) => tr.isUserEvent('select.pointer'))
    if (!pointer) return

    const to = positionIn(update.state, this.path)
    const from = originOf(update, this.path)
    const selection = update.state.selection
    if (
      !recordsClick({
        pointer,
        // One range and empty: a caret. Two tests folded into one field, because they answer one
        // question — see `ClickGesture.caret` for the drag, the double-click and the Alt+click.
        caret: selection.ranges.length === 1 && selection.main.empty,
        docChanged: update.docChanged,
        from,
        to,
      })
    ) {
      return
    }
    recordClick(this.project, from, to)
  }
}

/**
 * Where the user was, from whichever of the two sources can still answer.
 *
 * `caretTrack`'s slot first, and **only when it names another file**. That is the case
 * `update.startState` cannot serve: a click into the other half of a split leaves this view's own
 * previous caret sitting wherever it was when the pane was last used, which is not where the user
 * was a moment ago. The slot is the same source `jumpTo` reads for every other kind of
 * navigation, so Back from a click and Back from Go to definition agree about what an origin is.
 *
 * Otherwise `update.startState` — this view's caret before this transaction — which is exactly
 * right for the common case of clicking about inside one buffer, and is authoritative in a way
 * the slot is not: two panes can show the *same* path, and then the slot's `path` matches while
 * its line belongs to the other pane.
 *
 * # The consequence worth stating: the first click after a tab switch records the switch
 *
 * `caretTrack`'s claims are released on *unmount*, and `TabContent` never unmounts an inactive
 * tab — so after Ctrl+Tab the slot still names the file in the tab just left, until something in
 * the new one takes focus. The first pointer click in the newly active tab therefore sees an
 * origin in another file, and `recordsClick` records it however few lines the click itself moved
 * the caret.
 *
 * That looks at first like a hole in `navHistory`'s "switching between already-open tabs is not a
 * jump" rule, and it is not: the tab switch on its own still records nothing, and what is recorded
 * here is a pointer landing in a document the caret was not in — which is the rule, applied
 * exactly. It is also the behaviour a user wants, because Back from the tab you just switched to
 * returns to the one you came from rather than refusing.
 *
 * The alternative was to always read `update.startState` and never the slot. It is *more*
 * consistent with the tab rule and it loses, twice over: it would break the split case this
 * function exists for — a click into the other half of a split would take its origin from
 * wherever that pane's own caret was parked, which is not where the user was a moment ago — and
 * it would make Back from a click disagree with Back from Go to definition about what an origin
 * is, since `jumpTo` reads the slot for every other gesture in the app.
 */
function originOf(update: ViewUpdate, path: string): FilePosition | null {
  const live = focusedCaret()
  if (live !== null && live.path !== path) {
    return { path: live.path, line: live.line, column: live.column }
  }
  return positionIn(update.startState, path)
}

/** A state's main head, as the shared position type. 1-based line, 1-based UTF-16 column. */
function positionIn(state: EditorState, path: string): FilePosition {
  const head = state.selection.main.head
  const line = state.doc.lineAt(head)
  return { path, line: line.number, column: head - line.from + 1 }
}
