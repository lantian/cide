/**
 * *Send lines to Claude* — the arithmetic and the wording, with nothing that can send.
 *
 * Import-free on purpose. `check-editor.mjs` compiles the editor's pure modules with `tsc`
 * and runs them under node; anything reaching `@/ipc/client` cannot go through that pipe
 * (`keys/dispatch.ts` is excluded for exactly this reason), and the part of this feature most
 * worth pinning is the part with numbers in it. The sending half is `useSendToClaude.ts`.
 *
 * # The convention, stated once
 *
 * **Everything here is 1-based**, because everything a human reads is: CodeMirror numbers its
 * lines from 1, so does every stack trace, and so does the `Ln 128` readout three lines below
 * this in `EditorSurface`. The wire is 0-based, and the single conversion lives in
 * `cmd::file::claude_send_lines` at the Rust boundary — see `cide_ide_mcp::protocol
 * ::AtMentioned`, which documents `lineStart`/`lineEnd` as 0-based inclusive and *omits* an
 * absent bound rather than sending `null`.
 */

/** An inclusive, 1-based span of lines. `null` anywhere below means "the whole file". */
export interface SendRange {
  readonly lineStart: number
  readonly lineEnd: number
}

/**
 * The range a gesture means, or `null` for the whole file.
 *
 * An empty selection is a *caret*, and a caret is not a range: `@file#L12-L12` because the
 * cursor happened to be resting on line 12 is not what anyone meant by "send this file", and
 * it is the one wrong answer that looks deliberate. So the emptiness of the text decides,
 * not the line numbers — a selection can span zero characters and two lines when it starts at
 * the end of one.
 */
export function rangeOf(text: string, firstLine: number, lastLine: number): SendRange | null {
  if (text.length === 0) return null
  const lineStart = Math.max(1, Math.min(firstLine, lastLine))
  const lineEnd = Math.max(1, Math.max(firstLine, lastLine))
  return { lineStart, lineEnd }
}

/**
 * What the menu item says it will do.
 *
 * Three wordings rather than one with a number in it, because "Send lines 12–12 to Claude" is
 * how a user learns not to trust the label.
 */
export function sendLabel(range: SendRange | null): string {
  if (range === null) return 'Send this file to Claude'
  if (range.lineStart === range.lineEnd) return `Send line ${range.lineStart} to Claude`
  return `Send lines ${range.lineStart}–${range.lineEnd} to Claude`
}

/**
 * `@src/main.rs#L10-20`, for the diagnostic log.
 *
 * **This is our rendering, not the wire.** The notification carries `filePath`, `lineStart`
 * and `lineEnd` as separate fields and the CLI formats the mention itself; this string exists
 * so the log records what the user will see in the prompt in the same spelling they will see
 * it, which is what makes a report of "it did nothing" answerable from a log file.
 */
export function mentionLabel(path: string, range: SendRange | null): string {
  if (range === null) return `@${path}`
  if (range.lineStart === range.lineEnd) return `@${path}#L${range.lineStart}`
  return `@${path}#L${range.lineStart}-${range.lineEnd}`
}
