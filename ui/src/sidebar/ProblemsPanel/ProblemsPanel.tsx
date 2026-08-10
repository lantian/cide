/**
 * The ⚑ Problems sidebar panel.
 *
 * ## What this surface is, and what it deliberately is not
 *
 * It is not a language-server client. v1 ships without one — a stated decision, the same one
 * `chrome/StatusBar.tsx` already encodes when it degrades its `✗ 0 ⚠ 2` slot to `✗ — ⚠ —`.
 * Building a diagnostics engine to make this button do something would be answering a
 * different question than the one the user asks by clicking it.
 *
 * So it is a real surface that reports the truth: nothing is analysing this workspace, that
 * is why the list is empty, and here is what would change it. The rail's ⚑ previously
 * rendered *nothing at all*, which is indistinguishable from a broken button.
 *
 * ## Why it is not a placeholder
 *
 * The list, the grouping, the per-file counters and the click-to-open path are all here and
 * all driven by `model.ts`. Hand this component a `ready` snapshot and it renders a working
 * problems list today. That is the difference between a surface that becomes live when a
 * source appears and a card that has to be thrown away and rewritten.
 *
 * ## Wiring
 *
 * Nothing here reaches into the store, matching every other chrome/sidebar surface: the host
 * passes a snapshot, or passes none and gets the v1 truth. See `index.ts` for the App.tsx
 * block.
 */
import { useMemo } from 'react'
import type { ProjectId } from '@/ipc/client'
import {
  NO_SOURCE,
  checked,
  groupByFile,
  headline,
  metaFigure,
  summaryLine,
  type DiagnosticsSnapshot,
  type HeadlineTone,
  type Severity,
} from './model'
import styles from './ProblemsPanel.module.css'

/** The mock has no problems row, so the glyphs follow the status bar's `✗` / `⚠` family. */
const SEVERITY_GLYPH: Record<Severity, string> = {
  error: '✗',
  warning: '⚠',
  info: 'ℹ',
  hint: '·',
}

/*
 * `string | undefined` because Vite types a CSS module as `Record<string, string>` and this
 * project runs `noUncheckedIndexedAccess`. Asserting the class away would be asserting that
 * the stylesheet still defines it, which is exactly what nothing here can promise.
 */
const SEVERITY_CLASS: Record<Severity, string | undefined> = {
  error: styles.error,
  warning: styles.warning,
  info: styles.info,
  hint: styles.hint,
}

export interface ProblemsPanelProps {
  /**
   * The active project, or `null` before one is open.
   *
   * Unused for fetching today — there is nothing to fetch — but taken now because the panel
   * is per-project the moment a source exists, and a host that has already written
   * `<ProblemsPanel project={activeProjectId} />` does not have to be revisited then. It is
   * rendered: an empty workspace says so rather than showing the language-server explainer,
   * because with no project open the missing analyser is not the reason the list is empty.
   */
  project: ProjectId | null
  /**
   * Where diagnostics come from.
   *
   * Omitted is not "unknown" — it is the answer v1 always gives, `NO_SOURCE`. A host that
   * later owns a language server passes a live snapshot instead and this file does not change.
   */
  snapshot?: DiagnosticsSnapshot | undefined
  /**
   * Jump to a diagnostic. Optional because the panel owns no pane and must not pretend it
   * can open one — the same contract `GitPanel`'s `onOpenDiff` has. Without it the rows are
   * rendered as static text rather than as buttons that do nothing when clicked.
   */
  onOpenLocation?: ((path: string, line: number, column: number) => void) | undefined
}

export function ProblemsPanel({
  project,
  // Omitting the prop *is* the v1 answer, not a missing one. See `NO_SOURCE`.
  snapshot = NO_SOURCE,
  onOpenLocation,
}: ProblemsPanelProps) {
  const groups = useMemo(
    () => (snapshot.kind === 'ready' ? groupByFile(snapshot.items) : []),
    [snapshot],
  )
  const head = headline(snapshot)
  const scanned = checked(snapshot)

  return (
    <aside className={styles.panel} data-audit="sidebarProblems" aria-label="Problems">
      <div className={styles.header} data-audit="problemsHeader">
        <span className={styles.headerTitle}>Problems</span>
        {/* `—` until something has looked. See `metaFigure`: a digit here is a claim. */}
        <span className={styles.headerMeta} title={scanned ? undefined : head.detail}>
          {metaFigure(snapshot)}
        </span>
      </div>

      {project === null ? (
        <div className={styles.body}>
          <p className={styles.claim}>No project open</p>
          <p className={styles.detail}>
            Problems are reported per project. Open one from the header’s <b>+</b> button.
          </p>
        </div>
      ) : (
        <div className={styles.body}>
          {/*
            * The claim, always, in every state — including `ready` with rows, where it is the
            * summary line. One consistent place to read "what does this panel currently
            * assert about my workspace" beats a heading that appears only when empty.
            */}
          <p className={`${styles.claim} ${toneClass(head.tone)}`} data-audit="problemsClaim">
            {head.text}
          </p>
          <p className={styles.detail}>{head.detail}</p>

          {/*
            * The explainer only in the state that needs explaining. "Nothing is running" is a
            * surprising answer, and a panel that gives a surprising answer without saying what
            * would change it sends the user to look for a setting that does not exist.
            */}
          {snapshot.kind === 'unavailable' && (
            <div className={styles.note} data-audit="problemsExplainer">
              <p className={styles.noteHead}>Why this is not a clean bill of health</p>
              <p className={styles.detail}>
                An empty problems list normally means “checked, nothing wrong”. Here it means
                nobody checked. The status bar says the same thing with <code>✗ —</code> rather
                than <code>✗ 0</code>.
              </p>
              <p className={styles.noteHead}>What still catches errors today</p>
              <ul className={styles.list}>
                {/* Named concretely so the answer is actionable rather than reassuring. */}
                <li>The terminal — <code>cargo check</code>, <code>tsc --noEmit</code>, your test run.</li>
                <li>Claude, which reads compiler output from the panes it is given.</li>
                <li>Git, for what changed; the ⑂ panel, not this one.</li>
              </ul>
              <p className={styles.noteHead}>What would fill this panel</p>
              <p className={styles.detail}>
                A language server per project, its diagnostics pushed into the workspace and
                mirrored here and in the status bar. Until then this panel and that slot both
                report “unknown”, never “none”.
              </p>
            </div>
          )}

          {groups.length > 0 && (
            <div className={styles.groups} data-audit="problemsGroups">
              {groups.map((group) => (
                <div key={group.path} className={styles.group}>
                  <div className={styles.groupHead} title={group.path}>
                    <span className={styles.groupPath}>{group.path}</span>
                    <span className={styles.groupCount}>{summaryLine(group.counts)}</span>
                  </div>
                  {group.items.map((item, i) => {
                    const where = `${item.line}:${item.column}`
                    const label = `${item.severity} at ${group.path} ${where}: ${item.message}`
                    const body = (
                      <>
                        <span
                          className={`${styles.glyph} ${SEVERITY_CLASS[item.severity] ?? styles.hint}`}
                          aria-hidden="true"
                        >
                          {SEVERITY_GLYPH[item.severity] ?? '·'}
                        </span>
                        <span className={styles.message}>{item.message}</span>
                        {item.code !== undefined && (
                          <span className={styles.code}>{item.code}</span>
                        )}
                        <span className={styles.where}>{where}</span>
                      </>
                    )
                    return onOpenLocation === undefined ? (
                      // Static, not a disabled button: there is nowhere to go, and a row that
                      // looks clickable and is not is the bug this whole task is about.
                      <div key={`${where}-${i}`} className={styles.row} title={label}>
                        {body}
                      </div>
                    ) : (
                      <button
                        key={`${where}-${i}`}
                        type="button"
                        className={`${styles.row} ${styles.rowButton}`}
                        title={label}
                        onClick={() => onOpenLocation(group.path, item.line, item.column)}
                      >
                        {body}
                      </button>
                    )
                  })}
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </aside>
  )
}

function toneClass(tone: HeadlineTone): string | undefined {
  if (tone === 'clean') return styles.claimClean
  if (tone === 'counts') return styles.claimCounts
  return styles.claimUnknown
}
