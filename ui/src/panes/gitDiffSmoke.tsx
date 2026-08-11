/**
 * Renders `GitDiffView` against a fixture and prints what came out.
 *
 * Driven by `ui/scripts/check-diff-render.mjs`. Not part of the app — nothing imports it, so
 * it is tree-shaken out of the real bundle — and it exists for the reason
 * `sidebar/GitPanel/smokeEntry.tsx` does: compiling is not painting, and the claim this pane
 * has to keep is about *markup*. `check-diff-selection.mjs` proves the model maps a selection
 * to the same positions in both directions; this proves the rows the component marks are that
 * same set, so the sentence "what is highlighted is what is sent" covers the DOM too.
 *
 * The digest is deliberately positional: `selected` is the `hunk:line` of every row the view
 * drew as selected, which is the thing a user looks at.
 */
import { renderToStaticMarkup } from 'react-dom/server'
import { GitDiffView } from './GitDiffPane'
import { toSelection, resolve, type Marks } from '@/sidebar/GitPanel/diffSelection'
import type { DiffSide, DiffView, FileDiff, LineOrigin } from '@/ipc/client'

function line(origin: LineOrigin, content: string, oldLineno: number | null, newLineno: number | null) {
  return { origin, content, oldLineno, newLineno, noNewline: false }
}

/** The same three-hunk shape `check-diff-selection.mjs` uses, so the two agree by eye. */
export const FIXTURE: FileDiff = {
  path: 'src/main.rs',
  oldPath: null,
  side: 'unstaged',
  status: 'modified',
  binary: false,
  oldMode: 33188,
  newMode: 33188,
  rev: 'cafef00dcafef00d',
  partialOk: true,
  hunks: [
    {
      index: 0,
      header: '@@ -1,2 +1,3 @@ fn main() {',
      oldStart: 1,
      oldLines: 2,
      newStart: 1,
      newLines: 3,
      lines: [
        line('context', 'fn main() {', 1, 1),
        line('addition', '    let x = 1;', null, 2),
        line('context', '}', 2, 3),
      ],
    },
    {
      index: 1,
      header: '@@ -10,3 +11,3 @@',
      oldStart: 10,
      oldLines: 3,
      newStart: 11,
      newLines: 3,
      lines: [
        line('context', 'let a = 0;', 10, 11),
        line('deletion', 'let b = 1;', 11, null),
        line('addition', 'let b = 2;', null, 12),
        line('context', 'let c = 3;', 12, 13),
      ],
    },
    {
      index: 2,
      header: '@@ -20,1 +21,4 @@',
      oldStart: 20,
      oldLines: 1,
      newStart: 21,
      newLines: 4,
      lines: [
        line('addition', 'one', null, 21),
        line('addition', 'two', null, 22),
        line('addition', 'three', null, 23),
        line('context', 'end', 20, 24),
      ],
    },
  ],
}

/**
 * An addition immediately *followed* by a deletion, which the three-hunk fixture never has.
 *
 * `splitHunk` treats `+` then `-` as two separate edits and refuses to zip across the boundary,
 * because git emits `-` before `+` within one edit — so a `+` already closed the edit it
 * belonged to. Nothing in `FIXTURE` can tell that rule from its absence: it only ever has `-`
 * before `+`, where flushing and not flushing produce identical rows. This one fixture is the
 * difference between the rule being checked and merely being written down.
 */
export const PAIR_FIXTURE: FileDiff = {
  ...FIXTURE,
  path: 'src/pair.rs',
  hunks: [
    {
      index: 0,
      header: '@@ -1,3 +1,3 @@',
      oldStart: 1,
      oldLines: 3,
      newStart: 1,
      newLines: 3,
      lines: [
        line('context', 'keep', 1, 1),
        line('addition', 'added first', null, 2),
        line('deletion', 'removed after', 2, null),
        line('context', 'tail', 3, 3),
      ],
    },
  ],
}

export interface DiffDigest {
  name: string
  /** `hunk:line` of every row drawn as selected, in document order. */
  selected: string[]
  /** The same, as the wire `Selection` resolves it — these two must be equal. */
  sent: string[]
  /** `data-state` of each hunk's tri-state box, in order. */
  hunkBoxes: string[]
  rows: number
  count: string | null
  applyLabel: string | null
  applyDisabled: boolean
  note: string | null
  /**
   * Every `hunk:line` the view wrote, selected or not, in document order.
   *
   * The split layout draws a context line *twice* — once per column — and only one of the two
   * carries the position, so this is what pins that rule: a repeat here means the same mark is
   * in the DOM twice and "the rows drawn as selected" has stopped being a set of positions.
   */
  positions: string[]
}

function digest(
  name: string,
  marks: Marks,
  over: {
    diff: FileDiff | null
    side: DiffSide
    note?: string
    reason?: string
    held?: number
    /** Defaults to the unified layout, which is what `GitDiffView` defaults to. */
    view?: DiffView
  },
): DiffDigest {
  const html = renderToStaticMarkup(
    <GitDiffView
      diff={over.diff}
      path="src/main.rs"
      side={over.side}
      marks={marks}
      collapsed={new Set()}
      busy={false}
      note={over.note ?? null}
      reason={over.reason ?? null}
      held={over.held ?? null}
      {...(over.view === undefined ? {} : { view: over.view })}
      onSide={() => {}}
      onMarks={() => {}}
      onCollapse={() => {}}
      onApply={() => {}}
      onDropHeld={() => {}}
    />,
  )
  const rows = [
    ...html.matchAll(/data-audit="gitDiffRow" data-at="([\d:]+)" data-selected="(\w+)"/g),
  ]
  const apply = /data-audit="gitDiffApply"([^>]*)>([^<]*)</.exec(html)
  return {
    name,
    selected: rows.flatMap((m) => (m[2] === 'true' ? [m[1] ?? ''] : [])),
    sent: over.diff === null ? [] : [...resolve(over.diff, toSelection(marks, over.diff))].sort(),
    hunkBoxes: [...html.matchAll(/data-audit="gitDiffHunkBox" data-state="(\w+)"/g)].map(
      (m) => m[1] ?? '',
    ),
    rows: rows.length,
    count: /data-audit="gitDiffCount"[^>]*>([\s\S]*?)<\/span>/.exec(html)?.[1]?.replace(/<[^>]*>/g, '') ?? null,
    applyLabel: apply?.[2] ?? null,
    applyDisabled: apply?.[1]?.includes('disabled') ?? false,
    note: /data-audit="gitDiffNote"[^>]*>([^<]*)</.exec(html)?.[1] ?? null,
    positions: rows.map((m) => m[1] ?? ''),
  }
}

const digests: DiffDigest[] = [
  digest('empty', new Set(), { diff: FIXTURE, side: 'unstaged' }),
  digest('oneLine', new Set(['1:2']), { diff: FIXTURE, side: 'unstaged' }),
  digest('wholeHunk', new Set(['2:0', '2:1', '2:2']), { diff: FIXTURE, side: 'unstaged' }),
  digest('ragged', new Set(['0:1', '2:0', '2:2']), { diff: FIXTURE, side: 'staged' }),
  digest('everything', new Set(['0:1', '1:1', '1:2', '2:0', '2:1', '2:2']), {
    diff: FIXTURE,
    side: 'combined',
  }),
  // A context mark and a mark past the end of the diff: neither may light a row.
  digest('junk', new Set(['0:0', '9:9']), { diff: FIXTURE, side: 'unstaged' }),
  digest('binary', new Set(['0:1']), {
    diff: { ...FIXTURE, partialOk: false },
    side: 'unstaged',
  }),
  digest('gone', new Set(), { diff: null, side: 'staged', reason: 'src/main.rs has no changes' }),

  /*
   * The same claims again in the side-by-side layout.
   *
   * Without these the split view is drawn by code no check can fail: every case above renders
   * with `view` unset, which is `'unified'`, so `splitHunk` and the `cell()` calls that pair
   * its rows were reachable from the app and from nothing that runs in CI. The pane exists to
   * keep one promise — what is highlighted is what is staged — and that promise has to be
   * proved in both layouts or the layout switch is the place it silently stops holding.
   */
  digest('splitEmpty', new Set(), { diff: FIXTURE, side: 'unstaged', view: 'split' }),
  digest('splitRagged', new Set(['0:1', '2:0', '2:2']), {
    diff: FIXTURE,
    side: 'staged',
    view: 'split',
  }),
  digest('splitWholeHunk', new Set(['2:0', '2:1', '2:2']), {
    diff: FIXTURE,
    side: 'unstaged',
    view: 'split',
  }),
  digest('splitEverything', new Set(['0:1', '1:1', '1:2', '2:0', '2:1', '2:2']), {
    diff: FIXTURE,
    side: 'combined',
    view: 'split',
  }),
  digest('splitJunk', new Set(['0:0', '9:9']), { diff: FIXTURE, side: 'unstaged', view: 'split' }),

  /*
   * The pairing boundary, in both layouts so the two can be compared.
   *
   * `positions` is in *document order* here and that is the point: the sorted comparisons above
   * cannot see a regrouping, only a gain or loss. Facing `added first` against `removed after`
   * — one edit's `+` against the next edit's `-` — reorders the columns without changing the
   * set, and reading a diff that claims one line became another when it did not is exactly the
   * wrong answer this pane must not give.
   */
  digest('pairUnified', new Set(['0:1']), { diff: PAIR_FIXTURE, side: 'unstaged' }),
  digest('pairSplit', new Set(['0:1']), { diff: PAIR_FIXTURE, side: 'unstaged', view: 'split' }),
]

console.log(JSON.stringify(digests))
