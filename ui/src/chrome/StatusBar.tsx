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
 *   `›` separators can be their own colour and each segment can carry its own bidi
 *   direction inside a box that clips from the left.
 * * `Rust · UTF-8 · LF · Ln 7, Col 48` — written straight into a DOM node this component
 *   owns. It moves on every caret move, which under a held arrow key is 30 times a second,
 *   and no version of re-rendering the bar that often is worth having.
 *
 * Both used to be a 28px row above every buffer. See `statusReadout.ts` for why they moved,
 * and `editor/EditorSurface.tsx` for the row that is no longer there.
 */
import { useEffect, useRef, useState } from 'react'
import { sameTrail, subscribeStatusReadout } from '@/editor/statusReadout'
import { NO_DIAGNOSTICS_SOURCE } from '@/sidebar/ProblemsPanel/model'
import { BranchSelector } from './BranchSelector'
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
  added?: number | undefined
  removed?: number | undefined
  /** `null` means no diagnostics source is running, which is distinct from zero of each. */
  diagnostics?: Diagnostics | null | undefined
  claude?: string | undefined
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

/** Shown until a real readout exists, and the value the pending title keys off. */
const CLAUDE_PLACEHOLDER = 'claude · —'

export function StatusBar({
  added = 0,
  removed = 0,
  diagnostics = null,
  claude = CLAUDE_PLACEHOLDER,
}: StatusBarProps) {
  const readoutRef = useRef<HTMLSpanElement | null>(null)
  const [trail, setTrail] = useState<readonly string[]>([])

  // Returns the unsubscriber directly, and runs once: the subscription fires immediately
  // with whatever the current editor is showing, so a bar that mounts after an editor —
  // StrictMode's second pass, a hot reload — is filled in rather than blank until the next
  // keystroke.
  useEffect(
    () =>
      subscribeStatusReadout((line) => {
        const el = readoutRef.current
        if (el !== null) el.textContent = line.detail
        // Returning the previous array is what keeps a caret move out of React: the state
        // setter bails on an identical value, and the trail only ever moves when the user
        // changes file. Without the guard this would re-render the bar on every keystroke —
        // the whole reason `detail` above goes straight into the DOM.
        setTrail((prev) => (sameTrail(prev, line.trail) ? prev : line.trail))
      }),
    [],
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
         * The mock separates the two counts by two spaces inside one slot rather than by
         * the 16px gap between slots, so they read as a single diff stat. Both are
         * non-breaking: under `white-space: nowrap` a run of ordinary spaces still
         * collapses, and the pair would come out one space wide.
         */}
        <span title={`${added} lines added, ${removed} removed`}>
          {`⊕ ${added}  ⊖ ${removed}`}
        </span>

        {diagnostics === null ? (
          <span className={styles.unknown} title={DIAGNOSTICS_PENDING}>
            <span>✗ —</span>
            <span>⚠ —</span>
          </span>
        ) : (
          <>
            <span className={styles.errors} title={`${diagnostics.errors} errors`}>
              ✗ {diagnostics.errors}
            </span>
            <span className={styles.warnings} title={`${diagnostics.warnings} warnings`}>
              ⚠ {diagnostics.warnings}
            </span>
          </>
        )}
      </div>

      <div className={styles.right}>
        {/*
         * `crates › cide-core › src › lib.rs`. Empty — and gone, along with its gap — when no
         * editor is open at all; clicking into a terminal leaves the last buffer's trail
         * standing, which is what it means. This is the only thing on the bar allowed to give
         * way when the window narrows, and it gives way from the *left*, so the file name is
         * the last segment to go.
         */}
        <div className={styles.path} data-audit="editorPath" title={trail.join('/')}>
          {trail.map((segment, i) => (
            // Keyed by position as well as text: a path can repeat a segment
            // (`src/cide/src`), and the text alone would collide.
            <span key={`${i}:${segment}`} className={styles.crumb}>
              {i > 0 && <span className={styles.separator}>›</span>}
              {segment}
            </span>
          ))}
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
          className={styles.claude}
          title={claude === CLAUDE_PLACEHOLDER ? CLAUDE_PENDING : 'Claude session'}
        >
          ◆ {claude}
        </span>
        {/* The mock's `rust-analyzer` slot sits here and stays out until one is running. */}
      </div>
    </div>
  )
}
