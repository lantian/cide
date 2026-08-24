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
 *
 * ## What M12 and M18 changed, and what did not
 *
 * The paragraphs above are the record of why this surface was written before there was anything
 * to put in it, and they stay: the *shape* they argue for is what let a real `DiagnosticsSnapshot`
 * be dropped in without a rewrite. What is no longer true is the first sentence — cide has had a
 * language-server client since M12 (`crates/cide-lsp`), and `App.tsx` passes a live snapshot.
 *
 * M18 added the two things that were still missing at the last inch:
 *
 * * **The footer.** `snapshot.sources` has been on the wire since M12 and `adapt.ts` has been
 *   converting it that whole time, and this component rendered none of it — so *"rust-analyzer is
 *   not installed"*, the one sentence that explains an empty list, arrived and was discarded. With
 *   it comes the first caller in the entire app of `diagnostics.restart`, which had been real,
 *   tested and contract-registered with nothing invoking it.
 * * **Staleness.** A diagnostic carries the line it had when it was published, and out-of-editor
 *   writes (an agent's edit, a `git checkout`) move the file underneath it. The row now says so
 *   rather than silently sending the user to whatever is at that line now.
 */
import { useMemo } from 'react'
import type { ProjectId } from '@/ipc/client'
import {
  NO_SOURCE,
  STALE_NOTE,
  checked,
  groupByFile,
  headline,
  isSeverity,
  metaFigure,
  sourceRows,
  summaryLine,
  type DiagnosticsSnapshot,
  type HeadlineTone,
  type Severity,
  type SourceRow,
} from './model'
import { Icon, type IconName } from '@/icons/Icon'

import styles from './ProblemsPanel.module.css'

/**
 * The severity marks, and they are deliberately the status bar's and the editor gutter's.
 *
 * A user reading the margin of a file, the status bar's counters and this panel is looking at
 * one alphabet in three places — which was already the intent when all three were `✗ ⚠ ℹ ·`, and
 * is now literally true: `styles/iconMasks.css` draws the gutter from the same vendored paths.
 */
const SEVERITY_ICON: Record<Severity, IconName> = {
  error: 'circle-x',
  warning: 'triangle-alert',
  info: 'info',
  hint: 'minus',
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

/**
 * The glyph and colour class for a severity that may not be one.
 *
 * Both tables are keyed by `Severity`, and `item.severity` only claims to be one — it comes
 * from another process. A bare `TABLE[item.severity] ?? fallback` does not close that gap:
 * for a prototype key such as `'constructor'` the lookup returns a *function*, `??` passes it
 * through, and React refuses a function as a child while `className` stringifies it into
 * garbage. So membership is checked first, and everything else renders as a hint — dim, with
 * the raw severity still visible in the row's title, which is the honest way to show a row we
 * do not know how to categorise.
 */
function severityLook(severity: Severity): { icon: IconName; className: string | undefined } {
  if (!isSeverity(severity)) return { icon: 'minus', className: styles.hint }
  return { icon: SEVERITY_ICON[severity], className: SEVERITY_CLASS[severity] }
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
  /**
   * How many items the filters removed. (M12)
   *
   * Threaded to [`headline`], and only there — it is what stops an empty list produced by the
   * user's own settings from headlining as *No problems found*. Defaulted so every existing call
   * site, and every fixture, keeps its behaviour exactly.
   */
  hidden?: number | undefined
  /**
   * Re-run every analyser for this project. (M18)
   *
   * The button the user asked for in as many words. Optional for the same reason
   * [`onOpenLocation`] is: the panel owns no IPC client, and without a handler the control is not
   * rendered at all rather than rendered dead — a button that does nothing when pressed is the
   * bug this whole surface keeps being rewritten to avoid.
   */
  onRefresh?: (() => void) | undefined
  /**
   * Restart one analyser. (M18)
   *
   * `id` is the wire's `DiagnosticSourceId` — `rustAnalyzer`, `gopls`. Only the rows
   * `sourceRows` marks `restartable` offer it, because `ProjectDiagnostics::restart` returns
   * immediately for anything that is not a process.
   *
   * Separate from [`onRefresh`] and labelled separately, because they cost two different things:
   * a re-run is seconds, a restart is minutes of re-indexing on a large workspace.
   */
  /** The row's `label` travels with its id, so the notice need not guess a display name. */
  onRestartSource?: ((id: string, label: string) => void) | undefined
}

export function ProblemsPanel({
  project,
  // Omitting the prop *is* the v1 answer, not a missing one. See `NO_SOURCE`.
  snapshot = NO_SOURCE,
  onOpenLocation,
  hidden = 0,
  onRefresh,
  onRestartSource,
}: ProblemsPanelProps) {
  const groups = useMemo(
    () => (snapshot.kind === 'ready' ? groupByFile(snapshot.items) : []),
    [snapshot],
  )
  /*
   * The footer's rows. (M18)
   *
   * `sources` has been on the wire since M12 and `adapt.ts` has been handing it over that whole
   * time, and this component drew none of it — so "rust-analyzer is not installed", the one
   * sentence that explains an empty list, crossed the IPC boundary and was dropped on the floor.
   * Drawn on **every** snapshot kind, not only `ready`: the states that most need explaining are
   * `unavailable` and `scanning`.
   */
  const sources = useMemo(() => sourceRows(snapshot), [snapshot])
  const head = headline(snapshot, hidden)
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
                nobody checked. The status bar says the same thing by showing a dash beside its
                error mark rather than a zero.
              </p>
              <p className={styles.noteHead}>What still catches errors today</p>
              <ul className={styles.list}>
                {/* Named concretely so the answer is actionable rather than reassuring. */}
                <li>The terminal — <code>cargo check</code>, <code>tsc --noEmit</code>, your test run.</li>
                <li>Claude, which reads compiler output from the panes it is given.</li>
                <li>Git, for what changed; the Git panel, not this one.</li>
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
                <div
                  key={group.path}
                  className={`${styles.group} ${group.stale ? styles.groupStale : ''}`}
                  data-audit="problemsGroup"
                  data-stale={group.stale ? 'true' : undefined}
                >
                  <div className={styles.groupHead} title={group.path}>
                    <span className={styles.groupPath}>{group.path}</span>
                    <span className={styles.groupCount}>{summaryLine(group.counts)}</span>
                  </div>
                  {/*
                    * The qualification, and the whole of the M18 fix on this surface.
                    *
                    * Without it a row whose file has moved underneath it is indistinguishable
                    * from a current one — which is precisely the report: the user clicks a hint
                    * Claude has already fixed and lands in a comment, because the row still
                    * carries the line number it was published with. The rows stay clickable; a
                    * jump that may be a few lines off beats a dead row, as long as it says so.
                    */}
                  {group.stale && (
                    <p className={styles.staleNote} data-audit="problemsStale">
                      {STALE_NOTE}
                    </p>
                  )}
                  {group.items.map((item, i) => {
                    const where = `${item.line}:${item.column}`
                    const label =
                      `${item.severity} at ${group.path} ${where}: ${item.message}` +
                      // Appended to the row's own `title` as well as shown once per group,
                      // because the group note scrolls out of view long before a twenty-row file
                      // does and the tooltip is what a user checks when a jump lands oddly.
                      (item.stale === true ? ` — ${STALE_NOTE}` : '')
                    const look = severityLook(item.severity)
                    const staleClass = item.stale === true ? ` ${styles.rowStale}` : ''
                    const body = (
                      <>
                        <span className={`${styles.glyph} ${look.className}`} aria-hidden="true">
                          <Icon name={look.icon} size={1} />
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
                      <div
                        key={`${where}-${i}`}
                        className={`${styles.row}${staleClass}`}
                        data-audit="problemsRow"
                        data-severity={item.severity}
                        data-stale={item.stale === true ? 'true' : undefined}
                        title={label}
                      >
                        {body}
                      </div>
                    ) : (
                      <button
                        key={`${where}-${i}`}
                        type="button"
                        className={`${styles.row} ${styles.rowButton}${staleClass}`}
                        data-audit="problemsRow"
                        data-severity={item.severity}
                        data-stale={item.stale === true ? 'true' : undefined}
                        title={label}
                        onClick={() =>
                          // The *absolute* path when the producer sent one: `group.path` is
                          // workspace-relative for display, and in a multi-root project it is
                          // label-prefixed as well, so it names no file on disk. Falling back to
                          // it keeps every pre-M12 fixture working.
                          onOpenLocation(item.absPath ?? group.path, item.line, item.column)
                        }
                      >
                        {body}
                      </button>
                    )
                  })}
                </div>
              ))}
            </div>
          )}

          {/*
            * The footer: who is analysing, and the two ways to make them do it again. (M18)
            *
            * Rendered whenever there is anything to say — which is *not* only the `ready` state.
            * A source list under an `unavailable` headline is the whole explanation of why the
            * list is empty, and it was being thrown away.
            */}
          {sources.length > 0 && (
            <div className={styles.footer} data-audit="problemsSources">
              <div className={styles.footerHead}>
                <span className={styles.footerTitle}>Analysers</span>
                {/*
                  * Absent, not disabled, when the host passed no handler. A disabled control is a
                  * promise that it would work under some condition the user could reach; there is
                  * no such condition here, and this panel's own rules forbid dressing a dead
                  * thing as a live one.
                  */}
                {onRefresh !== undefined && (
                  <button
                    type="button"
                    className={styles.action}
                    data-audit="problemsRefresh"
                    onClick={onRefresh}
                    title="Ask every running analyser to check this project again. Takes seconds; it does not re-index."
                  >
                    Re-run
                  </button>
                )}
              </div>
              {sources.map((source) => (
                <SourceLine
                  key={source.id}
                  source={source}
                  onRestart={onRestartSource}
                />
              ))}
            </div>
          )}
        </div>
      )}
    </aside>
  )
}

/** The status dot's colour class. Dim for `unavailable` — not running is not broken. */
const STATUS_CLASS: Record<SourceRow['status'], string | undefined> = {
  ready: styles.sourceReady,
  scanning: styles.sourceScanning,
  unavailable: undefined,
}

function SourceLine({
  source,
  onRestart,
}: {
  source: SourceRow
  onRestart?: ((id: string, label: string) => void) | undefined
}) {
  // Same membership-before-lookup discipline as `severityLook`: `status` is a string that came
  // from another process, and a prototype key would hand back a function that `className`
  // stringifies into garbage.
  const dotClass = Object.hasOwn(STATUS_CLASS, source.status)
    ? STATUS_CLASS[source.status]
    : undefined
  return (
    <div className={styles.source} data-audit="problemsSource" data-source={source.id}>
      <span className={`${styles.sourceDot} ${dotClass ?? ''}`} aria-hidden="true" />
      <span className={styles.sourceText}>
        <span className={styles.sourceLabel}>{source.label}</span>
        <p className={styles.sourceDetail}>{source.detail}</p>
      </span>
      {/*
        * Restart, only for the sources that are processes.
        *
        * `diagnostics.restart` has existed, been tested, been registered in the contract and been
        * wrapped in `client.ts` since M12 with **no caller anywhere in the app** — the mechanism
        * this codebase keeps building and never connecting. This is its first one. tree-sitter
        * and Claude get no button because `ProjectDiagnostics::restart` returns immediately for
        * them, and a control that does nothing when pressed is worse than no control.
        */}
      {source.restartable && onRestart !== undefined && (
        <button
          type="button"
          className={styles.action}
          data-audit="problemsRestart"
          onClick={() => onRestart(source.id, source.label)}
          title={`Stop ${source.label} and start it again. Slower than Re-run — it re-indexes the workspace from scratch.`}
        >
          Restart
        </button>
      )}
    </div>
  )
}

function toneClass(tone: HeadlineTone): string | undefined {
  if (tone === 'clean') return styles.claimClean
  if (tone === 'counts') return styles.claimCounts
  return styles.claimUnknown
}
