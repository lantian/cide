/** Presentation only: the editor and saved scenario body retain their original Markdown. */
export type ScenarioLine =
  | { kind: 'clause'; keyword: 'WHEN' | 'THEN' | 'AND'; text: string }
  | { kind: 'text'; text: string }

const CLAUSE = /^ {0,3}(?:[-*+][ \t]+)?(?:\*\*(WHEN|THEN|AND)\*\*|(WHEN|THEN|AND))(?:[ \t]+|$)(.*)$/
const FENCE = /^ {0,3}(`{3,}|~{3,})(.*)$/

/** Recognize clause prefixes without interpreting arbitrary prose or fenced examples. */
export function scenarioLines(body: string): ScenarioLine[] {
  let fence: { marker: string; length: number } | null = null
  return body.split(/\r?\n/).map((text): ScenarioLine => {
    const boundary = FENCE.exec(text)
    if (fence !== null) {
      if (
        boundary !== null &&
        boundary[1]![0] === fence.marker &&
        boundary[1]!.length >= fence.length &&
        boundary[2]!.trim() === ''
      ) fence = null
      return { kind: 'text', text }
    }
    if (boundary !== null && !(boundary[1]![0] === '`' && boundary[2]!.includes('`'))) {
      fence = { marker: boundary[1]![0]!, length: boundary[1]!.length }
      return { kind: 'text', text }
    }
    const clause = CLAUSE.exec(text)
    if (clause === null) return { kind: 'text', text }
    return {
      kind: 'clause',
      keyword: (clause[1] ?? clause[2]) as 'WHEN' | 'THEN' | 'AND',
      text: clause[3]!,
    }
  })
}
