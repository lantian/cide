/**
 * A tool line of an agent run's rendered stream, and the handle at its end. (M42)
 *
 * `cide_agents::harness::opencode::render_event` draws every tool call as one line —
 *
 * ```text
 * ● bash  cargo test --workspace  1.2s #7
 * ✗ bash  cat ~/.cargo/config.toml  #9
 * ∴ thought  4.1s #8
 * ```
 *
 * — and puts the call's input and whole output behind the `#7`: a handle into the same
 * per-session ring a shell pane's structured log lines live in, answered by
 * `session_log_detail`. This module reads the handle back off the buffer text so a click can
 * resolve it (`runLinkProvider.ts`).
 *
 * # Why a visible token and not an OSC 8 hyperlink
 *
 * The JSON-log road (`logLink.ts`) rides an OSC 8 link over the timestamp, and it works for
 * lines the pane received *live*. `vt100` drops OSC 8 from its replays, and a run is mostly
 * read after the fact — opened from History, re-attached after a restart — so a link that
 * only survives live delivery would be missing exactly where it is wanted. A few dim
 * characters of plain text survive every replay.
 *
 * # Import-free, on purpose
 *
 * `ui/scripts/check-json-log.mjs` compiles this file standalone with a bare `tsc` and drives
 * the parser over the line shapes the renderer emits. Keep it that way.
 */

export interface RunLine {
  /** The tool's display name, as the line spells it. */
  readonly tool: string
  /** The ring handle the token names. */
  readonly handle: number
  /** 0-based, exclusive: the end of the `● tool` prefix. */
  readonly toolEnd: number
  /** 0-based: the `#` of the handle token. */
  readonly tokenStart: number
  /** 0-based, exclusive: the end of the token — the trimmed line's length. */
  readonly end: number
}

/**
 * The glyph, one space, the tool name, then anything (the title, the duration, an exit code),
 * then one space and the token at the very end. The two spaces the renderer puts after the
 * tool name are not required, so a tool line with no title still parses.
 *
 * `∴` joined `●` and `✗` in M62, when a block of the model's reasoning became one collapsed row
 * — `∴ thought  4.1s #8` — with the whole of the thinking behind the handle. It parses as a
 * tool line whose `tool` is the word `thought`, which is what makes the prefix and the token
 * clickable with no second grammar. All three glyphs are one UTF-16 unit and one cell, so
 * `toolEnd` and `spanOf`'s column arithmetic are unchanged.
 *
 * The cost of widening the class: a line of the model's *own words* that begins `∴ ` and ends
 * ` #12` is now read as a run line and points at an unrelated ring entry. That has always been
 * true of `●` and `✗`; the glyphs are rare enough at the start of a sentence to be worth it,
 * and the alternative — a marker the model could not accidentally type — is an escape sequence
 * `vt100` drops from every replay, which is the whole reason this token is plain text.
 */
const RUN_LINE = /^[●✗∴] (\S+)(?: .*)? #(\d+)$/u

/** The line's parts, or `null` for a line that is not a tool line with a handle. */
export function parseRunLine(text: string): RunLine | null {
  const line = text.replace(/\s+$/u, '')
  const match = RUN_LINE.exec(line)
  if (match === null) return null
  const tool = match[1] ?? ''
  const digits = match[2] ?? ''
  const handle = Number(digits)
  if (tool === '' || digits === '' || !Number.isSafeInteger(handle)) return null
  return {
    tool,
    handle,
    toolEnd: 2 + tool.length,
    tokenStart: line.length - digits.length - 1,
    end: line.length,
  }
}
