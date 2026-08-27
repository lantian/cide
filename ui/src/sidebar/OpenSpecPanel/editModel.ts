/**
 * The requirement editor's pure core. (M28)
 *
 * Import-free, like every other model in this directory, so `check:openspec` can compile it
 * standalone with a bare `tsc` and drive it under node.
 *
 * # What is structured, and what deliberately is not
 *
 * A requirement has a **name**, some **prose**, and a list of **scenarios**. The name is a real
 * field: the archive matches requirements by it, so getting it wrong is not a formatting mistake
 * but a silent delete-and-add. The prose is a textarea. And a scenario is *also* a textarea —
 * title and body, as written — rather than a pair of WHEN/THEN inputs.
 *
 * That last one is the decision worth stating, because rigid clause fields are the obvious design
 * and they are wrong here. A scenario is free markdown: OpenSpec's own templates show `- **WHEN**`
 * and `- **THEN**`, but nothing enforces two clauses, an `AND` is ordinary, and people write
 * prose, tables and links inside them. Structured fields would need a parser *and* a serialiser
 * that round-trip every scenario anybody ever hand-wrote — and the first one that did not would
 * silently rewrite a committed file that a reviewer had already approved. So the editor keeps the
 * text and offers a **template**: `+ Scenario` seeds the canonical shape, and the clause chips
 * insert a line at the caret. The vocabulary is taught without being owned.
 *
 * # Composing is not round-tripping
 *
 * [`compose`] builds a block from the fields, and a block built from unedited fields is *not*
 * guaranteed byte-identical to the one that was read — blank-line runs and trailing spaces
 * inside the prose are normalised. That is why [`dirty`] exists and why the host sends
 * `requirement.block` unchanged when nothing was edited: a save that reformatted a file because
 * somebody opened and closed an editor would be a diff nobody asked for.
 */

/** Which field of which requirement is open. */
export interface EditTarget {
  /** Index into the change's deltas. */
  delta: number
  /** Index into that delta's requirements. */
  requirement: number
}

/** The whole of what an open editor holds. */
export interface RequirementDraft {
  name: string
  text: string
  scenarios: ScenarioDraft[]
}

export interface ScenarioDraft {
  title: string
  body: string
}

/** A target as the one string a `data-target` attribute can carry. */
export function targetId(target: EditTarget): string {
  return `d${target.delta}.r${target.requirement}`
}

/**
 * A `data-target` back into a target, or `null`.
 *
 * Takes a `string` and not an `EditTarget`, which is the point: the value arrives off an
 * attribute, so `'constructor'`, `''` and `'d1.rX'` are all real inputs. `null` makes them
 * no-ops instead of `NaN` indices that read past the end of an array.
 */
export function parseTarget(id: string): EditTarget | null {
  const match = /^d(\d+)\.r(\d+)$/.exec(id)
  if (match === null) return null
  const delta = Number(match[1])
  const requirement = Number(match[2])
  if (!Number.isInteger(delta) || !Number.isInteger(requirement)) return null
  return { delta, requirement }
}

export function sameTarget(a: EditTarget | null, b: EditTarget | null): boolean {
  if (a === null || b === null) return a === b
  return a.delta === b.delta && a.requirement === b.requirement
}

/** The draft an editor opens on, from what is in the file. */
export function draftOf(requirement: {
  name: string
  text: string
  scenarios: readonly { title: string; body: string }[]
}): RequirementDraft {
  return {
    name: requirement.name,
    text: requirement.text,
    scenarios: requirement.scenarios.map((scenario) => ({
      title: scenario.title,
      body: scenario.body,
    })),
  }
}

/**
 * Has anything actually been typed?
 *
 * Compared field by field against the requirement it opened on. The alternative — comparing
 * `compose(draft)` against `requirement.block` — would report *every* draft as dirty, because
 * composing normalises whitespace the file may carry; and a Save that rewrote a file because
 * somebody opened an editor and closed it again is a diff nobody asked for.
 */
export function dirty(
  draft: RequirementDraft,
  requirement: {
    name: string
    text: string
    scenarios: readonly { title: string; body: string }[]
  },
): boolean {
  if (draft.name !== requirement.name) return true
  if (draft.text !== requirement.text) return true
  if (draft.scenarios.length !== requirement.scenarios.length) return true
  return draft.scenarios.some((scenario, index) => {
    const was = requirement.scenarios[index]
    return was === undefined || scenario.title !== was.title || scenario.body !== was.body
  })
}

/** The canonical shape `+ Scenario` seeds, and the shape [`compose`] writes. */
export const SCENARIO_TEMPLATE = '- **WHEN** \n- **THEN** '

/** The clauses the chip row inserts. `AND` is there because two is not the rule. */
export type Clause = 'WHEN' | 'THEN' | 'AND'
export const CLAUSES: readonly Clause[] = ['WHEN', 'THEN', 'AND']

/**
 * Insert a clause line at the caret.
 *
 * `applyTool`'s signature one directory over, and for its reason: the caller has to put the caret
 * back, and only this function knows where the inserted text ended.
 *
 * The line is inserted *whole* — on its own line, after whatever line the caret is in — because a
 * clause spliced into the middle of a word is never what was meant, and undoing it costs more
 * than typing the bullet by hand would have.
 */
export function insertClause(
  text: string,
  selStart: number,
  clause: string,
): { text: string; selStart: number; selEnd: number } {
  // Clamped, because a selection index can outlive the text it was taken from — a chip pressed
  // after the draft was replaced would otherwise slice at a negative or past-the-end offset.
  const at = Math.max(0, Math.min(selStart, text.length))
  const lineEnd = text.indexOf('\n', at)
  const cut = lineEnd === -1 ? text.length : lineEnd
  const before = text.slice(0, cut)
  const after = text.slice(cut)
  const line = `- **${clause}** `
  const prefix = before === '' || before.endsWith('\n') ? '' : '\n'
  const inserted = `${prefix}${line}`
  return {
    text: `${before}${inserted}${after}`,
    selStart: cut + inserted.length,
    selEnd: cut + inserted.length,
  }
}

/** A blank scenario, appended. */
export function addScenario(draft: RequirementDraft): RequirementDraft {
  return {
    ...draft,
    scenarios: [...draft.scenarios, { title: '', body: SCENARIO_TEMPLATE }],
  }
}

/** Drop one scenario. Out-of-range is a no-op rather than a hole in the array. */
export function removeScenario(draft: RequirementDraft, index: number): RequirementDraft {
  if (index < 0 || index >= draft.scenarios.length) return draft
  return { ...draft, scenarios: draft.scenarios.filter((_, at) => at !== index) }
}

/** Replace one scenario's field. Out-of-range is a no-op. */
export function setScenario(
  draft: RequirementDraft,
  index: number,
  patch: Partial<ScenarioDraft>,
): RequirementDraft {
  if (index < 0 || index >= draft.scenarios.length) return draft
  return {
    ...draft,
    scenarios: draft.scenarios.map((scenario, at) =>
      at === index ? { ...scenario, ...patch } : scenario,
    ),
  }
}

/**
 * The markdown block a draft becomes.
 *
 * The canonical OpenSpec shape, which is also what the templates write:
 *
 * ```text
 * ### Requirement: <name>
 * <prose>
 *
 * #### Scenario: <title>
 * <body>
 * ```
 *
 * Rust refuses anything this could get wrong *before* the file is touched — a header whose name
 * does not match the requirement being replaced, no scenario at all, no SHALL or MUST, and (for a
 * MODIFIED delta) a scenario the current block has and this one does not. So a compose that is
 * not valid comes back as a sentence naming the field, rather than as a validator issue list
 * after a write and a rollback.
 */
export function compose(draft: RequirementDraft): string {
  const lines: string[] = [`### Requirement: ${draft.name.trim()}`]
  const text = draft.text.trim()
  if (text !== '') lines.push(text)
  for (const scenario of draft.scenarios) {
    lines.push('')
    lines.push(`#### Scenario: ${scenario.title.trim()}`)
    const body = scenario.body.trim()
    if (body !== '') lines.push(body)
  }
  return `${lines.join('\n')}\n`
}

/**
 * Why Save is off, or `null` when it is on.
 *
 * The same three rules Rust enforces, asked here so the sentence names the field being typed in
 * at the moment Save is pressed — rather than arriving as a validator issue after a write that
 * had to be rolled back. Rust asks them again regardless: this is a courtesy, not the guard.
 */
export function saveRefusal(draft: RequirementDraft): string | null {
  if (draft.name.trim() === '') {
    return 'A requirement needs a name — it is what the archive matches on.'
  }
  const body = `${draft.text}\n${draft.scenarios.map((s) => s.body).join('\n')}`
  if (!/\b(SHALL|MUST)\b/.test(body) && !/\b(SHALL|MUST)\b/.test(draft.text)) {
    return 'A requirement has to say SHALL or MUST — that is what makes it a requirement rather than a note, and openspec validate refuses it without one.'
  }
  if (draft.scenarios.length === 0) {
    return 'Every requirement needs at least one scenario — one with none is a statement nothing can check.'
  }
  const untitled = draft.scenarios.findIndex((scenario) => scenario.title.trim() === '')
  if (untitled >= 0) {
    return `Scenario ${untitled + 1} has no title. A scenario’s title is what says which case it is.`
  }
  return null
}
