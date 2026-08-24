/**
 * The markdown pane: the buffer, the rendering, the divider between them, and the switch. (M20)
 *
 * `EditorPane` renders this **only** for a file the editor calls Markdown, and passes the
 * `EditorSurface` in as `children`. A `.rs` pane's tree is therefore byte-identical to what it
 * was before this feature existed, which is the property that makes the whole thing safe to add
 * to a component as load-bearing as `EditorPane`.
 *
 * # The buffer is never unmounted, in any layout
 *
 * In `preview` the surface is still in the tree, still laid out at the pane's full size, and
 * merely `visibility: hidden` with the rendering painted over it. Unmounting it would throw away
 * unsaved edits, the undo history, the find state, the LSP document and the autosave timer — and
 * `layout/paneHosts.ts` has the general form of this rule for terminals, including why the
 * tempting `display: none` is worse than useless: "measurements read zero and `fit()` corrupts
 * the child". CodeMirror measures the same way. Keeping the surface at unchanged size also means
 * switching text ⇄ preview costs no reflow at all; only `split` re-measures, which is what a
 * splitter drag already costs.
 *
 * # The divider commits once
 *
 * A drag writes `flex-basis` straight onto the DOM node and calls `setSplitRatio` on
 * `pointerup`. That is `check:resize`'s rule and its reason: the expensive reactions to a size
 * change — CodeMirror re-measuring a wrapped document, this component rebuilding the anchor
 * table — must be deferred to the end of the gesture, or the app locks up while somebody drags.
 */
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type JSX,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from 'react'
import {
  beginResizeGesture,
  cancelResizeSettle,
  endResizeGesture,
  whenResizeSettles,
} from '@/layout/resizeGesture'
import { notify } from '@/chrome/notices'
import { writeClipboard } from '../clipboard'
import { utf8ByteLength } from '../byteSize'
import { parseMarkdown } from './blocks'
import { MarkdownPreview } from './MarkdownPreview'
import { clearImages } from './images'
import { dirOf, resolveLocal, targetKind } from './links'
import {
  claimDriver,
  newLatch,
  previewTopFor,
  releaseDriver,
  sourceLineFor,
  type Anchor,
  type SyncGeometry,
} from './scrollSync'
import {
  MD_VIEWS,
  MD_VIEW_LABELS,
  MD_CONTROL_MIN_PX,
  MD_SPLIT_MIN_PX,
  PREVIEW_LIMIT_BYTES,
  clampRatio,
  effectiveView,
  setSplitRatio,
  splitRatio,
  subscribeSplitRatio,
} from './view'
import type { MarkdownDoc, MdView } from './types'
import styles from './MarkdownFrame.module.css'
import { Icon, type IconName } from '@/icons/Icon'

/** How long after the last keystroke the preview re-parses. */
const PARSE_IDLE_MS = 120

export interface MarkdownFrameProps {
  /** The markdown file's absolute path. */
  path: string
  /** The layout the user chose. */
  view: MdView
  onView: (next: MdView) => void
  /** Read the buffer as it is now. The same closure `EditorPane` gets from `onDocChanged`. */
  readText: () => string
  /**
   * Be told when the buffer changes.
   *
   * A subscription and **not** a `revision: number` prop, which was the first shape and was
   * wrong: a counter bumped on every keystroke is a `setState` on every keystroke in
   * `EditorPane`, which re-renders the editor for every file in the app — including the `.rs`
   * ones this feature is supposed to leave untouched. `EditorSurface` avoids passing text through
   * React for exactly this reason (`onDocChanged` hands out a reader rather than a string), and
   * this is the same trade one level up.
   */
  subscribeText: (listener: () => void) => () => void
  /** Whether this tab is the one in front. A background tab parses nothing. */
  onScreen: boolean
  /** Scroll the buffer to a line, without moving the caret. Null before the view is built. */
  scrollEditorTo: (line: number) => void
  /**
   * Register the "the buffer is now looking at line N" sink.
   *
   * A **callback registration** and not a `topLine` prop, deliberately: `viewTracker.ts` reports
   * once per animation frame, and a prop would re-render this component — and with it the whole
   * rendered document — sixty times a second while somebody scrolls.
   */
  onSyncHandle: (follow: ((topLine: number) => void) | null) => void
  /** Open a local path as a tab. What a `[link](../adr/0009.md)` does. */
  openPath: (path: string) => void
  /** The `EditorSurface`. */
  children: ReactNode
}

export function MarkdownFrame({
  path,
  view,
  onView,
  readText,
  subscribeText,
  onScreen,
  scrollEditorTo,
  onSyncHandle,
  openPath,
  children,
}: MarkdownFrameProps): JSX.Element {
  const rootRef = useRef<HTMLDivElement | null>(null)
  const bufferRef = useRef<HTMLDivElement | null>(null)
  const previewRef = useRef<HTMLDivElement | null>(null)

  const [width, setWidth] = useState(0)
  const ratio = useSyncExternalStore(subscribeSplitRatio, splitRatio, splitRatio)
  const { layout, overruled } = effectiveView(view, width)
  const showPreview = layout !== 'text'

  /* --- the document ----------------------------------------------------------------------- */

  const [doc, setDoc] = useState<MarkdownDoc | null>(null)
  const [tooBig, setTooBig] = useState(false)

  /*
   * Parsed on an idle timer rather than on every keystroke, and not at all while the tab is
   * behind another one. `TabContent` keeps every tab mounted so switching is instant, so without
   * the `onScreen` test a background README would re-parse on every change an agent made to it.
   */
  useEffect(() => {
    if (!showPreview || !onScreen) return undefined
    let timer: ReturnType<typeof setTimeout> | undefined
    const reparse = (): void => {
      const text = readText()
      const over = utf8ByteLength(text) > PREVIEW_LIMIT_BYTES
      setTooBig(over)
      setDoc(over ? null : parseMarkdown(text))
    }
    const schedule = (): void => {
      if (timer !== undefined) clearTimeout(timer)
      timer = setTimeout(reparse, PARSE_IDLE_MS)
    }
    // Once for what is on screen now, then on every change. The first pass is scheduled rather
    // than immediate so that opening a previewed file does not parse it on the same frame as
    // CodeMirror is building its view.
    schedule()
    const off = subscribeText(schedule)
    return () => {
      if (timer !== undefined) clearTimeout(timer)
      off()
    }
  }, [showPreview, onScreen, readText, subscribeText])

  // A pane that stops previewing drops the parse; a document held for a tab nobody is looking at
  // is the largest object this feature allocates.
  useEffect(() => {
    if (!showPreview) setDoc(null)
  }, [showPreview])

  useEffect(() => clearImages, [])

  /* --- geometry --------------------------------------------------------------------------- */

  const geometryRef = useRef<SyncGeometry>({
    anchors: [],
    contentHeight: 0,
    viewportHeight: 0,
    docLines: 1,
  })

  /**
   * Read every `data-line` back off the DOM and turn it into the anchor table.
   *
   * Once per render and once per *settled* resize, never per scroll frame. `check:resize` exists
   * because of exactly this: a `getBoundingClientRect` per block per frame is the version of
   * this feature that locks the window up while somebody flicks a trackpad.
   */
  const measure = useCallback(() => {
    const host = previewRef.current
    if (host === null) return
    const anchors: Anchor[] = []
    for (const element of host.querySelectorAll<HTMLElement>('[data-line]')) {
      const line = Number.parseInt(element.dataset.line ?? '', 10)
      if (!Number.isFinite(line)) continue
      anchors.push({ line, top: element.offsetTop })
    }
    geometryRef.current = {
      anchors,
      contentHeight: host.scrollHeight,
      viewportHeight: host.clientHeight,
      docLines: doc === null ? 1 : Math.max(1, doc.lines[doc.lines.length - 1] ?? 1),
    }
  }, [doc])

  useEffect(() => {
    measure()
  }, [measure, layout, ratio, width])

  /* --- scroll sync ------------------------------------------------------------------------- */

  const latch = useRef(newLatch())

  useEffect(() => {
    if (layout !== 'split') {
      onSyncHandle(null)
      releaseDriver(latch.current)
      return undefined
    }
    const follow = (topLine: number): void => {
      const host = previewRef.current
      if (host === null) return
      if (!claimDriver(latch.current, 'editor', Date.now())) return
      host.scrollTop = previewTopFor(geometryRef.current, topLine)
    }
    onSyncHandle(follow)
    return () => {
      onSyncHandle(null)
    }
  }, [layout, onSyncHandle])

  const onPreviewScroll = useCallback(() => {
    const host = previewRef.current
    if (host === null || layout !== 'split') return
    if (!claimDriver(latch.current, 'preview', Date.now())) return
    scrollEditorTo(sourceLineFor(geometryRef.current, host.scrollTop))
  }, [layout, scrollEditorTo])

  /* --- the pane's width -------------------------------------------------------------------- */

  useEffect(() => {
    const root = rootRef.current
    if (root === null || typeof ResizeObserver === 'undefined') return undefined
    /*
     * Settled, not live. `layout/resizeGesture.ts` is the module `check:resize` protects, and
     * the reason it exists is that reacting to every frame of a divider drag — here, re-measuring
     * the anchor table and re-deciding whether split fits — is what makes a drag unusable.
     */
    const observer = new ResizeObserver(() => {
      whenResizeSettles(root, () => {
        setWidth(root.clientWidth)
        measure()
      })
    })
    observer.observe(root)
    setWidth(root.clientWidth)
    return () => {
      observer.disconnect()
      // Load-bearing rather than tidy, for the reason `cancelResizeSettle` states: a deferred
      // callback that runs after this pane has gone would measure a detached node and publish a
      // width of zero, which `effectiveView` reads as "not narrow" and this component reads as a
      // reason to re-render.
      cancelResizeSettle(root)
    }
  }, [measure])

  /* --- the divider -------------------------------------------------------------------------- */

  const onDividerDown = useCallback((event: ReactPointerEvent<HTMLDivElement>) => {
    const root = rootRef.current
    const buffer = bufferRef.current
    if (root === null || buffer === null) return
    event.preventDefault()
    const box = root.getBoundingClientRect()
    const divider = event.currentTarget
    divider.setPointerCapture(event.pointerId)
    /*
     * The drag is a resize gesture, announced to the module that exists to know about them.
     * Everything expensive that a size change wakes — xterm's `fit()`, a `session_resize` that
     * reflows scrollback on the IPC thread, a minimap repaint, this frame's own anchor table —
     * defers until `endResizeGesture`. Not announcing it is the shape `check:resize` exists to
     * catch: nothing throws, nothing looks wrong, and the window simply locks up mid-drag.
     */
    beginResizeGesture()

    let next = clampRatio((event.clientX - box.left) / box.width)
    const move = (moved: PointerEvent): void => {
      next = clampRatio((moved.clientX - box.left) / box.width)
      // Written straight onto the node: a `setState` per pointermove is a React render per
      // frame of the drag, and every one of them re-renders the rendered document.
      buffer.style.flexBasis = `${next * 100}%`
    }
    const up = (): void => {
      divider.removeEventListener('pointermove', move)
      divider.removeEventListener('pointerup', up)
      divider.removeEventListener('pointercancel', up)
      buffer.style.flexBasis = ''
      setSplitRatio(next)
      endResizeGesture()
    }
    divider.addEventListener('pointermove', move)
    divider.addEventListener('pointerup', up)
    divider.addEventListener('pointercancel', up)
  }, [])

  /* --- links --------------------------------------------------------------------------------- */

  const dir = useMemo(() => dirOf(path), [path])

  const onNavigate = useCallback(
    (href: string) => {
      const kind = targetKind(href)
      if (kind === 'fragment') {
        const id = href.slice(1)
        const host = previewRef.current
        // An empty fragment is `[top](#)`, which means the top and would otherwise be a
        // `querySelector('#')` — a syntax error, thrown out of a click handler with no boundary
        // above it.
        if (id === '') {
          host?.scrollTo({ top: 0 })
          return
        }
        host?.querySelector(`#${CSS.escape(id)}`)?.scrollIntoView({ block: 'start' })
        return
      }
      if (kind === 'local') {
        const resolved = resolveLocal(dir, href)
        if (resolved === null) {
          notify(`cide cannot resolve ${href} from this file`, { kind: 'warn' })
          return
        }
        openPath(resolved)
        return
      }
      /*
       * An external URL. cide does not navigate to one, and `terminal/xterm.ts` has the
       * argument at length for the identical case in terminal output: opening it for real
       * "would have to go through Rust and `tauri_plugin_opener`, because the JS opener command
       * is capability-gated per window and a detached-pane window deliberately has no `opener`
       * permission, so a JS-side open would work in the shell window and silently do nothing in
       * a torn-out pane".
       *
       * Refusing *silently* is the defect this project keeps finding — "a link that underlines,
       * takes a click and does nothing is indistinguishable from one wired to nothing" — and the
       * terminal's answer is a notice telling the user to copy the address. Here the address can
       * simply be copied for them, which is that hint carried out rather than recited.
       */
      void writeClipboard(href).then((copied) => {
        notify(
          copied
            ? `Copied ${href} — cide does not open web links from a document`
            : `cide does not open web links from a document: ${href}`,
          { kind: copied ? 'ok' : 'warn', hint: copied ? undefined : 'Copy the address by hand.' },
        )
      })
    },
    [dir, openPath],
  )

  /* --- render -------------------------------------------------------------------------------- */

  return (
    <div ref={rootRef} className={styles.frame} data-layout={layout}>
      <div
        ref={bufferRef}
        className={styles.buffer}
        style={layout === 'split' ? { flexBasis: `${ratio * 100}%` } : undefined}
        /*
         * `visibility` and never `display: none`. The buffer keeps its measured size in every
         * layout, so CodeMirror never sees a zero-width viewport and never re-wraps a document
         * to nothing — the same rule `layout/paneHosts.ts` states for hidden terminal tabs.
         */
        data-hidden={layout === 'preview' ? 'true' : undefined}
      >
        {children}
      </div>

      {layout === 'split' ? (
        <div
          className={styles.divider}
          role="separator"
          aria-orientation="vertical"
          aria-label="Preview width"
          onPointerDown={onDividerDown}
        />
      ) : null}

      {showPreview ? (
        <div
          ref={previewRef}
          className={styles.preview}
          onScroll={onPreviewScroll}
          data-audit="markdownPreview"
        >
          {tooBig ? (
            <p className={styles.notice}>
              This file is larger than {Math.round(PREVIEW_LIMIT_BYTES / 1024)} KB, which is the
              size past which cide stops highlighting a buffer as well. Editing it still works.
            </p>
          ) : doc === null ? null : (
            <MarkdownPreview doc={doc} path={path} onNavigate={onNavigate} />
          )}
        </div>
      ) : null}

      <ViewSwitch
        view={view}
        onView={onView}
        overruled={overruled}
        corner={width === 0 || width >= MD_CONTROL_MIN_PX ? 'top' : 'bottom'}
      />
    </div>
  )
}

/**
 * The floating three-way switch.
 *
 * Revealed on hover like the ⊞ ⛶ ⧉ × cluster it sits beside, and **always** visible while the
 * layout is not `text` — a control that hides itself is a control the user cannot use to undo
 * what it just did.
 *
 * `corner` is not cosmetic. The pane's top-right is spoken for: `layout/PaneTitleBar.module.css`
 * floats the pane controls there and an editor pane reserves `--pane-corner-clear` — 221px, the
 * minimap's 96px plus the cluster's 125px — for them. `panes/EditorPane.module.css` carries the
 * write-up of the once a strip reserved 125 instead and put both its buttons inside the ×'s live
 * hit box. Rather than squeeze three squares into what is left of a narrow pane, the switch drops
 * to the bottom-right, where nothing floats.
 */
function ViewSwitch({
  view,
  onView,
  overruled,
  corner,
}: {
  view: MdView
  onView: (next: MdView) => void
  overruled: boolean
  corner: 'top' | 'bottom'
}): JSX.Element {
  return (
    <div
      className={styles.switch}
      data-corner={corner}
      data-pinned={view === 'text' ? undefined : 'true'}
      role="group"
      aria-label="Markdown layout"
    >
      {MD_VIEWS.map((option) => (
        <button
          key={option}
          type="button"
          className={styles.switchButton}
          aria-pressed={view === option}
          aria-label={MD_VIEW_LABELS[option]}
          data-overruled={option === 'split' && overruled ? 'true' : undefined}
          title={
            option === 'split' && overruled
              ? `${MD_VIEW_LABELS[option]} — this pane is narrower than ${MD_SPLIT_MIN_PX}px, so it shows the preview alone`
              : MD_VIEW_LABELS[option]
          }
          onClick={() => {
            onView(option)
          }}
        >
          <ViewGlyph view={option} />
        </button>
      ))}
    </div>
  )
}

/**
 * The three glyphs, drawn rather than typed.
 *
 * Inline SVG rather than characters: there is no Unicode pair that says *buffer* / *buffer and
 * rendering* / *rendering* without one of the three being a near-twin of another at 14px, and
 * `icons/FileIcon.tsx` already records why a sprite reference is not the route here. `currentColor`
 * throughout, so the pressed and unpressed states are one colour rule and not two drawings.
 */
/*
 * The three-way view switcher's marks.
 *
 * This was a hand-drawn 14x14 SVG — a third grid in an app that had two already, with no
 * `stroke-width` at all (so the SVG default of 1 applied) and a `strokeWidth="1.6"` override on
 * two lines to fake a heading rule. It drew at a different weight from the activity rail and
 * from the git log's toggle, which is exactly the drift the vendored set exists to end.
 *
 * `file-code` for the source, `columns-2` for the split, `book-open-text` for the preview —
 * `columns-2` being the same mark the git toolbar's Show diff and the header's Split use, which
 * is the point: one idea, one picture, wherever it appears.
 */
const VIEW_MARK: Readonly<Record<MdView, IconName>> = {
  text: 'file-code',
  split: 'columns-2',
  preview: 'book-open-text',
}

function ViewGlyph({ view }: { view: MdView }): JSX.Element {
  return <Icon name={VIEW_MARK[view]} size={1} />
}
