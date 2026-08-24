/**
 * Whether the file tree holds the caret right now, for the surfaces that are not inside it.
 *
 * # The gap this closes
 *
 * Ctrl+Shift+F means *search this project* from anywhere, and *search this folder* when it is
 * pressed with the file tree focused. Only the second half needs this module, and the reason
 * it cannot be done the way the tree's other focus-scoped chords are done is worth stating,
 * because it looks at first like the same problem:
 *
 * `keys/gate.ts` is `window.addEventListener('keydown', gate, true)` — **capture at the
 * window** — and `ctrl+shift+f` is bound in `cide_core::keymap::defaults()`. So the gate
 * resolves it, calls `preventDefault`, dispatches `sidebar.search`, and the tree scroller's
 * `onKeyDown` is never called at all. Ctrl+C, Ctrl+X, Ctrl+V, Delete and every speed-search
 * letter *can* live on the scroller precisely because nothing in the default keymap binds
 * them; `FileTree.tsx`, `clickSemantics.ts` and `speedSearch.ts` each say so. This one is
 * bound, so that route is shut and the question has to be asked from the dispatcher instead.
 *
 * # Why not a context flag
 *
 * `CONTEXT_FLAGS` would be the other reflex, and it is the wrong tool twice over. A
 * `Command::when` does not gate the keyboard at all — `keys/keymap.ts` evaluates
 * `Binding::when`, a different field — so a `fileTreeFocused` clause would change what the
 * palette lists and nothing about what the chord does. And gating is not what is wanted: the
 * command must keep working everywhere, and merely do slightly more in one place. A flag that
 * hid the row from the palette whenever the tree was unfocused would be a strictly worse
 * command than the one that exists.
 *
 * The flag that does exist, `sidebarFiles`, means the panel is *visible* — which it is while
 * the user types in a terminal beside it. That is the distinction this whole module is about.
 *
 * # Why a plain module and not a store
 *
 * Nothing renders from it. It is written on focus and blur and read exactly once, inside a key
 * handler, when a chord fires — so a zustand `set()` would re-render subscribers for a fact no
 * subscriber wants. `editor/caretTrack.ts` is the same shape for the same reason and its
 * header makes the same argument at greater length.
 *
 * Per window by construction, like everything else in this neighbourhood: each Tauri window is
 * its own JavaScript realm, and each has its own caret.
 *
 * Imports nothing, so `ui/scripts/check-search.mjs` can compile it alone and drive it.
 */

/**
 * A single boolean, not a claims stack.
 *
 * `caretTrack.ts` keeps a stack because a split shows two editors and there is one caret worth
 * reporting. There is exactly one file tree in a window — the Git panel's `ChangesTree` is a
 * different tree with a different selection and is deliberately not counted here, because
 * "search in the selected folder" is about the *file* tree's selection and nothing else.
 */
let focused = false

/**
 * The tree has the caret.
 *
 * Called from the scroller's `onFocus`, which is the bubbling `focusin` — so it also fires when
 * focus lands on the rename box or the speed-search field *inside* the tree, and that is
 * correct: those are the tree, and a chord pressed with the caret in one of them still means
 * the row the tree has selected.
 */
export function claimTreeFocus(): void {
  focused = true
}

/**
 * The tree no longer has it.
 *
 * Called on a blur that really left the tree — `speedSearch.blurLeftTheTree` is the test, and
 * without it every focus move *within* the tree would report a departure — and on unmount,
 * which happens every time the rail's selection changes.
 */
export function releaseTreeFocus(): void {
  focused = false
}

/**
 * Does the file tree hold the caret?
 *
 * Read at the moment a command runs, never captured: the dispatcher lives in a window listener
 * outside React and a value closed over at render time would answer for whenever that render
 * was.
 */
export function treeFocused(): boolean {
  return focused
}

/**
 * Test seam: forget the claim. Never called by the app.
 *
 * The precedent is `chrome/panelRequests.ts::__resetPanelRequests`. Module state outlives an
 * assertion, and a check that had to blur its way back to a clean slate would be asserting on
 * the cleanup as much as on the rule.
 */
export function __resetTreeFocus(): void {
  focused = false
}
