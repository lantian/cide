/**
 * Pasting a screenshot into a buffer: whether to, where the file goes, and what gets typed. (M35)
 *
 * The reported ask was *"if i do PASTE action in file tree and in buffer image — we should be
 * able to create image file in place where we pasting it"*. The file-tree half lands in
 * `sidebar/FileTree.tsx`; this is the buffer half, and it is the same command underneath
 * (`fs_paste_image`) with two extra decisions on top: **is this paste an image at all**, and
 * **what text replaces it**.
 *
 * # Why the DOM event decides, and Rust reads
 *
 * `ui/src/terminal/clipboard.ts` records at length why cide does not ask the clipboard whether
 * it holds an image: the plugin's `read_image` is a *decode*, not a probe — it materialises a
 * full RGBA bitmap and hands back a resource id. That argument is about asking a *question*.
 * Here the question is free: a `paste` event carries `clipboardData.types`, a list of strings
 * the browser has already assembled, and reading it costs nothing and decodes nothing. So the
 * DOM event is the detector, and `fs_paste_image` — which does decode, once, on a blocking
 * thread in Rust — is the reader. Nothing about the image is ever in JavaScript.
 *
 * # Import-free on purpose
 *
 * `check:paste-image` compiles this module standalone and drives it, the way
 * `sidebar/newEntry.ts` and `sidebar/clipboardModel.ts` are driven. Everything here is a rule
 * with a wrong answer that would be invisible on screen — a link with the wrong number of `../`
 * in it renders as a broken image and not as an error — so the rules must be assertable
 * without a webview.
 */

/**
 * Does this paste want an image file rather than text?
 *
 * Two conditions, and the second is the load-bearing one.
 *
 * **There must be an image type.** `image/png` for every screenshot tool on every platform cide
 * targets; the prefix test rather than an exact match, because a clipboard can offer
 * `image/jpeg` or `image/tiff` and cide writes a PNG from the decoded bitmap either way.
 *
 * **There must be no text.** A clipboard holding both is holding text as far as this is
 * concerned, and that is the same trade `terminal/clipboard.ts` makes for Ctrl+V in a Claude
 * pane, for the same reason: a file copied in a file manager offers `text/uri-list` *and*
 * `text/plain` *and* sometimes a thumbnail, and a person who copies a path and pastes it into a
 * buffer means the path. Screenshot tools (Spectacle, Flameshot, GNOME Screenshot, macOS
 * ⌘⇧4) offer the image alone, so the gesture this exists for is untouched.
 *
 * `Files` is deliberately not treated as an image. Dragging or copying a `.png` *file* puts
 * `Files` in the list, and the right answer for that is the file tree's paste — copying the
 * file — not re-encoding its pixels into a new one under a new name.
 */
export function wantsImagePaste(types: readonly string[]): boolean {
  const hasImage = types.some((type) => type.startsWith('image/'))
  const hasText = types.some((type) => type === 'text/plain' || type === 'text/uri-list')
  return hasImage && !hasText
}

/**
 * The directory a buffer's pasted image belongs in: the one the document is in.
 *
 * *"in place where we pasting it"*, read literally, and it is also the only answer that keeps
 * the inserted reference short and the file findable. An `assets/` convention was the
 * alternative and it loses here: cide would be creating a directory the project did not ask
 * for, and the right name for it differs per project (`assets`, `images`, `static`, `docs/img`).
 *
 * `null` for a path with no directory part — a buffer that is not a file on disk. The caller
 * must refuse rather than guess: writing a screenshot into the process's cwd is writing it
 * somewhere the user cannot see.
 */
export function imageDirFor(docPath: string): string | null {
  const at = docPath.lastIndexOf('/')
  if (at < 0) return null
  // A file at the filesystem root has an empty directory part, which is `/` and not "".
  return at === 0 ? '/' : docPath.slice(0, at)
}

/**
 * The text to type at the cursor for an image that was just written beside the document.
 *
 * Markdown gets a picture (`![](name.png)`) because that is what a markdown buffer means by an
 * image and because the preview renders it immediately — the whole point of the gesture there.
 * Everything else gets the bare relative path: a comment, a string literal, an HTML `src`, a
 * YAML value and a CSS `url()` all want that and none of them wants markdown syntax around it.
 *
 * The alt text is left empty rather than filled with the file name. A screenshot's generated
 * name (`Pasted image 2026-08-31 at 22.41.07`) describes nothing, and alt text that describes
 * nothing is worse than none — a screen reader reads it out in place of the image. An empty
 * `![]()` is the documented way to say *this image is decorative or not yet described*, and the
 * cursor lands somewhere the user can type into.
 *
 * `languageId` is the editor's own language id, so this is one table rather than a second
 * extension list to keep in step with `builtinLanguages.ts`.
 */
export function referenceFor(languageId: string | null, relative: string): string {
  return isMarkdown(languageId) ? `![](${encodeReference(relative)})` : relative
}

/**
 * Which languages get a markdown picture. `mdx` as well as `markdown`, because an `.mdx`
 * buffer is markdown with components in it and `![](…)` means the same thing there.
 */
function isMarkdown(languageId: string | null): boolean {
  return languageId === 'markdown' || languageId === 'mdx'
}

/**
 * `relative`, escaped for the inside of a markdown link's parentheses.
 *
 * A generated name has a space in it (`Pasted image 2026-08-31 at 22.41.07.png`) and a space
 * ends the destination in CommonMark — `![](Pasted image ….png)` parses as a link to `Pasted`
 * followed by literal text, which renders as a broken image with prose after it. Angle brackets
 * are the spec's own answer and are what every markdown editor emits, so a destination with a
 * space in it is wrapped rather than percent-encoded: `%20` is also correct and is unreadable
 * in the source, which is the half of a markdown file people actually edit.
 *
 * A destination that already contains `<` or `>` cannot be wrapped — the delimiter would close
 * early — so that one falls back to percent-encoding the spaces. It is not reachable from a
 * cide-generated name and is here because a caller may pass a name the user chose.
 */
function encodeReference(relative: string): string {
  if (!relative.includes(' ')) return relative
  if (relative.includes('<') || relative.includes('>')) return relative.replaceAll(' ', '%20')
  return `<${relative}>`
}

/**
 * `target` expressed relative to the **document** `from` names — that is, to its directory.
 *
 * `from` is a file, not a directory, and taking it that way rather than making the caller strip
 * the last component is the whole reason this is one function: the reference is written into
 * `from`, so relating it to anything else is an error nobody would see. A link that resolves
 * against the wrong directory renders as a broken-image glyph, with the file present on disk
 * and nothing logged — so it reads as a preview bug rather than as a paste bug.
 *
 * Component-wise, and it has to be: the two paths come from different sides of the IPC — `from`
 * is the pane's document path and `target` is what Rust answered — so they are two strings
 * about one filesystem, and string arithmetic on them is where an off-by-one `../` comes from.
 *
 * A sibling (the ordinary case, since the image is written into the document's own directory)
 * yields a bare file name. Two absolute paths always relate, through the root if through
 * nothing else; a non-absolute side cannot be related at all, and the answer for it is
 * `target` unchanged, which is at least a true reference.
 *
 * No `./` prefix. Markdown, HTML and CSS all resolve a bare name against the containing
 * document, and the prefix is noise in the one place the text is read by a person.
 */
export function relativeTo(from: string, target: string): string {
  if (!from.startsWith('/') || !target.startsWith('/')) return target

  const base = from.split('/').filter((part) => part.length > 0)
  // The document's own name is not a directory to climb out of.
  base.pop()
  const to = target.split('/').filter((part) => part.length > 0)

  let shared = 0
  while (shared < base.length && shared < to.length && base[shared] === to[shared]) shared += 1

  const up = base.length - shared
  const down = to.slice(shared)
  if (up === 0) return down.join('/')
  return [...Array<string>(up).fill('..'), ...down].join('/')
}

/**
 * The CodeMirror side of the gesture: intercept a `paste` that carries an image and nothing
 * else, and let the caller write the file and type the reference.
 *
 * Lives here rather than in `EditorSurface.tsx` for the reason `ctrlLink` gives about its own
 * pair of handlers: the rule that decides whether to take the event and the code that acts on
 * it must not be able to disagree. Everything above this line is pure and is driven by
 * `check:paste-image`; this is the ten impure lines that connect them.
 *
 * `write` is asynchronous and `paste` is not, so the choice to claim the event is made from
 * `clipboardData.types` alone — which is a list the browser has already assembled, costs
 * nothing to read and decodes nothing. That is the same shape, and the same constraint, as
 * `terminal/clipboard.ts`'s note about a synchronous key handler and an asynchronous clipboard.
 *
 * `preventDefault` happens **only** when the answer is yes. Everything else — text, a file, an
 * empty clipboard, a read-only buffer, a buffer with no path — falls through to the webview's
 * own paste, unchanged.
 */
export interface ImagePasteHost {
  /** Where the document lives, or `null` when it is not a file on disk. */
  readonly docPath: string | null
  /** The editor's language id, for [`referenceFor`]. */
  readonly languageId: string | null
  /** Refuse in a read-only buffer: a revision or a diff has nothing to paste into. */
  readonly readOnly: boolean
  /**
   * Write the clipboard image into `dir` and answer where it landed, or `null` for *there was
   * no image after all* — which the `types` list cannot rule out on its own, because a
   * clipboard can change between the event and the write.
   */
  readonly write: (dir: string) => Promise<string | null>
  /** Type `text` at the cursor. */
  readonly insert: (text: string) => void
}

/** What [`onImagePaste`] decided, so a caller (and a check) can see it. */
export type PasteVerdict = 'claimed' | 'no-image' | 'read-only' | 'no-path'

/**
 * Decide what a `paste` carrying `types` should do in this host, and start it.
 *
 * Answers synchronously — the caller needs to know whether to `preventDefault` before the
 * write has begun — and the write runs on its own.
 */
export function onImagePaste(host: ImagePasteHost, types: readonly string[]): PasteVerdict {
  if (!wantsImagePaste(types)) return 'no-image'
  // Read-only is checked before the path, because a revision buffer has both a path and no
  // business growing a file beside the working copy.
  if (host.readOnly) return 'read-only'
  const dir = host.docPath === null ? null : imageDirFor(host.docPath)
  if (dir === null || host.docPath === null) return 'no-path'

  const docPath = host.docPath
  const languageId = host.languageId
  void host.write(dir).then((written) => {
    if (written === null) return
    host.insert(referenceFor(languageId, relativeTo(docPath, written)))
  })
  return 'claimed'
}
