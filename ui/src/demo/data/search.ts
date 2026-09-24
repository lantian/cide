/**
 * The search scene's results: `backpressure`, case-insensitive, over the cide tree.
 *
 * Most rows are the lines a real `grep -ri backpressure` over this repository finds — the PTY
 * crate, the docs, the ADR — and the rest are the files the demo branch adds (`coalesce.rs`,
 * `tests/backpressure.rs`), so the list agrees with the git panel and the Claude panes.
 */
import type { SearchFrame, SearchHit, SearchQuery } from '../../ipc/generated'
import { HOME } from '../world'
import { SESSION_CLEAN } from './editor'

export const PATTERN = 'backpressure'

/** `session.rs`'s rows read off the text the editor beside the list shows, so the numbers agree. */
function sessionLines(): Array<[string, number, string]> {
  return SESSION_CLEAN.split('\n').flatMap((text, i): Array<[string, number, string]> =>
    text.toLowerCase().includes(PATTERN) ? [['crates/cide-pty/src/session.rs', i + 1, text]] : [])
}

const LINES: Array<[string, number, string]> = [
  ['crates/cide-pty/src/coalesce.rs', 3, '//! Backpressure is structural rather than negotiated: when the bounded channel fills, the'],
  ['crates/cide-pty/src/coalesce.rs', 41, '/// Depth of the reader→coalescer channel. Small on purpose: this is the backpressure.'],
  ['crates/cide-pty/src/coalesce.rs', 118, '            // A blocking send here is the whole backpressure design: it stalls'],
  ['crates/cide-pty/src/coalesce.rs', 176, '    /// Frames held past the ack window, so backpressure shows up in the stats.'],
  ...sessionLines(),
  ['crates/cide-pty/src/lib.rs', 20, '//! Backpressure is structural rather than negotiated: when the bounded channel fills, the'],
  ['crates/cide-pty/src/lib.rs', 196, '    #[error("the read queue is full ({depth} frames); the child is under backpressure")]'],
  ['crates/cide-pty/src/lib.rs', 197, '    Backpressure { depth: usize },'],
  ['crates/cide-pty/src/lib.rs', 635, '/// have meant a second terminal stack with a second answer to backpressure, reattach and'],
  ['crates/cide-pty/tests/backpressure.rs', 1, '//! A child that writes faster than the webview acks must block in `write()`, not grow'],
  ['crates/cide-pty/tests/backpressure.rs', 24, 'fn a_flooding_child_is_held_by_backpressure() {'],
  ['crates/cide-pty/tests/backpressure.rs', 58, 'fn backpressure_releases_once_the_sink_acks() {'],
  ['crates/cide-docker/src/session.rs', 8, "//! sink list that makes detach-into-a-window free, and `CreditPolicy`'s per-sink backpressure with"],
  ['crates/cide-docker/src/stream.rs', 13, '//! queue, because that is what makes backpressure structural here as well — a full channel stops'],
  ['crates/cide-remote/src/host.rs', 87, "    /// into the child it is watching, because `cide-pty`'s backpressure is structural — a full"],
  ['crates/cide-lsp/src/server.rs', 131, '/// backpressure worth propagating. Full means drop, with a line, rather than block the UI thread'],
  ['docs/architecture.md', 21, '  cide-pty/       PTY sessions: spawn, coalescing, backpressure, vt100 mirror.'],
  ['docs/architecture.md', 129, 'thread → bounded channel (cap 16) → coalescer → vt100 mirror + sinks. Backpressure is'],
  ['docs/adr/0003-xterm-owns-vt.md', 53, '- **Real OS backpressure.** Blocking reader thread → bounded channel (16) → coalescer. When'],
  ['docs/journal.md', 11298, '  cide-pty/        PTY sessions: spawn, coalescing, backpressure, vt100 mirror.'],
  ['docs/journal.md', 11825, "`cide-pty`'s backpressure is **structural and deliberate**: a full sink channel blocks the"],
]

const bytes = (s: string) => new TextEncoder().encode(s).length

/** Byte offsets, as `SearchHit` states them — the lines with an arrow in them are not ASCII. */
export const HITS: SearchHit[] = LINES.map(([rel, line, text]) => {
  const at = text.toLowerCase().indexOf(PATTERN)
  const start = bytes(text.slice(0, at))
  return { path: `${HOME}/${rel}`, rel, line, text, start, end: start + bytes(text.slice(at, at + PATTERN.length)) }
})

/** One finished frame: every hit on the first poll, and nothing after the offset it reports. */
export function frame(query: SearchQuery, offset: number): SearchFrame {
  const files = new Set(HITS.map((h) => h.rel)).size
  return {
    query,
    hits: offset === 0 ? HITS : [],
    offset,
    total: HITS.length,
    files,
    scanned: 1843,
    running: false,
    truncated: false,
    error: null,
  }
}
