/**
 * Live model, token and cost figures, from the Claude Code statusline.
 *
 * # Why the statusline and not something better
 *
 * There isn't something better. The statusline is the only supported source of token usage
 * as it accrues — nothing else the CLI exposes reports it during a turn. So the status bar's
 * `claude · opus 5 · 128.4k / 1M` is downstream of a hook that runs on the CLI's own timer,
 * which is also why the figures can be a second or two stale and should never be presented
 * as authoritative for anything a user would act on financially.
 *
 * # The payload is the CLI's, and is passed through whole
 *
 * The shape below was captured from a real 2.1.226 session rather than assumed. It is not
 * versioned, so every field is optional here: a release that renames one should cost a
 * placeholder in the status bar, never a crash or a bar that renders a confident wrong
 * number. Everything is `?? undefined` for that reason.
 */

/** The statusline payload, as far as we read it. Fields are optional by design. */
export interface StatusPayload {
  session_id?: string
  version?: string
  model?: { display_name?: string; id?: string }
  context_window?: {
    context_window_size?: number
    current_usage?: number | null
    total_input_tokens?: number
    total_output_tokens?: number
    used_percentage?: number | null
  }
  cost?: { total_cost_usd?: number }
}

/** Tokens counted against the context window, or `undefined` when the CLI reports none. */
export function usedTokens(p: StatusPayload): number | undefined {
  const cw = p.context_window
  if (!cw) return undefined
  // `current_usage` is the CLI's own figure and wins when present. It is null on a fresh
  // session, which is a real state — zero tokens used — and distinct from "not reported".
  if (typeof cw.current_usage === 'number') return cw.current_usage
  const input = cw.total_input_tokens
  const output = cw.total_output_tokens
  if (typeof input !== 'number' && typeof output !== 'number') return undefined
  return (input ?? 0) + (output ?? 0)
}

/**
 * Compact token counts the way the mock writes them: `128.4k`, `1M`, `840`.
 *
 * Deliberately not `Intl.NumberFormat`'s compact notation, which renders 1000000 as `1M` but
 * 128400 as `128K` — a capital K the design does not use, and a rounding that loses the
 * tenth the mock shows.
 */
export function compact(n: number): string {
  if (n < 1_000) return String(n)
  // One decimal, with a trailing `.0` dropped: `128.4k`, `840k`, `1M`, `1.5M`. The mock's
  // own figure is `128.4k / 1M`, so the tenth is kept at every magnitude rather than being
  // dropped above some threshold — a rule that would have rendered the design's own example
  // as `128k`.
  const scaled = n < 1_000_000 ? [n / 1_000, 'k'] : [n / 1_000_000, 'M']
  const [value, unit] = scaled as [number, string]
  const text = value.toFixed(1).replace(/\.0$/, '')
  return `${text}${unit}`
}

/**
 * The status bar's Claude slot: `claude · opus 5 · 128.4k / 1M`.
 *
 * Degrades one field at a time rather than all-or-nothing, because a partial readout is
 * still useful and a missing one is not an error — a session that has not run a turn yet
 * genuinely has no token figures.
 */
export function formatClaude(p: StatusPayload | undefined): string | undefined {
  if (!p) return undefined

  const parts = ['claude']

  const model = p.model?.display_name ?? p.model?.id
  if (model) parts.push(model)

  const used = usedTokens(p)
  const size = p.context_window?.context_window_size
  if (typeof used === 'number' && typeof size === 'number') {
    parts.push(`${compact(used)} / ${compact(size)}`)
  } else if (typeof used === 'number') {
    parts.push(compact(used))
  }

  // Only ever shown once it is non-zero. A confident `$0.00` on a session that simply has
  // not reported yet reads as "this is free", which is the wrong impression to give.
  const cost = p.cost?.total_cost_usd
  if (typeof cost === 'number' && cost > 0) parts.push(`$${cost.toFixed(2)}`)

  return parts.length > 1 ? parts.join(' · ') : undefined
}
