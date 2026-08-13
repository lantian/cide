/**
 * What the Settings screen says about the last `claude` to complete the IDE handshake here.
 *
 * # Why this is a module and not three lines of JSX
 *
 * The record is one small object and the temptation is to render it inline. But the sentence
 * it produces has real rules in it — five states, and two of them look identical from a
 * distance while meaning opposite things — and the last time a rule in this codebase lived
 * inside a component it shipped a bug that every check script missed, because there is no
 * check script that can compile a React hook.
 *
 * So the decision is here, import-free, and `ui/scripts/check-handshake.mjs` compiles this
 * file on its own and runs every state. `CliHandshakeRow` in `sections.tsx` renders the
 * answer and contains no rule at all.
 *
 * # The five states, and what separates them
 *
 * 1. **never** — nothing has ever handshook on this machine. A fresh install, or an IDE
 *    integration that has never worked. Not an error: the user may simply not have opened a
 *    Claude pane yet.
 * 2. **ok** — a version handshook, it is inside this build's range, and the record was written
 *    against this build's range too. Nothing to explain.
 * 3. **staleBuild** — the record's range is not this build's. The observation was made by a
 *    different cide, so it says nothing about whether *this* one works. This is the state that
 *    exists only because the record stores the range it was measured against; without that
 *    field it would be indistinguishable from `ok`, and a user upgrading cide would be shown a
 *    reassuring date about a build they are no longer running.
 * 4. **outsideRange** — a version handshook here and it is not one this build has checked.
 *    Which is not the same as "broken": it *did* connect. The honest sentence says both.
 * 5. **unnamed** — something completed the handshake and would not say what it was. Rare, and
 *    kept distinct from `never` because "the integration works and we cannot name the build"
 *    and "nothing has ever connected" have nothing in common.
 *
 * The one thing this module deliberately does **not** do is decide whether the range contains
 * the version. That comparison lives in `cide_ide_mcp::protocol::support_of` beside the
 * constant it compares against, and a second copy of the parse-and-compare in TypeScript would
 * be exactly the drift the whole item is about. The verdict arrives already computed, as
 * `ClaudeCliSupport.warning`.
 */

/** Mirrors `cide_core::handshake::Handshake`. Restated so this module imports nothing. */
export interface HandshakeRecord {
  /** `clientInfo.version`, or null when the CLI named none. */
  version: string | null
  /** Unix milliseconds. */
  atUnixMs: number
  /** The verified range as it stood when this was recorded. */
  verifiedRange: string
}

/** The fields of `ClaudeCliSupport` this module reads. */
export interface SupportValues {
  version: string | null
  verifiedRange: string
  warning: string | null
  handshake: HandshakeRecord | null
}

export type HandshakeState = 'never' | 'ok' | 'staleBuild' | 'outsideRange' | 'unnamed'

export interface HandshakeNote {
  state: HandshakeState
  /** The one-line readout. Always present; this row is never blank. */
  headline: string
  /**
   * A second sentence, when the state needs one. `null` for `ok` — a screen that explains
   * itself on every launch is one nobody reads on the launch it matters, which is the same
   * argument `ClaudeCliSupport.warning` is built on.
   */
  detail: string | null
}

/**
 * The record, read against this build.
 *
 * `now` is a parameter rather than a `Date.now()` call so every case below is drivable from a
 * check script without freezing time. The component passes the real clock.
 */
export function handshakeNote(support: SupportValues, now: number): HandshakeNote {
  const record = support.handshake
  if (record === null) {
    return {
      state: 'never',
      headline: 'No claude has completed the IDE handshake on this machine yet.',
      detail:
        'Open a Claude pane. Inline diffs, @-mentions and the editor selection all travel over that connection, and this line is the only evidence that it works here.',
    }
  }

  const when = relativeTime(record.atUnixMs, now)

  if (record.version === null) {
    return {
      state: 'unnamed',
      headline: `A claude completed the IDE handshake ${when}, without naming its version.`,
      detail:
        'The integration works here. The CLI simply sent no version in its opening frame, so there is nothing to compare against the range this build was checked with.',
    }
  }

  // Checked before the range comparison, deliberately. A record written by a different build
  // is not evidence about this one, so saying "2.1.227 is outside 2.1.224–2.1.231" about it
  // would be comparing a stale observation against a range it was never measured against.
  if (record.verifiedRange !== support.verifiedRange) {
    return {
      state: 'staleBuild',
      headline: `claude ${record.version} completed the IDE handshake ${when}, under a different build of cide.`,
      detail: `That run was recorded against ${record.verifiedRange}; this build was checked against ${support.verifiedRange}. Open a Claude pane to record one for this build.`,
    }
  }

  // `warning` is Rust's verdict on the version now on PATH, not on the recorded one — but the
  // two agree whenever the recorded version is the one running, which is the case this reads
  // for. When they disagree the version strings differ too, and that is what is said.
  if (support.version !== null && support.version !== record.version) {
    return {
      state: 'outsideRange',
      headline: `claude ${record.version} completed the IDE handshake ${when}; ${support.version} is on PATH now.`,
      detail:
        'The CLI updates itself. The version that last proved the integration works is not the one that would run next, so this line is evidence about the older one.',
    }
  }

  if (support.warning !== null) {
    return {
      state: 'outsideRange',
      headline: `claude ${record.version} completed the IDE handshake ${when} — and it is not a version this build has checked.`,
      detail: `It connected, so the integration is working here. But ${support.verifiedRange} is what anybody verified, and a protocol change would show up as diffs quietly not appearing rather than as an error.`,
    }
  }

  return {
    state: 'ok',
    headline: `claude ${record.version} completed the IDE handshake ${when}.`,
    detail: null,
  }
}

/**
 * The whole readout as one string.
 *
 * The join lives here rather than as a ternary in JSX for the same reason everything else in
 * this module does: it is one line today and it is the line somebody adds a third clause to.
 */
export function sentence(note: HandshakeNote): string {
  return note.detail === null ? note.headline : `${note.headline} ${note.detail}`
}

/**
 * The short value for the row's control column: the version that handshook, or why there
 * isn't one. Never blank — an empty control column reads as a control that failed to render.
 */
export function badge(support: SupportValues): string {
  const record = support.handshake
  if (record === null) return 'never'
  return record.version ?? 'unnamed'
}

/**
 * "3 hours ago", and the units stop at days.
 *
 * Written here rather than reached for through `Intl.RelativeTimeFormat` because the whole
 * module is import-free and standalone-compilable, and because the rounding wanted here is
 * blunter than that API's: this is a claim about evidence, and "2 days ago" is the right
 * precision for one. Weeks and months are not units this needs — a record that old is
 * already saying "nothing has connected in a long time", which reads better as a day count.
 *
 * A record from the future — a clock that moved, or one machine's state directory on
 * another's — says so rather than rendering a negative interval.
 */
export function relativeTime(atUnixMs: number, now: number): string {
  const seconds = Math.round((now - atUnixMs) / 1000)
  if (seconds < 0) return 'at a time later than now, by this machine’s clock'
  if (seconds < 45) return 'just now'
  const minutes = Math.round(seconds / 60)
  if (minutes < 60) return `${plural(Math.max(minutes, 1), 'minute')} ago`
  const hours = Math.round(minutes / 60)
  if (hours < 24) return `${plural(hours, 'hour')} ago`
  return `${plural(Math.round(hours / 24), 'day')} ago`
}

function plural(count: number, noun: string): string {
  return `${count} ${noun}${count === 1 ? '' : 's'}`
}
