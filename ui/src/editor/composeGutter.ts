/**
 * A run marker in a compose file's gutter. (M48)
 *
 * The fourth of the surfaces M48 gives the Compose verbs, and the only one that can act on a
 * single **service**: the other three know a file and nothing about what is inside it.
 *
 * # Why the menu is a tooltip with a React root, and not `useContextMenu`
 *
 * `changeBars.ts` built this road and its comments carry the detail. The short version is that a
 * gutter's `domEventHandlers` are a plain `addEventListener` on the gutter element, running
 * outside any React tree, so there is nothing for a hook to hang on. A `showTooltip` field driven
 * by a `StateEffect` is the shape CodeMirror already has for "something the gutter opened", and
 * it is what the change card uses three hundred lines away.
 *
 * # Why the targets are rescanned only when the document changes
 *
 * `composeTargets` is a linear pass over the text and the gutter asks for markers on every
 * viewport update — a scroll, a selection move, a focus change. Recomputing there would put a
 * full scan on gestures that cannot have changed the answer. The field below recomputes on
 * `docChanged` and on nothing else, which is `changeBars`' own discipline.
 *
 * # What this deliberately does not do
 *
 * It does not gate on whether the *daemon* is reachable. Deciding that would be a round trip per
 * buffer open, and the refusal it would produce ("no Compose on this machine") is a sentence
 * `docker_compose_plan` already returns — after the user has said plainly what they wanted, which
 * is the moment it is worth interrupting them. A marker that is occasionally optimistic beats a
 * gutter that is empty for a reason the user cannot see.
 */
import { RangeSet, StateEffect, StateField } from '@codemirror/state'
import type { EditorState, Extension } from '@codemirror/state'
import { EditorView, GutterMarker, gutter, showTooltip } from '@codemirror/view'
import type { Tooltip } from '@codemirror/view'
import { createElement } from 'react'
import { createRoot } from 'react-dom/client'
import { flushSync } from 'react-dom'
import {
  COMPOSE_LABEL,
  COMPOSE_VERBS,
  composeTargets,
  isComposePath,
  type ComposeTarget,
  type ComposeVerb,
} from '@/chrome/composeModel'
import { runCompose } from '@/chrome/composeRun'
import { iconElement } from '@/icons/iconElement'
import { ComposeRunMenu } from './ComposeRunMenu'

/** Which file this buffer is, so a picked verb knows what to run against. */
const pathFacet = StateField.define<string>({
  create: () => '',
  update: (value) => value,
})

/**
 * The marker: the `play` mark, built imperatively.
 *
 * `iconElement` and not a `▶` character, which is what this was first written with and which
 * `check:ui-icons` refused by name — the set is closed in both directions and a glyph beside a
 * column of drawn marks is the half-glyphs-half-paths state `chrome/ActivityRail.tsx` argued
 * against. `iconElement` exists for exactly this position: a `GutterMarker.toDOM` has no React
 * tree to render an `<Icon>` into, and `panes/mergeGutter.ts` sits in the same place.
 *
 * Size `0`, the smallest rung: the column is one mark wide and shares a line with 11px text.
 */
class RunMarker extends GutterMarker {
  constructor(readonly target: ComposeTarget) {
    super()
  }

  override eq(other: RunMarker): boolean {
    return other.target.line === this.target.line && other.target.service === this.target.service
  }

  override toDOM(): Node {
    const mark = iconElement('play', 0, 'cm-compose-run')
    // On the `<svg>` itself, so the tooltip follows the pointer onto the mark rather than onto a
    // wrapper the mark does not fill.
    mark.setAttribute(
      'aria-label',
      this.target.service === null
        ? 'Run Compose on this file'
        : `Run Compose on ${this.target.service}`,
    )
    mark.setAttribute('role', 'img')
    return mark
  }
}

/** The marker set, recomputed only when the text changed. */
const targetField = StateField.define<RangeSet<RunMarker>>({
  create: (state) => markers(state),
  update: (value, tr) => (tr.docChanged ? markers(tr.state) : value),
})

function markers(state: EditorState): RangeSet<RunMarker> {
  const found = composeTargets(state.doc.toString())
  return RangeSet.of(
    found
      // A target past the end of the document cannot happen from a fresh scan, and can from a
      // stale one; the guard costs nothing and `RangeSet.of` throws rather than ignoring it.
      .filter((target) => target.line >= 1 && target.line <= state.doc.lines)
      .map((target) => new RunMarker(target).range(state.doc.line(target.line).from)),
    true,
  )
}

/** Which marker's menu is open, or `null`. */
const setOpen = StateEffect.define<ComposeTarget | null>()

const openField = StateField.define<Tooltip | null>({
  create: () => null,
  update: (value, tr) => {
    for (const effect of tr.effects) {
      if (effect.is(setOpen)) {
        return effect.value === null ? null : menu(tr.state, effect.value)
      }
    }
    // Typing dismisses it, `changeBars`' rule: the alternative is mapping the position through
    // the change and leaving a menu pointing at a service that has just moved or been renamed.
    if (tr.docChanged) return null
    return value
  },
  provide: (field) => showTooltip.from(field),
})

function menu(state: EditorState, target: ComposeTarget): Tooltip | null {
  if (target.line < 1 || target.line > state.doc.lines) return null
  const path = state.field(pathFacet, false) ?? ''
  if (path === '') return null

  return {
    pos: state.doc.line(target.line).from,
    above: false,
    create: (view) => {
      const dom = document.createElement('div')
      dom.className = 'cm-compose-menu'
      const root = createRoot(dom)
      /*
       * `flushSync`, for `changeBars`' and `blame.ts`' reason: CodeMirror measures this element in
       * the same frame it is created and `root.render` is scheduled rather than synchronous, so
       * without it the menu is positioned against an empty box — visibly wrong near the foot of a
       * pane, where the flip-above decision is made from a height it does not yet have. Safe here
       * specifically because `create` is reached from the gutter's own DOM listener and never
       * from inside a React render.
       */
      flushSync(() => {
        root.render(
          createElement(ComposeRunMenu, {
            subject: target.service ?? 'Whole file',
            verbs: COMPOSE_VERBS.map((verb) => ({ id: verb, label: COMPOSE_LABEL[verb] })),
            onPick: (picked: string) => {
              // Closed first. `runCompose` splits a pane, which moves focus and re-lays out this
              // editor — a menu still open through that is a box floating over a pane that has
              // moved under it.
              view.dispatch({ effects: setOpen.of(null) })
              void runCompose(
                path,
                picked as ComposeVerb,
                target.service === null ? [] : [target.service],
              )
            },
          }),
        )
      })
      return {
        dom,
        destroy: () => {
          // Deferred, `changeBars`' note: `destroy` is reachable from a dispatch made inside a
          // React effect, and unmounting a root while React is flushing is the warning React 19
          // raises and then recovers from badly. The container is already detached by then.
          queueMicrotask(() => root.unmount())
        },
      }
    },
  }
}

/**
 * The gutter for one buffer, or nothing at all.
 *
 * Returns `[]` for a file Compose would not read, so a Rust buffer pays for none of this — not
 * the field, not the scan, and not the gutter column's width. That is why this is a function of
 * the path rather than one value for the process the way `CHANGE_BARS` is: the change column's
 * width is reserved unconditionally in the stylesheet and must exist in every buffer, and this
 * one must exist in almost none.
 */
export function composeGutter(path: string): Extension {
  if (!isComposePath(path)) return []
  return [
    pathFacet.init(() => path),
    targetField,
    openField,
    gutter({
      class: 'cm-cide-compose',
      markers: (view) => view.state.field(targetField, false) ?? RangeSet.empty,
      domEventHandlers: {
        mousedown: (view, block, event) => {
          const line = view.state.doc.lineAt(block.from)
          const hit = composeTargets(view.state.doc.toString()).find(
            (target) => target.line === line.number,
          )
          if (hit === undefined) {
            view.dispatch({ effects: setOpen.of(null) })
            return false
          }
          // Prevented: a click in a gutter otherwise moves the editor's selection first, which
          // scrolls the pane out from under the pointer. `changeBars` and `mergeGutter` record
          // the same.
          event.preventDefault()
          // Same marker closes; a different one switches — the change card's rule, and what makes
          // a second click on the row you just opened feel like a toggle rather than a no-op.
          const showing = view.state.field(openField, false)
          const openAtThisLine =
            showing != null && showing.pos === view.state.doc.line(hit.line).from
          view.dispatch({ effects: setOpen.of(openAtThisLine ? null : hit) })
          return true
        },
      },
    }),
    /*
     * Dismiss on a click in the text.
     *
     * `EditorView.domEventHandlers` attaches to `contentDOM`, and neither the gutter nor a
     * tooltip is inside it — so this handler's scope is already exactly the one wanted and the
     * click that opens the menu cannot reach it. `changeBars.ts` carries that argument in full,
     * including which neighbouring fact is *not* true.
     */
    EditorView.domEventHandlers({
      mousedown: (_event, view) => {
        if (view.state.field(openField, false) != null) {
          view.dispatch({ effects: setOpen.of(null) })
        }
        return false
      },
    }),
  ]
}
