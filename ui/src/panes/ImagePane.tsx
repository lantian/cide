/**
 * A file tab's pane when the file is a picture. (M18)
 *
 * > *"need render images when opening them"*
 *
 * # What this replaced
 *
 * Nothing, which is the point. A file tab had exactly one pane — `EditorPane` — and exactly
 * one reader behind it, `cide_core::document::read`, which refuses any file with a NUL byte in
 * its first 8 KiB. So opening `logo.png` produced a permanent tab reading **"logo.png looks
 * like a binary file"**. Not mojibake and not a crash: a refusal, correctly worded for the
 * question it was asked, which was the wrong question. SVG was the exception — it is text, so
 * it opened as XML source.
 *
 * # The three decisions worth knowing before editing this file
 *
 * **1. The pixels never cross the IPC.** `image.read` answers identity — format, byte size,
 * stamp, canonical path — and widens Tauri's asset-protocol scope to admit that one file. The
 * `<img>` then fetches `asset://localhost/…`, which wry serves as a streaming response off the
 * webview's own thread. `crates/cide-app/src/cmd/file.rs::image_read` carries the full
 * argument, including what a base64 `data:` URI would have cost and how the CSP was verified
 * to admit this. If you are tempted to "simplify" this into one command that returns the
 * bytes, read that comment first.
 *
 * **2. SVG goes in an `<img>`, never inline.** An `<img>` puts SVG in the SVG spec's *secure
 * static mode*: no script execution, no external references, no interactivity — the browser
 * enforces it, and it is the reason every site that shows user-uploaded SVG does it this way.
 * Inlining the source (`dangerouslySetInnerHTML`, or an `<svg>` element built from the text)
 * would execute any `<script>` inside the file **with this webview's origin**, and this
 * origin can `invoke` every Tauri command cide has — the file tree, `file_write`, session
 * spawn. A repository is full of SVGs nobody wrote, so this is not a hypothetical. The cost of
 * the safe choice is real and accepted: an `<img>` will not load an SVG's external references
 * (a web font, another file's `<use>`), so such a document renders with pieces missing rather
 * than wrongly. Showing it as a working picture with a hole in it beats showing it as a
 * working picture that ran somebody's script.
 *
 * **3. It is a document, so it is never dirty and never saves.** Nothing here calls
 * `file.setDirty`, so the tab's `dirty` flag stays `false` and `close_tab` never guards it;
 * nothing here calls `registerBuffer`, so a Ctrl+S with this tab in front finds no editor for
 * the tab and is a silent no-op — which is exactly what `keys/dispatch.ts` already does for a
 * Claude tab. Both are absences, and absences rot, so they are written down: an image pane
 * that starts registering a buffer would offer *Save* on a file it cannot serialise.
 *
 * Everything else about the tab is inherited rather than reimplemented. It is an ordinary
 * `TabKind::File`, so it is closable, splittable, draggable, in the tab strip, on the
 * closed-tab stack for Ctrl+Shift+T, and in the navigation history — none of which is code in
 * this file, and all of which would have had to be rebuilt behind a parallel `TabKind::Image`.
 */
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { image as imageApi } from '@/ipc/client'
import { describe } from '@/chrome/notices'
import { claimStatusReadout, pathTrail, type ReadoutSlot } from '@/editor/statusReadout'
import type { ImageDoc } from '@/ipc/generated'
import {
  formatBytes,
  imageDetail,
  imageKindFor,
  imageLabel,
  undecodableMessage,
  type ImageKind,
  type ImageSize,
} from './imageKinds'
import styles from './ImagePane.module.css'

export interface ImagePaneProps {
  /** Absolute path of the file this tab shows — the path as the user asked for it. */
  path: string
  /** The project root, so the status bar's trail is repo-relative. */
  root?: string | undefined
  /**
   * Whether this pane's tab is the one in front.
   *
   * Same input, same reason, as `EditorPane`: a background tab is `visibility: hidden`, not
   * unmounted, so its panes are mounted and painting and nothing below can tell them from the
   * visible one. Without it the status bar would be claimed by whichever tab's `image_read`
   * happened to resolve last. See `editor/statusReadout.ts`.
   */
  onScreen?: boolean | undefined
}

/** What the pane is showing. The fourth state — decoded — is `size` below, not a `Load`. */
type Load =
  | { kind: 'loading' }
  | { kind: 'ready'; doc: ImageDoc; src: string }
  | { kind: 'failed'; why: string }

/** The last path segment, for sentences that should name the file rather than its whole path. */
function basename(path: string): string {
  const cut = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'))
  return cut >= 0 ? path.slice(cut + 1) : path
}

export function ImagePane({ path, root, onScreen }: ImagePaneProps): ReactNode {
  const [load, setLoad] = useState<Load>({ kind: 'loading' })
  /** Pixel dimensions, measured by the decoder rather than parsed out of the header. */
  const [size, setSize] = useState<ImageSize | null>(null)
  /** Fit-to-pane (the default) or 1:1. See `.canvas` in the stylesheet for what each does. */
  const [actualSize, setActualSize] = useState(false)
  /**
   * Bumped by *Reload*, and the only thing that re-runs the read.
   *
   * There is no filesystem subscription here, deliberately. `cide://fs-changed` is per project
   * and covers the indexed roots only, so it would refresh an image inside the project and
   * silently not refresh one under *External Libraries* or one opened out of project — two
   * behaviours for one button, with the broken half invisible. A gesture that always works is
   * the honest version, and it is what makes `ImageDoc::stamp`'s cache-buster load-bearing:
   * the re-read produces a new mtime, which produces a new URL, which is the only way WebKit
   * will fetch bytes it has already cached under the old one.
   */
  const [reloads, setReloads] = useState(0)

  /**
   * Which viewer this path routes to, which is also the label used before the bytes are known.
   *
   * Non-null by construction — `PaneBody` only mounts this component when `imageKindFor`
   * answered — but the type says `| null` and the fallback is honest rather than a `!`.
   */
  const nameKind = imageKindFor(path)

  useEffect(() => {
    let live = true
    setLoad({ kind: 'loading' })
    setSize(null)
    imageApi
      .read(path)
      .then((doc) => {
        if (!live) return
        setLoad({ kind: 'ready', doc, src: imageApi.srcUrl(doc) })
      })
      .catch((error: unknown) => {
        if (!live) return
        /*
         * `describe`, and **not** `String(error)`.
         *
         * `CoreError` is `#[serde(tag = "kind", content = "detail")]`, so every refusal from
         * `cide_core::image::read` arrives as `{kind: "io", detail: "<the sentence>"}` — an
         * object with no `toString`, which `String()` renders as the literal text
         * `[object Object]`. That is not a smaller version of the sentence; it is the blank
         * pane this feature was written to avoid, wearing different words. `chrome/notices.ts`
         * already carries the unwrapping and the write-up of the bug that produced it.
         */
        setLoad({ kind: 'failed', why: describe(error) })
      })
    return () => {
      live = false
    }
  }, [path, reloads])

  /**
   * The format as the **bytes** report it, which may not be what the name claimed.
   *
   * This assignment is the drift gate between the two enums. `imageKinds.ts` is import-free by
   * design (see its header) so it declares `ImageKind` itself, and `ImageFormat` is generated
   * from Rust; a variant added on one side and forgotten on the other is a type error here
   * rather than a string that falls through `LABELS` and renders `undefined`.
   */
  const kind: ImageKind | null = load.kind === 'ready' ? load.doc.format : nameKind

  const detail = useMemo(
    () => (load.kind === 'ready' && kind !== null ? imageDetail(kind, load.doc.bytes, size) : ''),
    [load, kind, size],
  )
  const trail = useMemo(() => pathTrail(path, root), [path, root])

  /*
   * The status bar's single slot, claimed the same way every editor claims it.
   *
   * This is the "show something useful in the chrome" half, and claiming the *existing* slot
   * rather than adding a second readout is what keeps one rule for who owns the bar: a split
   * with an image on the left and a buffer on the right hands the slot to whichever the user is
   * in, because both went through `claimStatusReadout`. A parallel image-only readout would
   * have needed its own precedence rule against that stack, and two precedence rules over one
   * strip of text is how a status bar starts flickering between two files.
   *
   * What it says is `PNG · 1920 × 1080 · 245 KiB` where a buffer says
   * `Rust · UTF-8 · LF · Ln 7, Col 48`. The line-and-column readout is meaningless for a
   * picture, and so are the encoding and the line endings; the format, the pixel dimensions and
   * the byte size are the three facts an image editor puts there.
   */
  const slot = useRef<ReadoutSlot | null>(null)
  /*
   * Read by the claim effect below, which must not re-run when the detail changes — a re-claim
   * pushes this pane to the top of the readout stack, and the dimensions arriving is not a
   * reason to steal the bar from the pane the user is actually in.
   */
  const detailNow = useRef(detail)
  detailNow.current = detail

  useEffect(() => {
    if (load.kind !== 'ready') return
    const claim = claimStatusReadout(path, trail, detailNow.current, onScreen !== false)
    slot.current = claim
    return () => {
      claim.release()
      slot.current = null
    }
    // `trail` and `onScreen` are read at claim time and maintained by the two effects below;
    // listing them here would re-claim on every root change and on every tab switch.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [load.kind, path])

  useEffect(() => {
    slot.current?.set(detail)
  }, [detail])

  useEffect(() => {
    // The root can arrive after the pane mounts — `PaneBody` passes `roots[0]?.path ??
    // PROJECT_ROOT` — and a trail computed against the fallback would show an absolute path
    // for the rest of the session. Every segment is a path segment here: an image has no
    // symbol tail, so `pathCount` is the whole length.
    slot.current?.setTrail(trail, trail.length)
  }, [trail])

  useEffect(() => {
    if (onScreen === true) slot.current?.focus()
  }, [onScreen])

  const onDecoded = useCallback((el: HTMLImageElement) => {
    // `naturalWidth`/`naturalHeight` rather than a header parse in Rust, because this is the
    // decoder's own answer and therefore the only one that cannot disagree with what is on
    // screen. It is also the only way to get a straight answer for ICO (several sizes in one
    // file; the decoder picks) and for SVG (no pixels at all until it is laid out).
    //
    // Zero means "no intrinsic size" — an SVG with neither width/height nor a viewBox — and is
    // reported as no dimensions rather than as WebKit's 300×150 fallback, which is a fact about
    // the engine and not about the file. `imageDetail` drops the pair in that case.
    setSize({ width: el.naturalWidth, height: el.naturalHeight })
  }, [])

  if (load.kind === 'loading') {
    return <div className={styles.notice}>Opening {basename(path)}…</div>
  }

  if (load.kind === 'failed') {
    return (
      <div className={styles.notice}>
        <div className={styles.noticeTitle}>This image could not be shown</div>
        <div className={styles.noticeWhy}>{load.why}</div>
        <button type="button" className={styles.button} onClick={() => setReloads((n) => n + 1)}>
          Try again
        </button>
      </div>
    )
  }

  const name = basename(path)
  return (
    <div className={styles.pane}>
      <div className={actualSize ? `${styles.canvas} ${styles.actual}` : styles.canvas}>
        <img
          className={styles.image}
          src={load.src}
          /*
           * The filename, not "image" and not the path. A screen reader reading a pane whose
           * whole content is one picture needs to know *which* picture, and the alt text is the
           * only place that fact exists in the accessibility tree — the tab strip is in another
           * part of the document entirely.
           */
          alt={name}
          /*
           * `async` rather than the default. Decoding happens off the main thread, which in
           * this app is the thread that also runs every terminal in the window: a synchronous
           * decode of a large PNG is a visible stall in an unrelated `claude` session.
           */
          decoding="async"
          draggable={false}
          onLoad={(e) => onDecoded(e.currentTarget)}
          /*
           * Reachable, and not a theoretical branch. Rust vouched for the first 8 KiB, so a
           * truncated file — an interrupted download, a partial `git-lfs` checkout, a PNG an
           * agent is still writing — has a perfect signature and no pixels behind it. Without
           * this the pane is the blank rectangle this whole feature was written to avoid.
           */
          onError={() =>
            setLoad({
              kind: 'failed',
              why: undecodableMessage(name, load.doc.format),
            })
          }
        />
      </div>
      {/*
        * The pane's own footer, which is not a duplicate of the status bar above.
        *
        * The status bar has one slot for the whole window, so in a split showing two images it
        * describes exactly one of them. This strip says which picture *this* pane is holding,
        * and it is where the two controls live: the zoom toggle, and the reload that is the
        * only way to see bytes something rewrote under an open tab.
        */}
      <div className={styles.bar}>
        <span className={styles.name} title={path}>
          {name}
        </span>
        <span className={styles.facts}>
          {imageLabel(load.doc.format)}
          {size !== null && size.width > 0 ? ` · ${size.width} × ${size.height}` : ''}
          {` · ${formatBytes(load.doc.bytes)}`}
        </span>
        <span className={styles.spacer} />
        <button
          type="button"
          className={styles.button}
          onClick={() => setActualSize((on) => !on)}
          /*
           * The label names the state it will move to, not the state it is in. "Fit" while
           * already fitted is a button that appears to do nothing, which is the single most
           * common way a toggle gets reported as broken.
           */
          title={actualSize ? 'Scale the image down to fit the pane' : 'Show the image at 1:1'}
        >
          {actualSize ? 'Fit' : '1:1'}
        </button>
        <button
          type="button"
          className={styles.button}
          onClick={() => setReloads((n) => n + 1)}
          title="Read the file again — use this after something else has rewritten it"
        >
          Reload
        </button>
      </div>
    </div>
  )
}
