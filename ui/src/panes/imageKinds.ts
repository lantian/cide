/**
 * Which files open in the image viewer, and how that viewer describes what it is showing.
 *
 * The whole of the frontend's opinion about images lives here, and it is deliberately a
 * *pure* module with no imports: `PaneBody` has to choose between `EditorPane` and
 * `ImagePane` on its **first render**, before any IPC has happened, so the choice can only be
 * made from the path. Keeping the rule out of the component is also what lets
 * `scripts/check-image.mjs` compile it standalone and exercise it, which is the only test
 * runner this frontend has.
 *
 * # Why the extension decides the viewer and the bytes decide everything else
 *
 * There are two questions and they are answered on different sides:
 *
 * | question | answered by | with |
 * | --- | --- | --- |
 * | which pane does this tab render? | here | the file's name |
 * | what is actually in the file? | `cide_core::image::sniff` | the file's first 8 KiB |
 *
 * Splitting them is what makes "a `.png` that is really a JPEG" render correctly *and*
 * describe itself honestly, and it is what makes "a `.png` that is really a zip" a sentence in
 * the pane rather than a blank rectangle. A single table on either side collapses the two: put
 * it only in Rust and the pane cannot decide what to mount; put it only here and nothing ever
 * checks the claim.
 *
 * `ImageKind` is therefore *not* imported from `ipc/generated`, even though `ImageFormat` is
 * the same seven strings. This module must stay import-free. `ImagePane.tsx` assigns one to
 * the other at a typed call site, so a variant added on the Rust side and forgotten here is a
 * compile error rather than a drift.
 */

/**
 * The formats the image pane claims it can display.
 *
 * Exactly what a WebKitGTK `<img>` decodes without help, which is what the pane uses. Note
 * that `jpg` and `jpeg` are one kind, and that this is a list of *kinds*, not of extensions —
 * `IMAGE_EXTENSIONS` below is the many-to-one map.
 */
export type ImageKind = 'png' | 'jpeg' | 'gif' | 'webp' | 'bmp' | 'ico' | 'svg'

/**
 * Extension (lower case, no dot) to viewer.
 *
 * The user asked for "png, jpeg, gif, webp, bmp, svg and ico" and that is what this is, plus
 * the `jpg` spelling of `jpeg`, which is the common one and would be a bizarre omission.
 *
 * Four candidates were left out on purpose, and each would fail *silently* — the failure mode
 * this feature exists to avoid — rather than usefully:
 *
 * * `tiff` / `tif`: not decoded by WebKitGTK at all, so the tab would show a broken image
 *   where today it shows a readable refusal from Rust.
 * * `avif`, `jxl`: decoded only by some builds of the engine. A format that works on the
 *   developer's machine and not the user's is worse than one that works nowhere.
 * * `svgz`: a gzipped SVG. `<img>` would need the response to carry `Content-Encoding: gzip`,
 *   and Tauri's asset protocol sets `Content-Type` and nothing else, so the decoder would be
 *   handed gzip bytes and quietly give up.
 *
 * Adding one is a two-line change here plus a signature in `cide_core::image::sniff` — and
 * doing only the first half gets you a pane that mounts and a backend that refuses, which is
 * at least a sentence on screen.
 */
const IMAGE_EXTENSIONS: Readonly<Record<string, ImageKind>> = {
  png: 'png',
  jpg: 'jpeg',
  jpeg: 'jpeg',
  gif: 'gif',
  webp: 'webp',
  bmp: 'bmp',
  ico: 'ico',
  svg: 'svg',
}

/**
 * The viewer for a path, or `null` for "this is not an image; open it in the editor".
 *
 * `null` rather than a thrown error or a `'text'` member, because the caller is a `? :` in a
 * render function and every non-image path in the app goes through it.
 *
 * The rules that are easy to get wrong, each pinned by `check-image.mjs`:
 *
 * * **The extension is taken from the basename**, so `~/photos.png/notes.txt` is a text file
 *   and `~/v1.2/logo` is not a `2/logo`-something. Both separators are handled because a
 *   `TabKind::File` path is whatever the platform produced.
 * * **A leading dot is a dotfile, not an extension.** A file literally named `.png` is a
 *   config file, and treating it as an image would open an unreadable pane over it.
 * * **Case-insensitive.** `SCREENSHOT.PNG` comes off a camera, a Windows share, or a
 *   colleague, and is the same file.
 * * **A trailing dot is not an extension.** `logo.` has an empty extension, not the previous
 *   segment's.
 */
export function imageKindFor(path: string): ImageKind | null {
  const cut = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'))
  const name = cut >= 0 ? path.slice(cut + 1) : path
  const dot = name.lastIndexOf('.')
  // `dot <= 0` covers both "no dot at all" and "the only dot starts the name".
  if (dot <= 0 || dot === name.length - 1) return null
  return IMAGE_EXTENSIONS[name.slice(dot + 1).toLowerCase()] ?? null
}

/** How each kind names itself in the readout. Not `toUpperCase()`: WebP is spelled that way. */
const LABELS: Readonly<Record<ImageKind, string>> = {
  png: 'PNG',
  jpeg: 'JPEG',
  gif: 'GIF',
  webp: 'WebP',
  bmp: 'BMP',
  ico: 'ICO',
  svg: 'SVG',
}

/** `PNG`, `WebP`, `SVG` — what the file *is*, per the bytes Rust sniffed. */
export function imageLabel(kind: ImageKind): string {
  return LABELS[kind]
}

/**
 * A file size a person can read: `812 B`, `245 KiB`, `3.4 MiB`.
 *
 * Binary units and binary prefixes, because every other size cide prints is binary — the
 * editor's and the image cap's refusals both say "MiB" — and a status bar that says `KB` next
 * to a refusal that says `MiB` invites the reader to work out which one is lying.
 *
 * One decimal place from a mebibyte up and none below it. Below a mebibyte the fraction is
 * noise; above it, `3 MiB` and `3.9 MiB` are meaningfully different to someone wondering why a
 * repository is large.
 */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return '—'
  if (bytes < 1024) return `${Math.round(bytes)} B`
  const kib = bytes / 1024
  if (kib < 1024) return `${Math.round(kib)} KiB`
  const mib = kib / 1024
  return `${mib.toFixed(1)} MiB`
}

/** Pixel dimensions as the `<img>` reported them, once it has actually decoded something. */
export interface ImageSize {
  readonly width: number
  readonly height: number
}

/**
 * The status bar's line for an image: `PNG · 1920 × 1080 · 245 KiB`.
 *
 * This is what replaces `Rust · UTF-8 · LF · Ln 7, Col 48` while an image tab is in front —
 * that readout's four facts are all about a text buffer, and three of them are meaningless
 * here. `ImagePane` claims the bar's slot through the same `claimStatusReadout` every editor
 * uses, so an image tab and a file tab compete for it under one rule rather than two.
 *
 * `size` is `null` until the browser has decoded the file, and stays `null` for an SVG that
 * declares neither `width`/`height` nor a `viewBox` — such a document genuinely has no
 * intrinsic size, and inventing WebKit's 300×150 fallback would be reporting a fact about the
 * engine as a fact about the file. Rather than a placeholder, the dimensions are simply
 * omitted; the format and the byte size are true in every case.
 *
 * The multiplication sign is U+00D7, not the letter x: this is a dimension, and the bar's font
 * renders it a pixel narrower and centred, which is the whole reason editors use it.
 */
export function imageDetail(kind: ImageKind, bytes: number, size: ImageSize | null): string {
  const parts = [imageLabel(kind)]
  if (size !== null && size.width > 0 && size.height > 0) {
    parts.push(`${size.width} × ${size.height}`)
  }
  parts.push(formatBytes(bytes))
  return parts.join(' · ')
}

/**
 * What the pane says when the browser could not decode a file Rust already vouched for.
 *
 * Reachable, and not a theoretical branch: `cide_core::image::sniff` reads the first 8 KiB, so
 * a **truncated** PNG — an interrupted download, a partial `git-lfs` checkout, a file an agent
 * is still writing — has a perfectly good signature and no pixels behind it. Rust says yes,
 * the decoder says nothing at all, and without this the pane is the blank rectangle the whole
 * feature was written to avoid.
 *
 * Named rather than inlined so `check-image.mjs` can assert it mentions the file and does not
 * blame the user's disk for what is, most often, a half-written file.
 */
export function undecodableMessage(name: string, kind: ImageKind): string {
  return (
    `${name} is ${imageLabel(kind)} by its header, but the image could not be decoded. `
    + `The file is most likely truncated or still being written.`
  )
}
