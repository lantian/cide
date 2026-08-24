/**
 * The 24px status bar along the bottom of the window.
 *
 * Two of the mock's slots describe a language server, and v1 ships without one. Rather
 * than print the mock's `✗ 0 ⚠ 2` and `rust-analyzer` regardless, the diagnostics group
 * degrades to a visible gap and the server name is dropped altogether: a confident `✗ 0`
 * that really means "nobody looked" is the one failure mode a status bar cannot afford.
 *
 * Every field is a prop with a placeholder default. The bar reads nothing from the store,
 * so it stays a pure render target that a screenshot test can drive directly.
 *
 * The open file is the one thing that does not arrive as a prop. It comes from
 * `editor/statusReadout.ts`, which the editor pane feeds, and it arrives in two pieces
 * because they change at different rates:
 *
 * * `crates › cide-core › src › lib.rs` — the trail, in React state. It moves when the user
 *   switches file, which is rare, and it wants real markup: one element per segment, so the
 *   `›` separators can be their own colour, each segment can carry its own bidi direction
 *   inside a box that clips from the left, and — since M16 — a segment that leads somewhere can
 *   be a control while one that does not is plain text with no handler at all.
 * * `Rust · UTF-8 · LF · Ln 7, Col 48` — written straight into a DOM node this component
 *   owns. It moves on every caret move, which under a held arrow key is 30 times a second,
 *   and no version of re-rendering the bar that often is worth having.
 *
 * Both used to be a 28px row above every buffer. See `statusReadout.ts` for why they moved,
 * and `editor/EditorSurface.tsx` for the row that is no longer there.
 */
import { useEffect, useRef, useState } from 'react'
import { sameTrail, subscribeStatusReadout, type ReadoutLine } from '@/editor/statusReadout'
import { NO_DIAGNOSTICS_SOURCE } from '@/sidebar/ProblemsPanel/model'
import { crumbTargets } from '@/sidebar/rowPaths'
import { BranchSelector } from './BranchSelector'
import { Icon } from '@/icons/Icon'

import styles from './StatusBar.module.css'

export interface Diagnostics {
  errors: number
  warnings: number
}

export interface StatusBarProps {
  /*
   * No `branch` prop. It was here, defaulted to `'—'`, and nothing ever passed one — so the
   * slot printed a dash forever. `BranchSelector` reads the repository itself; a prop would be
   * a second source of truth for a value it already holds to draw its list.
   */
  /**
   * Lines added and removed in the working tree.
   *
   * **Nothing supplies these, and nothing ever has.** They defaulted to `0`, so the slot has
   * spent every launch claiming a clean tree with total confidence — which is the same failure
   * the `branch` prop above is a note about, and worse, because a dash is obviously a
   * placeholder and a zero is a measurement. They are `undefined` by default now and the slot
   * draws a dash, exactly as the diagnostics pair does when no source has looked.
   *
   * Wiring them is a backend change rather than a prop: there is no working-tree line count on
   * the wire at all. `cide_ipc::git`'s `insertions`/`deletions` belong to a *pull outcome*, and
   * `cide_git::show::line_counts` is commit-oriented — it takes a rev. A real figure needs a
   * diff of HEAD against the working tree, summed across every root, behind a command of its
   * own and a subscription that refreshes it as files change.
   */
  added?: number | undefined
  removed?: number | undefined
  /** `null` means no diagnostics source is running, which is distinct from zero of each. */
  diagnostics?: Diagnostics | null | undefined
  claude?: string | undefined
  /**
   * Every path the file tree can hang a row from — the project's roots plus each synthetic
   * group's top-level children, from `fs_reveal_roots` by way of `sidebar/treeStore.ts`. (M16)
   *
   * What it decides is which segments of the trail below are clickable, through the pure
   * `crumbTargets`. It is a *list of paths* rather than a predicate because that keeps this
   * component a render target: the rule lives in a module a check script compiles and runs, not
   * in a callback nothing can reach.
   */
  revealRoots?: readonly string[] | undefined
  /**
   * Select a crumb's path in the file tree, or absent when this window has no file tree.
   *
   * `App.tsx` routes it to `runCommand('file.reveal', { path })` — the same command the ⌃⇧E
   * chord, the palette row, the Explorer's ⌖ button and a Ctrl+click on a directory in terminal
   * output all run. A second reveal path with its own preconditions is how five gestures come to
   * behave in five ways, and this one would additionally have to know about External Libraries.
   *
   * **Absent means no crumb is revealable at all**, not "clicks are ignored": `dispatch.ts`
   * refuses `file.reveal` in a window with no sidebar, with a notice, so a detached-tab window
   * must draw the whole trail inert rather than offer a control that explains itself afterwards.
   * The same rule, for the same reason, that `App.tsx` gives for withholding `onRevealPath` from
   * a detached pane's terminal.
   */
  onRevealSegment?: ((path: string) => void) | undefined
}

/*
 * The wording now lives in `sidebar/ProblemsPanel/model.ts`, because the problems panel says
 * the same thing at length and these two are the only surfaces that speak for the missing
 * analyser. Two hand-kept copies of a user-visible claim are two claims, and this one had
 * already been written twice by the time the panel existed.
 *
 * The import goes chrome → sidebar, which is the wrong direction for a component but not for
 * this: `model.ts` is pure, importless data, so the bar stays a render target that pulls in
 * no store and no React tree. `ui/scripts/check-problems.mjs` fails if this file grows its
 * own copy of the sentence again.
 */
const DIAGNOSTICS_PENDING = NO_DIAGNOSTICS_SOURCE

const CLAUDE_PENDING = 'Session readout arrives from the Claude statusline hook.'

/*
 * Deliberately says nobody has counted, rather than pretending a clean tree.
 *
 * The same distinction the diagnostics pair draws: `— —` means no source looked, `0 0` means a
 * source looked and found nothing. This slot has only ever been able to say the first.
 */
const DIFF_PENDING = 'No working-tree line count is wired yet — this is not a claim of zero.'

/** Shown until a real readout exists, and the value the pending title keys off. */
const CLAUDE_PLACEHOLDER = 'claude · —'

/** The crumb half of the readout, which is the half that goes through React state. */
type Crumbs = Pick<ReadoutLine, 'trail' | 'pathCount' | 'file'>

const NO_CRUMBS: Crumbs = { trail: [], pathCount: 0, file: '' }

/**
 * The three fields together, because they are drawn together.
 *
 * `sameTrail` alone is not enough any more and the gap is silent: two buffers can produce the
 * *same* segments — `src › lib.rs` under two different roots is the ordinary case in this
 * repository — so a switch between them would keep the previous `file`, and every crumb would
 * reveal a row in the wrong tree.
 */
function sameCrumbs(a: Crumbs, b: Crumbs): boolean {
  return a.file === b.file && a.pathCount === b.pathCount && sameTrail(a.trail, b.trail)
}

/**
 * A drag that ended on a crumb is a selection, not a click.
 *
 * The trail became selectable text in M15 (`0033f33`, "a notice's text can be selected and
 * copied"), and copying a path out of the bar is a thing people do. Without this, releasing the
 * mouse at the end of that drag also fires the reveal.
 */
function isSelecting(): boolean {
  const selection = typeof window === 'undefined' ? null : window.getSelection()
  return selection !== null && !selection.isCollapsed
}

export function StatusBar({
  added,
  removed,
  diagnostics = null,
  claude = CLAUDE_PLACEHOLDER,
  revealRoots,
  onRevealSegment,
}: StatusBarProps) {
  const readoutRef = useRef<HTMLSpanElement | null>(null)
  const [crumbs, setCrumbs] = useState<Crumbs>(NO_CRUMBS)

  // Returns the unsubscriber directly, and runs once: the subscription fires immediately
  // with whatever the current editor is showing, so a bar that mounts after an editor —
  // StrictMode's second pass, a hot reload — is filled in rather than blank until the next
  // keystroke.
  useEffect(
    () =>
      subscribeStatusReadout((line) => {
        const el = readoutRef.current
        if (el !== null) el.textContent = line.detail
        // Returning the previous object is what keeps a caret move out of React: the state
        // setter bails on an identical value, and these three move only when the user changes
        // file or crosses a member boundary. Without the guard this would re-render the bar on
        // every keystroke — the whole reason `detail` above goes straight into the DOM.
        setCrumbs((prev) => (sameCrumbs(prev, line) ? prev : line))
      }),
    [],
  )

  /*
   * Which crumbs lead somewhere. One call per render of the bar, which happens when the file or
   * the caret's member changes and at no other time — the `Ln 7, Col 48` half deliberately never
   * gets here.
   *
   * The rule is `sidebar/rowPaths.ts`, which is pure and import-free and is compiled and driven
   * by two check scripts. That is not a style preference: the four bugs this project has paid
   * most for were all rules written inside a hook or an event handler, where nothing can compile
   * them. The import goes chrome → sidebar, which is the same direction — and the same
   * justification — as `NO_DIAGNOSTICS_SOURCE` above: a pure, importless module, so the bar stays
   * a render target that pulls in no store and no React tree.
   */
  const targets = crumbTargets(
    crumbs.file,
    crumbs.trail.length,
    crumbs.pathCount,
    revealRoots ?? [],
  )

  return (
    <div className={styles.bar} data-audit="statusBar">
      <div className={styles.left}>
        {/*
          * The live control, not a label. This slot used to be a `<span>` printing a `branch`
          * prop that the app never passed — so it read `⑂ —` on every launch and did nothing
          * when clicked, which is what the user reported.
          *
          * `BranchSelector` fetches its own data and owns its popup, so the bar hands it
          * nothing: a prop would be a second source of truth for a value the selector already
          * has to hold to render the list.
          */}
        <BranchSelector />

        {/*
         * The two counts sit in one slot rather than being separated by the gap between
         * slots, so they read as a single diff stat.
         *
         * It was one template literal with two **non-breaking** spaces in it, because under
         * `white-space: nowrap` a run of ordinary spaces collapses and the pair came out one
         * space wide. With drawn marks the string is gone and so is that hazard: `.stat` is a
         * flex row with its own gaps, and a gap cannot collapse.
         *
         * A dash rather than a zero while nothing supplies them — see the props above.
         */}
        <span
          className={
            added === undefined || removed === undefined
              ? `${styles.diffStat} ${styles.unknownInk}`
              : styles.diffStat
          }
          title={
            added === undefined || removed === undefined
              ? DIFF_PENDING
              : `${added} lines added, ${removed} removed`
          }
        >
          <span className={styles.stat}>
            <Icon name="circle-plus" size={1} />
            {added ?? '—'}
          </span>
          <span className={styles.stat}>
            <Icon name="circle-minus" size={1} />
            {removed ?? '—'}
          </span>
        </span>

        {diagnostics === null ? (
          <span className={styles.unknown} title={DIAGNOSTICS_PENDING}>
            <span className={styles.stat}>
              <Icon name="circle-x" size={1} />—
            </span>
            <span className={styles.stat}>
              <Icon name="triangle-alert" size={1} />—
            </span>
          </span>
        ) : (
          <>
            <span
              className={`${styles.errors} ${styles.stat}`}
              title={`${diagnostics.errors} errors`}
            >
              <Icon name="circle-x" size={1} />
              {diagnostics.errors}
            </span>
            <span
              className={`${styles.warnings} ${styles.stat}`}
              title={`${diagnostics.warnings} warnings`}
            >
              <Icon name="triangle-alert" size={1} />
              {diagnostics.warnings}
            </span>
          </>
        )}
      </div>

      <div className={styles.right}>
        {/*
         * `crates › cide-core › src › lib.rs › Workspace › open_project`. Empty — and gone, along
         * with its gap — when no editor is open at all; clicking into a terminal leaves the last
         * buffer's trail standing, which is what it means. This is the only thing on the bar
         * allowed to give way when the window narrows, and it gives way from the *left*, so the
         * innermost symbol is the last segment to go.
         *
         * # M12: the tail is now symbols, not only path
         *
         * `EditorSurface` appends the caret's `mod › impl › fn` chain to the same array. The
         * tooltip therefore joins with `›` rather than `/`: `src/main.rs/impl Parser/parse` reads
         * as a path to a file that does not exist.
         *
         * # M16: the path half is clickable, and only the part of it that leads somewhere
         *
         * The M12 note here said this component *"deliberately does not know where the path ends
         * and the symbols begin… no crumb is clickable yet"*. It knows now — `pathCount` rides on
         * the readout claim — and that was the whole of what was missing, because without it
         * `parse` would be classified as `…/lib.rs/impl Parser/parse`, come out "inside the
         * project root", and be drawn as a live control that reports a file which does not exist.
         *
         * **An inert crumb is drawn inert, and says nothing when clicked**, because it is not
         * clickable: the user ruled out the "not in this project's file tree" notice by name for
         * this gesture, and a control that looks identical and does nothing is the defect this
         * project has now found nineteen times. So the difference is in the paint —
         * `.crumbLive` is `--dim` against the trail's `--faint`, and underlines under the pointer
         * — and `crumbTargets` is what decides which is which. On a dependency source that means
         * `home › u › .cargo › registry › src › index.crates.io-…` reads as context and
         * `serde-1.0.229 › src › de › mod.rs` reads as live, which is exactly the shape of the
         * report.
         *
         * No tab stops. `BranchSelector` is this bar's one focusable control, and six to ten new
         * focus stops for a path trail would cost more than the gap they close — the keyboard
         * route to the same place already exists and is bound (⌃⇧E, *Select Opened File*).
         * `role="link"` so the affordance is at least announced.
         */}
        <div className={styles.path} data-audit="editorPath" title={crumbs.trail.join(' › ')}>
          {crumbs.trail.map((segment, i) => {
            // No handler means no file tree in this window, and therefore no revealable crumb —
            // not a swallowed click. See `onRevealSegment`.
            const target = onRevealSegment === undefined ? null : (targets[i] ?? null)
            return (
              // Keyed by position as well as text: a path can repeat a segment
              // (`src/cide/src`), and the text alone would collide.
              <span key={`${i}:${segment}`} className={styles.crumb}>
                {i > 0 && <span className={styles.separator}>›</span>}
                {/*
                 * The label carries the affordance, not the crumb: the `›` before it belongs to
                 * neither segment, and an underline that ran through the separator would read as
                 * one long link. It cannot be pulled out into a sibling of the crumbs instead —
                 * `.path` is `direction: rtl` (that is what makes the trail clip from the left)
                 * and only `.crumb` puts it back, so a bare separator out here would be
                 * reordered by the bidi algorithm.
                 */}
                {target === null ? (
                  segment
                ) : (
                  <span
                    className={styles.crumbLive}
                    role="link"
                    onClick={() => {
                      if (isSelecting()) return
                      onRevealSegment?.(target)
                    }}
                  >
                    {segment}
                  </span>
                )}
              </span>
            )
          })}
        </div>

        {/*
         * `Rust · UTF-8 · LF · Ln 7, Col 48`. Written from `editor/statusReadout.ts` and
         * therefore childless in JSX: giving React a child here would let the next render —
         * a branch change, a token tick — overwrite the live text.
         *
         * Inboard of the Claude readout, so the four facts that move as the user types do not
         * push the session slot around: that one is the widest thing on the bar and the one
         * whose position should stay put.
         */}
        <span className={styles.readout} ref={readoutRef} data-audit="editorReadout" />
        <span
          className={`${styles.claude} ${styles.stat}`}
          title={claude === CLAUDE_PLACEHOLDER ? CLAUDE_PENDING : 'Claude session'}
        >
          <Icon name="message-square" size={1} />
          {claude}
        </span>
        {/* The mock's `rust-analyzer` slot sits here and stays out until one is running. */}
      </div>
    </div>
  )
}
