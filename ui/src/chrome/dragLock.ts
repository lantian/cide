/**
 * The two things the `<body>` has to be told while a pointer drag is in flight, and the one
 * reason they cannot be written the obvious way.
 *
 * Both splitters — `layout/Splitter.tsx` and `chrome/SidebarSplitter.tsx` — spend a drag with
 * the pointer somewhere other than the handle: over the panes, over the terminal, over the
 * editor. Without a cursor on the body the pointer flickers to a text caret on every crossing,
 * and without the selection lock a drag that started over prose leaves a selection behind it.
 *
 * # Why `style.userSelect = 'none'` is not enough, and looks like it is
 *
 * WebKitGTK 2.52.3 does not implement unprefixed `user-select`. The CSSOM is worse than the
 * parser here rather than better: `el.style.userSelect = 'none'` **reads back as `'none'`**,
 * so the assignment appears to have taken, while `el.style.cssText` stays empty and nothing
 * on screen changes. Every debugging session that ends at "but the property is set" ends
 * there because of that. `setProperty` with the prefixed name is what actually lands.
 *
 * The unprefixed spelling is written too, and is not decoration either: it is the standard
 * one, and it is what any engine this app is later built against will use. The full argument,
 * including why the bug is invisible in a release build, is in `styles/tokens.css`.
 *
 * Both callers already `preventDefault()` on the pointerdown, which is the engine-level
 * refusal and does most of the work. These two properties cover the rest of the gesture,
 * after the press, while the pointer is travelling.
 */

/** `col-resize`, `row-resize` — whatever the gesture in flight should look like. */
export function lockBodyForDrag(cursor: string): void {
  const style = document.body.style
  style.cursor = cursor
  style.setProperty('-webkit-user-select', 'none')
  style.userSelect = 'none'
}

/**
 * Put the body back.
 *
 * Called from `stop`, and again from an unmount effect: a splitter can be taken off screen
 * mid-drag by a snapshot that removes its split or by a view switch, and a body left locked
 * keeps a resize cursor and an unselectable page for the rest of the session.
 */
export function unlockBodyAfterDrag(): void {
  const style = document.body.style
  style.cursor = ''
  style.removeProperty('-webkit-user-select')
  style.userSelect = ''
}
