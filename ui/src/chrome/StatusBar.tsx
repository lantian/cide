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
 * The editor readout is the one thing that does not arrive as a prop, and it is deliberately
 * not a store subscription either: `editor/statusReadout.ts` pushes
 * `Markdown · UTF-8 · LF · Ln 7, Col 48` straight into the span below, because a caret
 * readout in React state would re-render this bar — and, through `App`, the pane grid under
 * it — thirty times a second under a held arrow key. Nothing else here re-renders for it,
 * and the bar still renders correctly from props alone with no editor open.
 */
import { useEffect, useRef } from 'react'
import { subscribeStatusReadout } from '@/editor/statusReadout'
import { NO_DIAGNOSTICS_SOURCE } from '@/sidebar/ProblemsPanel/model'
import styles from './StatusBar.module.css'

export interface Diagnostics {
  errors: number
  warnings: number
}

export interface StatusBarProps {
  branch?: string | undefined
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
  branch = '—',
  added = 0,
  removed = 0,
  diagnostics = null,
  claude = CLAUDE_PLACEHOLDER,
}: StatusBarProps) {
  const readoutRef = useRef<HTMLSpanElement | null>(null)

  // Returns the unsubscriber directly, and runs once: the subscription fires immediately
  // with whatever the current editor is showing, so a bar that mounts after an editor —
  // StrictMode's second pass, a hot reload — is filled in rather than blank until the next
  // keystroke.
  useEffect(
    () =>
      subscribeStatusReadout((text) => {
        const el = readoutRef.current
        if (el !== null) el.textContent = text
      }),
    [],
  )

  return (
    <div className={styles.bar} data-audit="statusBar">
      <div className={styles.left}>
        <span className={styles.branch} title={`Branch: ${branch}`}>
          ⑂ {branch}
        </span>

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
         * The open file: `Markdown · UTF-8 · LF · Ln 7, Col 48`, and empty when no editor is
         * open at all — clicking into a terminal leaves the last buffer's position standing,
         * which is what it means. Written from `editor/statusReadout.ts` and therefore
         * childless in JSX: giving React a child here would let the next render — a branch
         * change, a token tick — overwrite the live text.
         *
         * First in the group, so the four facts that change as the user moves sit inboard of
         * the Claude readout rather than pushing it around: the session slot is the widest
         * thing on the bar and the one whose position should not move.
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
