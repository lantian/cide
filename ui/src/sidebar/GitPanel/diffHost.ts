/**
 * Whether anything in this window can render a git diff tab.
 *
 * # The gap this closes
 *
 * `tab_open_diff` opens a tab in the Rust-owned workspace; *drawing* it is the shell's job,
 * and the shell is `App.tsx` — one file this package cannot edit and which may not have been
 * wired yet. Without the check, the un-wired build answers a double-click with a tab
 * containing an empty pane: no error, no diff, nothing to explain it. That is the same shape
 * of failure the panel already had once, when it fetched a `FileDiff` and dropped it on the
 * floor because no host had been passed.
 *
 * So `GitDiffPane` announces itself at module scope, and the panel asks before it opens
 * anything. A build whose shell never imports the pane never bundles it, the flag stays
 * false, and the panel says so in its one warning line instead of opening a blank tab.
 *
 * A module-level flag rather than context or a prop because the two ends are not in one
 * React tree: the panel is in the sidebar and the pane is inside a workspace tab, mounted by
 * the shell only *after* the tab exists — so at the moment the question is asked there is no
 * pane to ask, only a renderer that either exists in this bundle or does not.
 *
 * The flag says the renderer was linked, not that a tab is currently showing. That is the
 * question that matters here: a tab this panel opens will be drawn by whatever draws the
 * others.
 */
let available = false

/** Called at import time by the module that can render a diff tab. */
export function markDiffPaneAvailable(): void {
  available = true
}

export function diffPaneAvailable(): boolean {
  return available
}
