/**
 * Renders every Tasks panel story to static HTML and prints one JSON digest line. (M18)
 *
 * Driven by `ui/scripts/check-agents-render.mjs`. Not part of the app — nothing imports it, so
 * it is tree-shaken out of the real bundle. Modelled on `ProblemsPanel/smokeEntry.tsx`.
 *
 * `model.ts` is checked directly under node by `check-agents.mjs`; what this adds is the half
 * the pure core cannot speak for. Four things, each pinned to a failure this repository has
 * actually had:
 *
 *  - that the **unreadable** board renders **zero** writing controls. `canWrite` returning
 *    `false` is worth nothing if the markup offers a New task button anyway, and the file it
 *    would overwrite is most often a half-resolved merge conflict — that is to say, a file
 *    still containing both sides of everything.
 *  - that a **live run's chip and an assigned-but-idle chip use different classes**. This is
 *    the single most important assertion in the file: a chip that looked the same either way
 *    would say an agent that exited an hour ago is still working.
 *  - that the detail view lists comments **oldest first**, from a fixture that arrives
 *    scrambled.
 *  - that a class referenced as `styles.x` exists in the stylesheet — see the Agents smoke
 *    entry's header for why neither `tsc` nor `vite build` can see that one.
 *  - that the **delete** control exists at all, in both places, and that its confirming half
 *    appears only when armed. `task_delete` was a registered command with a client wrapper and a
 *    working store action that **no component rendered a control for** — this project's
 *    twenty-second "built and reachable from nothing", and the class of failure no other gate in
 *    the repository can see. Counting the two halves as *elements* is what closes it.
 *  - that a **filtered** list omits the tasks that do not match and that its empty result is a
 *    different sentence from an empty tracker's. A filter that silently showed nothing is how a
 *    user concludes their tasks are gone.
 *  - that the card is **read-only at rest**: not one `<input>`, `<textarea>` or `<select>` in any
 *    of its fields until a field is put into edit, and then exactly one. That is the whole of the
 *    change that made this a modal, and it is a claim only markup can settle — `check-agents.mjs`
 *    can prove `beginEdit` opens one field at a time and still not see a card that draws three
 *    live boxes regardless.
 *
 * # Two components, one digest
 *
 * `TasksPanelView` for the list stories and `TaskDetail` for the card ones. The card's real
 * mount, `TaskDetailModal`, is an `OverlayCard` and therefore a **portal**, which
 * `react-dom/server` refuses outright — *"Portals are not currently supported by the server
 * renderer"*. `TaskDetail` is that component minus the one-line wrapper, so everything inside
 * the dialog stays inside this gate; `check-agents-render.mjs` reads the wrapper as source.
 */
import { renderToStaticMarkup } from 'react-dom/server'
import { TasksPanelView } from './TasksPanel'
import { TaskDetail } from './TaskDetail'
import { TaskCompose } from './TaskCompose'
import { MentionList } from './MentionTextarea'
import {
  CARD_STORIES,
  COMPOSE_STORIES,
  MENTION_STORIES,
  TASKS_STORIES,
  type CardStoryName,
  type ComposeStoryName,
  type MentionStoryName,
  type TasksStoryName,
} from './fixture'

/** What the check script asserts on. */
export interface TasksDigest {
  story: TasksStoryName | CardStoryName | ComposeStoryName | MentionStoryName
  /** The header's right-hand figure. Empty when `metaFigure` withheld it. */
  meta: string
  claim: string | null
  /** Every rendered button's text, in document order. `board-unknown` must have none. */
  buttons: string[]
  /** The whole panel as readable text — what the path and sentence greps run over. */
  text: string
  /**
   * How many controls can write to `.cide/tasks.json`.
   *
   * Counted by a `data-write="true"` marker rather than by naming the buttons, so a control
   * added later is included by default and has to be *deliberately* left out of the count.
   */
  writeControls: number
  /** Group headings, in the order drawn: `<label>|<count>`. */
  groups: string[]
  /** One entry per list row: `<status>|<icon name>|<id>|<title>`. */
  rows: string[]
  /** Each chip's class attribute, verbatim. Hashed, but stable within one bundle. */
  chipClasses: string[]
  /** Each chip's `data-lit`, in the same order. */
  chipLit: string[]
  /**
   * Each chip's `data-tone`, in the same order. `lit` alone cannot tell stalled from plain
   * assigned — both are unlit — and the stalled reading only exists as the pair
   * (`lit: false`, `tone: 'attention'`), so the check needs both discriminators in the digest
   * to assert the pair rather than half of it.
   */
  chipTone: string[]
  /** Each chip's label. */
  chipLabels: string[]
  /** Comment log lines, in the order drawn: `<author kind>|<text>`. The text is the rendered
   *  markdown flattened back to prose, so `**bold**` digests as `bold` — which is itself the
   *  assertion that the syntax was consumed rather than printed. */
  comments: string[]
  /**
   * How many per-comment Edit and Delete controls are drawn, and how many action rows hold them.
   *
   * Counted because until M21 no story passed `onEditComment`/`onDeleteComment`, so the gate
   * rendered a log with no controls on it while the app rendered two per line. A count of rows
   * as well as of buttons is what pins them to their **own row** under the comment rather than
   * back in the head beside the timestamp, which is where they were and what was reported.
   */
  commentEdits: number
  commentDeletes: number
  commentActionRows: number
  /** Whether the card is on screen at all. */
  detail: boolean
  /**
   * One entry per field row: `<field>|<mode>`, in the order drawn.
   *
   * `rest` is read-only text, `edit` is that field's control, and `live` is the status segment —
   * the one field with no affordance, whose control is always there. The whole read/edit posture
   * is visible in this array and nowhere else.
   */
  fields: string[]
  /**
   * How many form controls live **inside a field row**: `<input>`, `<textarea>`, `<select>`.
   *
   * Counted inside the rows on purpose, so the comment composer's textarea — which is not a
   * field, and deliberately has no affordance — cannot make a card at rest look like it has one.
   * At rest this must be `0`; with a field in edit, exactly `1`.
   */
  fieldControls: number
  /** How many edit affordances are on screen. One per editable field, and never on `status`. */
  fieldEdits: number
  /** How many Save controls. The assignee's editor commits on choice and draws none. */
  fieldSaves: number
  /** Each field's rest text, as `<field>|<text>`. Empty for a field that is in edit. */
  fieldValues: string[]
  /** Whether the card draws an explicit close control. */
  close: boolean
  /**
   * The status log. (M27) `historyDrawn` is whether the disclosure exists at all — it must not
   * for a task that never moved. `historyOpen` is whether it is expanded, and the load-bearing
   * assertion is that it never is: collapsed-by-default is the whole posture, and an `open`
   * that crept onto the `<details>` would ship every card with its audit trail unfolded.
   * `history` is one entry per row, in the order drawn: `<from> → <to>|<author>`.
   */
  historyDrawn: boolean
  historyOpen: boolean
  history: string[]
  /**
   * Every timestamp on the card — comment heads and history rows — as its rendered text. (M27)
   *
   * Asserted by *shape* (`HH:MM:SS (… ago)`) rather than by value, because the clock half is
   * local time and the render check runs in whatever timezone the machine has.
   */
  times: string[]
  /**
   * Whether the compose dialog is on screen at all, and what it drew. (M21)
   *
   * `composeControls` counts `<input>`/`<textarea>`/`<select>` inside the dialog's rows, exactly
   * as `fieldControls` does for the card — and the two assertions are inverses: the card must
   * have **none** at rest, this must have all of them at once. `composeCreate` is `''` when
   * there is no Create at all and `on`/`off` otherwise, so "the button vanished" and "the button
   * is waiting" cannot be digested as the same thing.
   */
  compose: boolean
  composeControls: number
  composeStatuses: string[]
  composeCreate: string
  composeCancel: boolean
  /**
   * The card's creator line, and the author arm it was drawn from: `<kind>|<text>`.
   *
   * The kind is digested beside the words so the check can assert the label was read off the
   * author enum rather than off a name — `You` is a claim about the *user's* own tasks, and a
   * creator line that string-matched a role called "you" would print it for an agent's.
   */
  creator: string
  /** The ids of list rows marked as the open one. At most one, and only in the list stories. */
  openRows: string[]
  /** Whether the live-run strip is drawn. */
  runStrip: boolean
  /** The status filter's toggles, in the order drawn: `<label>|<on>`. Empty when not drawn. */
  filters: string[]
  /** Whether the search box is on screen, and what is typed in it. */
  search: boolean
  searchValue: string
  /** Whether the box's clear control is drawn — it must exist exactly when text does. */
  searchClear: boolean
  /** Whether the filtered-to-nothing block is on screen. Never true beside a populated list. */
  noMatch: boolean
  /**
   * How many **unarmed** delete controls are on screen, and how many **confirming** ones.
   *
   * Counted as elements rather than by reading a label, for `ready-queued`'s reason one panel
   * over: a well-meaning "grey it out instead" must not be able to pass. The pair is what the
   * check compares — an unarmed row must offer the first and none of the second.
   */
  deletes: number
  deleteConfirms: number
  /**
   * The sentence under the assignee control saying why the list is short, or `''` when none is
   * drawn. The dropdown shipped empty-by-wiring-bug for one whole milestone; the hint is what
   * makes a legitimately-short list look different from that, and this field is its gate.
   */
  assigneeHint: string
  /** Every `<option>` on screen, in order. Only the assignee `<select>` renders any. */
  assigneeOptions: string[]
  /**
   * The @mention popup's rows: `<text>|<aria-selected>`, in the order drawn. The scored order
   * and the exactly-one-highlight rule are both read off this.
   */
  mentionRows: string[]
  /**
   * How many formatting-toolbar buttons are on screen. (M27)
   *
   * Counted rather than boolean because the toolbar rides `MentionTextarea` and every
   * markdown-bearing textarea must carry one: the card at rest has exactly the composer's
   * eight, a card with its body in edit has that editor's eight as well, and a story that
   * drifts to "some textareas have tools" is the state the count makes visible.
   */
  mdTools: number
  /**
   * What the rendered markdown actually produced, as element counts over the whole story. (M27)
   *
   * The pair of claims these carry: markdown *renders* (a story whose fixture says `**bold**`
   * must count a `<strong>`) and markdown renders **as elements, never as an HTML string** —
   * these are matched against real tags in server-rendered markup, which no
   * `dangerouslySetInnerHTML` output would ever have to contain.
   */
  md: {
    strong: number
    em: number
    code: number
    headings: number
    items: number
    fences: number
    links: number
  }
  /** See the Agents digest: hooked-but-unstyled elements, plus `class` lists carrying
   *  the literal token `undefined`. */
  unclassed: number
}

const digests: TasksDigest[] = [
  ...(Object.keys(TASKS_STORIES) as TasksStoryName[]).map((story) =>
    digest(story, renderToStaticMarkup(<TasksPanelView {...TASKS_STORIES[story]} />)),
  ),
  ...(Object.keys(CARD_STORIES) as CardStoryName[]).map((story) =>
    digest(story, renderToStaticMarkup(<TaskDetail {...CARD_STORIES[story]} />)),
  ),
  ...(Object.keys(COMPOSE_STORIES) as ComposeStoryName[]).map((story) =>
    digest(story, renderToStaticMarkup(<TaskCompose {...COMPOSE_STORIES[story]} />)),
  ),
  ...(Object.keys(MENTION_STORIES) as MentionStoryName[]).map((story) =>
    digest(story, renderToStaticMarkup(<MentionList {...MENTION_STORIES[story]} />)),
  ),
]
console.log(JSON.stringify(digests))

function digest(
  story: TasksStoryName | CardStoryName | ComposeStoryName | MentionStoryName,
  html: string,
): TasksDigest {
  /*
   * Local class names survive Vite's hashing as a substring — `_taskTitle_1a2b3` — which is what
   * `ProblemsPanel/smokeEntry.tsx` already relies on. Keyed off those rather than off a position
   * in the row, so a reordered row cannot make this silently digest the id as the title.
   */
  const rows = all(html, 'tasksRow').map((row) => {
    // The mark's name off `data-icon`, not its text: a drawn mark has no text node. See
    // `icons/Icon.tsx` for why the attribute is there.
    const glyph =
      /data-audit="tasksGlyph"[^>]*>\s*<svg[^>]*data-icon="([^"]*)"/.exec(row)?.[1] ?? ''
    const status = /data-audit="tasksGlyph"[^>]*data-status="([^"]*)"/.exec(row)?.[1] ?? ''
    const title = text(/class="[^"]*taskTitle[^"]*"[^>]*>([^<]*)</.exec(row)?.[1] ?? '')
    return `${status}|${glyph}|${attr(row, 'data-task')}|${title}`
  })
  return {
    story,
    meta: text(/data-audit="tasksMeta"[^>]*>([^<]*)</.exec(html)?.[1] ?? ''),
    claim: text(/data-audit="tasksClaim"[^>]*>([^<]*)</.exec(html)?.[1] ?? '') || null,
    buttons: [...html.matchAll(/<button\b[^>]*>([\s\S]*?)<\/button>/g)].map((m) => text(m[1] ?? '')),
    text: text(html),
    writeControls: count(html, 'data-write="true"'),
    groups: all(html, 'tasksGroup').map((group) => {
      const label = /class="[^"]*groupTitle[^"]*"[^>]*>([^<]*)</.exec(group)?.[1] ?? ''
      const n = [...group.matchAll(/data-audit="tasksRow"/g)].length
      return `${text(label)}|${n}`
    }),
    rows,
    chipClasses: [...html.matchAll(/class="([^"]*)"\s+data-audit="tasksChip"/g)].map((m) => m[1] ?? ''),
    chipLit: [...html.matchAll(/data-audit="tasksChip"[^>]*data-lit="([^"]*)"/g)].map((m) => m[1] ?? ''),
    chipTone: [...html.matchAll(/data-audit="tasksChip"[^>]*data-tone="([^"]*)"/g)].map((m) => m[1] ?? ''),
    chipLabels: all(html, 'tasksChip').map((chip) =>
      text(/class="[^"]*chipLabel[^"]*"[^>]*>([^<]*)</.exec(chip)?.[1] ?? ''),
    ),
    /* The whole `tasksCommentText` element, not a slice-to-`</p>`: a comment renders as
       markdown now, so its container holds block elements of its own and the first close tag
       is no longer the end of the text. */
    comments: all(html, 'tasksComment').map(
      (entry) => `${attr(entry, 'data-author')}|${text(all(entry, 'tasksCommentText')[0] ?? '')}`,
    ),
    commentEdits: count(html, 'data-audit="tasksCommentEdit"'),
    commentDeletes: count(html, 'data-audit="tasksCommentDelete"'),
    commentActionRows: count(html, 'data-audit="tasksCommentActions"'),
    detail: html.includes('data-audit="taskCard"'),
    fields: all(html, 'taskField').map(
      (field) => `${attr(field, 'data-field')}|${attr(field, 'data-mode')}`,
    ),
    fieldControls: all(html, 'taskField').reduce(
      (n, field) => n + [...field.matchAll(/<(input|textarea|select)\b/g)].length,
      0,
    ),
    fieldEdits: count(html, 'data-audit="taskFieldEdit"'),
    fieldSaves: count(html, 'data-audit="taskFieldSave"'),
    fieldValues: all(html, 'taskFieldValue').map(
      (value) => `${attr(value, 'data-field')}|${text(value)}`,
    ),
    close: html.includes('data-audit="tasksClose"'),
    historyDrawn: html.includes('data-audit="tasksHistory"'),
    historyOpen: /<details[^>]*data-audit="tasksHistory"[^>]*\sopen(?:=|\s|>)/.test(html),
    history: all(html, 'tasksHistoryRow').map((row) => {
      const move = text(/class="[^"]*historyMove[^"]*"[^>]*>([\s\S]*?)<\/span>/.exec(row)?.[1] ?? '')
      const by = text(/class="[^"]*historyBy[^"]*"[^>]*>([\s\S]*?)<\/span>/.exec(row)?.[1] ?? '')
      return `${move}|${by}`
    }),
    times: [
      ...html.matchAll(/class="[^"]*(?:logTime|historyTime)[^"]*"[^>]*>([\s\S]*?)<\/span>/g),
    ].map((m) => text(m[1] ?? '')),
    compose: html.includes('data-audit="taskCompose"'),
    composeControls: all(html, 'taskComposeRow').reduce(
      (n, row) => n + [...row.matchAll(/<(input|textarea|select)\b/g)].length,
      0,
    ),
    composeStatuses: all(html, 'taskComposeStatus').map(
      (button) => `${attr(button, 'data-status')}|${attr(button, 'aria-pressed')}`,
    ),
    composeCreate: (() => {
      const button = all(html, 'taskComposeCreate')[0]
      if (button === undefined) return ''
      return / disabled(?:=|\s|\/|>)/.test(button) ? 'off' : 'on'
    })(),
    composeCancel: html.includes('data-audit="taskComposeCancel"'),
    creator: all(html, 'tasksCreator')
      .map((span) => `${attr(span, 'data-creator')}|${text(span)}`)
      .join(''),
    openRows: all(html, 'tasksRow')
      .filter((row) => attr(row, 'data-open') === 'true')
      .map((row) => attr(row, 'data-task')),
    runStrip: html.includes('data-audit="tasksRunStrip"'),
    filters: all(html, 'tasksFilter').map(
      (button) => `${text(/>([^<]*)<\/button>/.exec(button)?.[1] ?? '')}|${attr(button, 'data-on')}`,
    ),
    search: html.includes('data-audit="tasksSearchInput"'),
    /* The whole `<input>` tag rather than `all()`: a void element has no closing tag for
       `outer` to count, and `value` is an attribute, so the tag is all there is to read. */
    searchValue: attr(
      /<input[^>]*data-audit="tasksSearchInput"[^>]*>/.exec(html)?.[0] ?? '',
      'value',
    ),
    searchClear: html.includes('data-audit="tasksSearchClear"'),
    noMatch: html.includes('data-audit="tasksNoMatch"'),
    deletes: count(html, 'data-audit="tasksDelete"'),
    deleteConfirms: count(html, 'data-audit="tasksDeleteConfirm"'),
    assigneeHint: text(all(html, 'tasksAssigneeHint')[0] ?? ''),
    assigneeOptions: [...html.matchAll(/<option[^>]*>([^<]*)<\/option>/g)].map((m) =>
      text(m[1] ?? ''),
    ),
    mdTools: count(html, 'data-audit="tasksMdTool"'),
    md: {
      strong: count(html, '<strong'),
      em: [...html.matchAll(/<em[\s>]/g)].length,
      code: count(html, 'mdCode'),
      headings: count(html, 'mdHeading'),
      items: count(html, '<li'),
      fences: count(html, 'mdPre'),
      links: count(html, 'mdLink'),
    },
    mentionRows: all(html, 'tasksMentionPopup').flatMap((popup) =>
      [...popup.matchAll(/<div[^>]*role="option"[^>]*>([\s\S]*?)<\/div>/g)].map((m) => {
        const selected = /aria-selected="([^"]*)"/.exec(m[0])?.[1] ?? ''
        return `${text(m[1] ?? '')}|${selected}`
      }),
    ),
    unclassed: unclassed(html),
  }
}

/* -------------------------------------------------------------------- markup scratching */

/**
 * Every element carrying `data-audit="<hook>"`, as its own outer HTML.
 *
 * Extents are found by counting opening and closing tags of the same name, rather than by
 * slicing to the next marker: a group contains its rows and a row contains its chip, so
 * slice-to-next-marker would attribute one element's children to another.
 */
function all(html: string, hook: string): string[] {
  return [...html.matchAll(new RegExp(`data-audit="${hook}"`, 'g'))].map((m) => outer(html, m.index))
}

function outer(html: string, at: number): string {
  const start = html.lastIndexOf('<', at)
  const name = /^<([a-zA-Z0-9]+)/.exec(html.slice(start, at + 1))?.[1]
  if (name === undefined) return ''
  const tags = new RegExp(`</?${name}\\b`, 'g')
  tags.lastIndex = start
  let depth = 0
  let m = tags.exec(html)
  while (m !== null) {
    depth += m[0].startsWith('</') ? -1 : 1
    if (depth === 0) {
      const end = html.indexOf('>', m.index)
      return html.slice(start, end === -1 ? undefined : end + 1)
    }
    m = tags.exec(html)
  }
  return html.slice(start)
}

function attr(fragment: string, name: string): string {
  return new RegExp(`${name}="([^"]*)"`).exec(fragment)?.[1] ?? ''
}

function count(haystack: string, needle: string): number {
  return haystack.split(needle).length - 1
}

/** Markup to readable text: drop tags, collapse whitespace, decode what React escapes. */
function text(html: string): string {
  return html
    .replace(/<[^>]*>/g, ' ')
    .replace(/&quot;/g, '"')
    .replace(/&#x27;/g, "'")
    .replace(/&amp;/g, '&')
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/\s+/g, ' ')
    .trim()
}

function unclassed(html: string): number {
  /* `/?` before the close: React writes void elements as `<input …/>`, and without it every
     `<input>` on the detail form would fall out of this counter unchecked. */
  const hooked = [...html.matchAll(/<[a-z]+((?:\s+[a-z-]+(?:="[^"]*")?)*)\s*\/?>/g)].filter((m) => {
    const attrs = m[1] ?? ''
    return attrs.includes('data-audit=') && !/\sclass="[^"]*[^"\s][^"]*"/.test(attrs)
  }).length
  const poisoned = [...html.matchAll(/class="([^"]*)"/g)].filter((m) =>
    (m[1] ?? '').split(/\s+/).includes('undefined'),
  ).length
  return hooked + poisoned
}
