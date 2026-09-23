/**
 * What a kept log line *is*, and the clock the card prints beside it.
 *
 * `LogDetailCard.tsx` draws one raw line three ways — a tool call as sections, the model's own
 * words as prose, anything else as pretty JSON — and which of the three is a rule over the
 * line's JSON shape, read off the same fields the Rust renderers read
 * (`cide_agents::harness::opencode::render_tool`, `cide_agents::harness::codex::render_item`).
 * That rule lived inside the component, where no check could compile it, and the two shapes it
 * reads are two harnesses' wire formats that change without telling this file.
 *
 * # Import-free, on purpose
 *
 * `ui/scripts/check-json-log.mjs` compiles this module standalone with a bare `tsc` and drives
 * `viewOf` over the event shapes each harness emits, and `stamp` over a fixed instant. Keep it
 * free of imports — `terminal/runLinks.ts`'s rule, for the same check.
 *
 * # The clock
 *
 * A tool call is drawn with a date and a 24-hour time, because a person reading a run back
 * asks *when* before anything else and a run is mostly read after the fact. Where the call comes
 * from is a per-harness question: opencode records `state.time.start` on the call itself, codex
 * records nothing, so the card falls back to the moment the line reached cide
 * (`LogLineDetail::recordedUnixMs`). [`stamp`] is hand-rolled rather than `toLocaleString` for
 * `overlays/format.ts`'s reason: a desktop app whose clock changes shape with `LC_ALL` is
 * guessing, and the 24-hour form was the one asked for.
 */

/** A tool call, as the opencode stream reports one once it has completed. */
export interface ToolView {
  readonly kind: 'tool'
  readonly tool: string
  readonly title: string
  /** `completed`, `error`, or whatever the CLI said. */
  readonly status: string
  readonly duration: string | null
  /**
   * When the call started, by the harness's own clock, in milliseconds since the Unix epoch —
   * or `null` for a harness that records none, in which case the card prints the moment the
   * line reached cide instead.
   */
  readonly startedAt: number | null
  /** A shell command's exit code, when the CLI recorded one. */
  readonly exit: number | null
  /** The one string input a call usually has — a command, a path — or `null` for a document. */
  readonly command: string | null
  /** The whole input as pretty JSON, for the document case and the card's copy action. */
  readonly input: string
  readonly output: string
  readonly error: string | null
}

/** The model's own words. */
export interface TextView {
  readonly kind: 'text'
  readonly text: string
}

/**
 * A block of the model's thinking, whole. (M62)
 *
 * Its own kind rather than a `TextView` with a heading, for two reasons. "The model said" and
 * "the model thought" are different claims about the same card and must not share a string. And
 * the pretty-JSON fallback is not an option here the way it is for an unrecognised shape: since
 * M62 the run's pane draws *none* of the reasoning — just `∴ thought  4.1s #8` — so this card is
 * the only copy, and rendering a page of prose as one escaped JSON string literal is exactly the
 * lossiness this module's header exists to answer.
 *
 * The text is carried **unclipped and with its newlines**: the row shows none of it, so a stray
 * `clip` or one-line collapse here would be invisible until somebody opened one.
 */
export interface ReasoningView {
  readonly kind: 'reasoning'
  readonly text: string
}

export type View = ToolView | TextView | ReasoningView

/**
 * `cide_cide_task_get` reads as `cide_task_get` — the renderer's `display_tool`, restated here
 * because the raw event still carries the doubled server prefix.
 */
export function displayTool(tool: string): string {
  return tool.startsWith('cide_cide_') ? `cide_${tool.slice('cide_cide_'.length)}` : tool
}

/** `8ms`, `1.2s`, `2m05s` — the renderer's three shapes, so the card agrees with the line. */
export function duration(ms: number): string {
  if (ms < 1_000) return `${ms}ms`
  if (ms < 60_000) return `${(ms / 1_000).toFixed(1)}s`
  const m = Math.floor(ms / 60_000)
  const s = Math.floor((ms % 60_000) / 1_000)
  return `${m}m${String(s).padStart(2, '0')}s`
}

/**
 * An instant as the wall clock read it: `2026-09-18 14:03:27`, local time, 24-hour, always
 * nineteen characters.
 *
 * Local time on purpose — it is read next to the reader's own clock, and next to the task
 * card's `clock()` in `TasksPanel/model.ts`, which prints the same hours. With the date, unlike
 * that one, because a run is opened from History days later and `14:03:27` alone does not say
 * which day. Total: a value that is not a finite number of milliseconds renders as a visibly
 * broken placeholder rather than `NaN-NaN-NaN NaN:NaN:NaN`.
 */
export function stamp(atMs: number): string {
  if (!Number.isFinite(atMs)) return '----.--.-- --:--:--'
  const date = new Date(atMs)
  const pad = (n: number) => String(n).padStart(2, '0')
  const day = `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`
  return `${day} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null
}

function asString(value: unknown): string | null {
  return typeof value === 'string' ? value : null
}

/**
 * What the raw line is, when it is one of the shapes the card draws specially.
 *
 * Read off the same fields `cide_agents::harness::opencode::render_tool` reads, and nothing
 * else: `part.tool`, `part.state.{status,title,input,output,error,time,metadata}`. A line that
 * is not JSON, or JSON of another shape, answers `null` and stays the JSON view.
 */
export function viewOf(raw: string): View | null {
  let event: unknown
  try {
    event = JSON.parse(raw)
  } catch {
    return null
  }
  const record = asRecord(event)
  if (record === null) return null
  // A codex event carries its call under `item`, not `part` (M44).
  if (record.type === 'item.completed') return codexItemView(asRecord(record.item))
  const part = asRecord(record.part)
  if (part === null) return null
  switch (record.type) {
    case 'text': {
      const text = asString(part.text)
      return text === null ? null : { kind: 'text', text }
    }
    case 'reasoning': {
      const text = asString(part.text)
      return text === null ? null : { kind: 'reasoning', text }
    }
    case 'tool_use': {
      const state = asRecord(part.state) ?? {}
      const input = asRecord(state.input) ?? {}
      const strings = Object.values(input).filter((v): v is string => typeof v === 'string')
      // One string input is the call's whole argument (a command, a path); a document is a
      // document. `Object.keys` rather than `strings` for the test, so `{ "command": "ls",
      // "timeout": 5 }` still reads as a command.
      const command = Object.keys(input).length === 1 && strings.length === 1 ? strings[0]! : null
      const time = asRecord(state.time)
      const start = typeof time?.start === 'number' ? time.start : null
      const end = typeof time?.end === 'number' ? time.end : null
      const metadata = asRecord(state.metadata)
      const exit = typeof metadata?.exit === 'number' ? metadata.exit : null
      return {
        kind: 'tool',
        tool: displayTool(asString(part.tool) ?? 'tool'),
        title: (asString(state.title) ?? '').trim() || strings.join(' '),
        status: asString(state.status) ?? '',
        duration: start !== null && end !== null ? duration(Math.max(0, end - start)) : null,
        startedAt: start,
        exit,
        command,
        input: JSON.stringify(input, null, 2),
        output: asString(state.output) ?? '',
        error: asString(state.error),
      }
    }
    default:
      return null
  }
}

/**
 * A codex `item.completed` as the card draws it (M44). Codex's items carry the whole call —
 * the command and its aggregated output, an MCP call's arguments and result, a file change's
 * list — under names of their own, so this is the opencode `tool_use` reading restated over that
 * shape. `cide_agents::harness::codex::render_item` is the one-line twin. An item the card has
 * no view for falls to the pretty JSON, which is still the whole event.
 *
 * No item carries a clock — not a start, not an end, not a duration — which is why every arm
 * answers `startedAt: null` and the card prints the line's arrival instead.
 */
function codexItemView(item: Record<string, unknown> | null): View | null {
  if (item === null) return null
  const status = asString(item.status) ?? ''
  switch (item.type) {
    case 'agent_message': {
      const text = asString(item.text)
      return text === null ? null : { kind: 'text', text }
    }
    case 'reasoning': {
      const text = asString(item.text)
      return text === null ? null : { kind: 'reasoning', text }
    }
    case 'command_execution': {
      const command = asString(item.command)
      const output = asString(item.aggregated_output) ?? ''
      const trimmed = output
        .split('\n')
        .map((line) => line.trim())
        .filter((line) => line !== '')
      const last = trimmed.length === 0 ? null : trimmed[trimmed.length - 1]!
      return {
        kind: 'tool',
        tool: 'shell',
        title: command ?? '',
        status,
        duration: null,
        startedAt: null,
        exit: typeof item.exit_code === 'number' ? item.exit_code : null,
        command,
        input: JSON.stringify({ command }, null, 2),
        output,
        error:
          status === 'failed'
            ? (last ?? 'failed')
            : status === 'declined'
              ? 'declined by the sandbox or the approval policy'
              : null,
      }
    }
    case 'mcp_tool_call': {
      const server = asString(item.server) ?? ''
      const tool = asString(item.tool) ?? 'tool'
      const args = asRecord(item.arguments) ?? {}
      const strings = Object.values(args).filter((v): v is string => typeof v === 'string')
      const command = Object.keys(args).length === 1 && strings.length === 1 ? strings[0]! : null
      const error = asRecord(item.error)
      return {
        kind: 'tool',
        // cide's own server by the tool's name, as the renderer spells it; another server's
        // with the server in front.
        tool: server === '' || server === 'cide' ? tool : `${server}:${tool}`,
        title: strings.join(' '),
        status,
        duration: null,
        startedAt: null,
        exit: null,
        command,
        input: JSON.stringify(args, null, 2),
        output: item.result === undefined ? '' : JSON.stringify(item.result, null, 2),
        error: asString(error?.message) ?? (status === 'failed' ? 'failed' : null),
      }
    }
    case 'file_change': {
      const changes = Array.isArray(item.changes) ? item.changes : []
      const lines = changes.map((change) => {
        const record = asRecord(change)
        const kind = asString(record?.kind) ?? 'update'
        const glyph = kind === 'add' ? '+' : kind === 'delete' ? '-' : '~'
        return `${glyph}${asString(record?.path) ?? ''}`
      })
      return {
        kind: 'tool',
        tool: 'edit',
        title: lines.join(' '),
        status,
        duration: null,
        startedAt: null,
        exit: null,
        command: null,
        input: JSON.stringify(changes, null, 2),
        output: '',
        error: status === 'failed' ? 'failed' : null,
      }
    }
    default:
      return null
  }
}

// ==========================================================================================
// Who ran it, on what, and how full the context is by now. (M80)
// ==========================================================================================

/**
 * One harness's token figures for its last completed step, as numbers.
 *
 * `cide_ipc::TokenUsage` on the wire, whose `u64`s arrive as `bigint` — the card converts them
 * the way it already converts `recordedUnixMs`, and this module stays import-free so
 * `check:json-log` can drive it. The normalisation is Rust's and is stated there at length: the
 * cached half of a prompt is counted in [`cacheRead`] and never inside [`input`], whichever CLI
 * reported it.
 */
export interface TokenSpend {
  readonly input: number
  readonly output: number
  readonly reasoning: number
  readonly cacheRead: number
  readonly cacheWrite: number
}

/**
 * `5725` → `5,725`.
 *
 * Hand-rolled rather than `toLocaleString` for `stamp`'s reason one function up, which is
 * `overlays/format.ts`': a desktop application whose numbers change shape with `LC_ALL` is
 * guessing, and a figure somebody is about to compare against another figure on the same screen
 * must not be grouped one way here and another way there.
 *
 * Grouped and never abbreviated. The pane's own row says `5.7k tok`, which is right for a line
 * in a stream and wrong here: this card is where somebody came *because* they wanted the number.
 */
export function groupDigits(n: number): string {
  if (!Number.isFinite(n)) return '—'
  const digits = Math.round(Math.abs(n)).toString()
  let grouped = ''
  for (let i = 0; i < digits.length; i += 1) {
    // Counted from the right, which is where the grouping is defined from.
    if (i > 0 && (digits.length - i) % 3 === 0) grouped += ','
    grouped += digits[i]
  }
  return n < 0 ? `-${grouped}` : grouped
}

/** Roughly what was in the window when the step ended — `cide_ipc::TokenUsage::context`'s twin. */
export function contextTokens(spend: TokenSpend): number {
  return spend.input + spend.cacheRead + spend.output + spend.reasoning
}

/**
 * The headline figure: `6,112 tokens`, or `6,112 of 32,768 tokens · 19%` where this machine's
 * configuration states a window.
 *
 * The percentage appears **only** with a stated limit, and `LogRunInfo::context_limit` is
 * `None` for every model whose window cide does not hold a number for — a percentage of a
 * guessed window is the one shape here worse than no percentage at all, because it reads as a
 * measurement. Rounded to whole percent: a tenth of a percent of a context window is precision
 * about an estimate.
 */
export function contextLine(spend: TokenSpend, limit: number | null): string {
  const used = contextTokens(spend)
  if (limit === null || limit <= 0) return `${groupDigits(used)} tokens`
  const percent = Math.round((used / limit) * 100)
  return `${groupDigits(used)} of ${groupDigits(limit)} tokens · ${percent}%`
}

/**
 * What that figure is made of: `4,000 prompt · 1,000 cached · 300 written · 200 thinking`.
 *
 * Each part is drawn only when it is non-zero, so codex — which reports no reasoning figure and
 * no cache writes — shows two parts rather than four zeroes claiming to be measurements. A spend
 * of nothing at all still answers a sentence, because a step that finished having spent nothing
 * measurable is a real and different state from a run that has never reported one.
 *
 * `cacheWrite` is named but is deliberately *not* part of [`contextLine`]: those tokens went
 * into the provider's cache, not into this conversation's window.
 */
export function spendLine(spend: TokenSpend): string {
  const parts: string[] = []
  if (spend.input > 0) parts.push(`${groupDigits(spend.input)} prompt`)
  if (spend.cacheRead > 0) parts.push(`${groupDigits(spend.cacheRead)} cached`)
  if (spend.output > 0) parts.push(`${groupDigits(spend.output)} written`)
  if (spend.reasoning > 0) parts.push(`${groupDigits(spend.reasoning)} thinking`)
  if (spend.cacheWrite > 0) parts.push(`${groupDigits(spend.cacheWrite)} cache write`)
  return parts.length === 0 ? 'nothing measurable' : parts.join(' · ')
}
