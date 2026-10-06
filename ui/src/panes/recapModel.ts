/** Conversation-scoped navigation. null means follow the newest prompt; a concrete ordinal
 * holds the user's place while the CLI is printing another turn. Import-free for the check. */
export interface PromptEntry { id: string; ordinal: number; text: string }

export function mergeInputs(previous: readonly PromptEntry[], incoming: readonly PromptEntry[]): PromptEntry[] {
  const byOrdinal = new Map(previous.map((entry) => [entry.ordinal, entry]))
  for (const entry of incoming) byOrdinal.set(entry.ordinal, entry)
  return [...byOrdinal.values()].sort((a, b) => a.ordinal - b.ordinal)
}

export function selectedOrdinal(selected: number | null, total: number): number {
  return selected === null ? total : Math.min(Math.max(1, selected), total)
}

export function navigateInput(selected: number | null, total: number, delta: number): number | null {
  const next = Math.min(total, Math.max(1, selectedOrdinal(selected, total) + delta))
  return next >= total ? null : next
}
