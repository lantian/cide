/** Find confirmed user blocks in a parsed terminal, rather than searching every occurrence of
 * a prompt in the model's output. A match requires the CLI's role marker, complete prompt
 * text, an assistant/tool boundary afterwards, and no overlap with the editable composer.
 * Unknown layouts or ambiguous repeated matches simply remain undecorated. */
export interface InputLine { text: string; wrapped: boolean }
export interface InputBlock { start: number; end: number }
export interface SubmittedInput { text: string }
export interface InputLocation extends InputBlock { ordinal: number }

const normalise = (text: string): string => text.replace(/\s+/gu, ' ').trim()

function promptStart(line: string, harness: 'claude' | 'codex'): string | null {
  const pattern = harness === 'codex' ? /^ ?› (.*)$/u : /^ ?[❯>] (.*)$/u
  return pattern.exec(line)?.[1] ?? null
}

function assistantStart(line: string): boolean {
  return /^\s*[⏺●•◦✻✽✶✳✢✺·]/u.test(line)
    || /^\s*─+\s*(Worked|Working|Thinking)/u.test(line)
}

function matchedBlocks(lines: readonly InputLine[], inputs: readonly SubmittedInput[],
  harness: 'claude' | 'codex', cursorRow: number): (InputBlock & { text: string })[] {
  const counts = new Map<string, number>()
  for (const input of inputs) {
    const text = normalise(input.text)
    if (text) counts.set(text, (counts.get(text) ?? 0) + 1)
  }
  const found = new Map<string, InputBlock[]>()
  for (let start = 0; start < lines.length; start++) {
    const first = lines[start]
    if (!first || first.wrapped) continue
    const prefix = promptStart(first.text, harness)
    if (prefix === null || !prefix.trim()) continue
    const candidates = [...counts.keys()].filter((text) => text.startsWith(normalise(prefix)))
    if (!candidates.length) continue
    let assembled = prefix
    for (let end = start; end < lines.length; end++) {
      if (end > start) {
        const row = lines[end]
        if (!row || assistantStart(row.text)) break
        assembled += row.wrapped ? row.text : `\n${row.text.trimStart()}`
      }
      const text = normalise(assembled)
      if (!candidates.some((candidate) => candidate.startsWith(text))) break
      if (!counts.has(text)) continue
      // A draft can have exactly the text of a previous prompt. The cursor and surrounding
      // composer rules distinguish it from a submitted message without inspecting keystrokes.
      if (cursorRow >= start && cursorRow <= end) break
      let next = end + 1
      while (next < lines.length && !lines[next]?.text.trim()) next++
      if (!assistantStart(lines[next]?.text ?? '')) continue
      const blocks = found.get(text) ?? []
      blocks.push({ start, end })
      found.set(text, blocks)
      break
    }
  }
  return [...found.entries()].flatMap(([text, blocks]) =>
    blocks.length <= (counts.get(text) ?? 0) ? blocks.map(block => ({ ...block, text })) : [],
  ).sort((a, b) => a.start - b.start)
}

export function inputBlocks(lines: readonly InputLine[], inputs: readonly SubmittedInput[],
  harness: 'claude' | 'codex', cursorRow: number): InputBlock[] {
  return matchedBlocks(lines, inputs, harness, cursorRow).map(({ start, end }) => ({ start, end }))
}

/** A visible suffix can contain repeated prompts. Only associate a block with a history
 * ordinal when both the earliest and latest chronological alignment agree. */
export function inputLocations(lines: readonly InputLine[], inputs: readonly (SubmittedInput & { ordinal: number })[],
  harness: 'claude' | 'codex', cursorRow: number): InputLocation[] {
  const blocks = matchedBlocks(lines, inputs, harness, cursorRow)
  const history = inputs.map(input => ({ ordinal: input.ordinal, text: normalise(input.text) }))
  const earliest: number[] = [], latest: number[] = []
  let index = 0
  for (const block of blocks) {
    while (index < history.length && history[index]?.text !== block.text) index++
    if (index === history.length) return []
    earliest.push(index++)
  }
  index = history.length - 1
  for (let i = blocks.length - 1; i >= 0; i--) {
    while (index >= 0 && history[index]?.text !== blocks[i]?.text) index--
    if (index < 0) return []
    latest[i] = index--
  }
  return blocks.flatMap((block, i) => {
    const entry = history[earliest[i] ?? -1]
    return earliest[i] === latest[i] && entry ? [{ start: block.start, end: block.end, ordinal: entry.ordinal }] : []
  })
}

/** The recap describes output at the viewport's top. When a prompt itself reaches that
 * edge, its predecessor is still the context; after the prompt passes above it, it owns
 * the following output. Undefined means the retained grid supplies no reliable anchor. */
export function visibleInputOrdinal(locations: readonly InputLocation[], top: number): number | undefined {
  let preceding: number | undefined
  for (const location of locations) {
    if (location.end < top) preceding = location.ordinal
    else return preceding ?? Math.max(1, location.ordinal - 1)
  }
  return preceding
}
