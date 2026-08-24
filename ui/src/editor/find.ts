/**
 * Find-in-file.
 *
 * `@codemirror/search` supplies the search itself — the cursor, the highlighting, the
 * next/previous commands — and its stock panel is replaced wholesale. Two reasons, and the
 * second is the one that matters: the default panel is styled by CodeMirror's base theme
 * with literal colours that survive a theme switch, and it has no match counter, which is
 * the one number a user looks for after typing a query.
 *
 * The counter is capped, and it names which match you are on. Both live in `findMatches.ts`,
 * which imports nothing so a check script can run the arithmetic; this file is the DOM.
 *
 * # What replacing the stock panel cost, and what it took to pay it back
 *
 * A custom `Panel` inherits none of `SearchPanel`'s behaviour, and the first version of this
 * file re-implemented about half of it. The two halves it left out were both invisible by
 * reading and both reported as bugs:
 *
 * * **`mount()` did not focus the field.** `openSearchPanel`'s closed-panel branch only
 *   dispatches `togglePanel`; the focusing lives in the panel's own `mount()` upstream. So
 *   Ctrl+F opened the bar and left the caret in the buffer — and the query the user typed next
 *   went *into the file*.
 * * **`onKeyDown` did not bridge to `search-panel` scope.** The panel is a sibling of
 *   `contentDOM` and the editor's keymap is installed on `contentDOM`, so nothing scoped
 *   `search-panel` — F3, Shift+F3, Ctrl+G, Mod-f's re-focus — could ever fire while the caret
 *   was in the find field. Which is exactly where the caret is, because of the first bug.
 *
 * Both are one line each; the comments beside them are longer than the fix because the failure
 * is not visible in the code that has it.
 */
import {
  SearchQuery,
  closeSearchPanel,
  findNext,
  findPrevious,
  getSearchQuery,
  highlightSelectionMatches,
  search,
  setSearchQuery,
} from '@codemirror/search'
import {
  keymap,
  runScopeHandlers,
  type EditorView,
  type Panel,
  type ViewUpdate,
} from '@codemirror/view'
import type { Extension } from '@codemirror/state'
import { searchBindings } from './editorKeys'
import { countLabel, tally, type MatchSpan, type Tally } from './findMatches'
import { iconElement } from '../icons/iconElement'
import type { IconName } from '../icons/iconPaths'

import styles from './EditorSurface.module.css'

/**
 * Every match of `query`, lazily.
 *
 * A generator so `tally` can abandon the walk at its cap without this file knowing what the cap
 * is. The positions are copied out rather than yielded by reference: `SearchCursor.next()`
 * returns *the cursor itself* with `value` overwritten, so yielding `step.value` would hand the
 * consumer one object that changes under it.
 */
function* matchSpans(view: EditorView, query: SearchQuery): Generator<MatchSpan> {
  const cursor = query.getCursor(view.state)
  for (let step = cursor.next(); step.done !== true; step = cursor.next()) {
    yield { from: step.value.from, to: step.value.to }
  }
}

/** How many matches a query has and which one holds the selection. `null` for an invalid one. */
export function countMatches(view: EditorView, query: SearchQuery): Tally | null {
  if (!query.valid) return null
  const { from, to } = view.state.selection.main
  return tally(matchSpans(view, query), { from, to })
}

function button(mark: IconName, title: string, onClick: () => void): HTMLButtonElement {
  const el = document.createElement('button')
  el.type = 'button'
  el.className = styles.findButton ?? ''
  // `iconElement` and not `textContent`: there is no component tree inside a CodeMirror panel,
  // and the alternative is that this bar keeps drawing characters while the rest of the app
  // does not — which is the two-systems state the icon set exists to end.
  el.append(iconElement(mark, 1))
  el.title = title
  // `mousedown` rather than `click`, and prevented: a click on a button steals focus from
  // the input, and the next keystroke would go nowhere.
  el.addEventListener('mousedown', (e) => e.preventDefault())
  el.addEventListener('click', onClick)
  return el
}

class FindPanel implements Panel {
  readonly dom: HTMLElement
  readonly top = true
  private readonly input: HTMLInputElement
  private readonly count: HTMLElement
  private readonly caseToggle: HTMLButtonElement
  private readonly regexpToggle: HTMLButtonElement
  /** `window.setTimeout` handle; 0 when no count is pending. */
  private recountTimer = 0

  constructor(private readonly view: EditorView) {
    const query = getSearchQuery(view.state)

    this.dom = document.createElement('div')
    this.dom.className = styles.findBar ?? ''
    // The panel is a form so Enter submits rather than inserting a newline into the buffer.
    this.dom.addEventListener('keydown', this.onKeyDown)

    this.input = document.createElement('input')
    this.input.className = styles.findInput ?? ''
    this.input.placeholder = 'Find'
    this.input.value = query.search
    this.input.spellcheck = false
    /*
     * The attribute `@codemirror/search` looks for when it needs to find *our* field.
     *
     * It used to say "to decide what to focus on open", and that sentence is why the missing
     * focus in `mount()` survived review: it is only true on the *second* Ctrl+F. `getSearchInput`
     * reads this attribute from two places — `openSearchPanel`'s already-open branch, and
     * `selectSearchInput`, which `findNext`/`findPrevious` call to keep a typed query selected —
     * and neither runs on the path a first Ctrl+F takes. Still needed for both of those, so it
     * stays; it is a plain DOM attribute rather than a class so it survives CSS Modules' hashing.
     */
    this.input.setAttribute('main-field', 'true')
    this.input.addEventListener('input', () => this.commit())

    this.count = document.createElement('span')
    this.count.className = styles.findCount ?? ''

    this.caseToggle = button('case-sensitive', 'Match case', () => {
      this.commit({ caseSensitive: !getSearchQuery(this.view.state).caseSensitive })
    })
    this.regexpToggle = button('regex', 'Regular expression', () => {
      this.commit({ regexp: !getSearchQuery(this.view.state).regexp })
    })

    this.dom.append(
      this.input,
      this.count,
      this.caseToggle,
      this.regexpToggle,
      // The titles name F3, and that is the only documentation these two chords get. They are
      // CodeMirror's bindings, not `cide-core::commands`', so they carry no palette row and no
      // shortcut chip — see the note in `codeMenu.tsx` about why that is deliberate. The bar is
      // therefore the one surface that can tell a user the keys exist.
      button('chevron-up', 'Previous match (Shift+F3, Shift+Enter)', () => findPrevious(this.view)),
      button('chevron-down', 'Next match (F3, Enter)', () => findNext(this.view)),
      button('x', 'Close (Escape)', () => closeSearchPanel(this.view)),
    )
  }

  mount(): void {
    this.sync()
    this.recount()
    /*
     * Take the caret. **The bug this fixes typed the user's query into their file.**
     *
     * `openSearchPanel` has two branches (`@codemirror/search`): with the panel already open it
     * focuses and selects the field, and with it closed — the Ctrl+F path — it only dispatches
     * `togglePanel` and returns. Upstream's `SearchPanel` compensates in its own `mount()`;
     * ours did not, so the bar appeared, focus stayed in the buffer, and the next keystrokes
     * were inserted into the document. Undoable and tab-marking, but a content-modification bug
     * rather than a cosmetic one.
     *
     * Synchronous, with no `requestAnimationFrame` and no parked request. CodeMirror's panel
     * manager inserts the panel DOM and only then walks the newly mounted panels calling
     * `mount()`, so the node is in the document by the time this line runs. Worth contrasting
     * with `sidebar/SearchPanel.tsx`, whose own comment explains at length why *it* needed a
     * parked focus request — the dispatcher there runs a React render before the component
     * exists. That constraint does not apply here, and copying the machinery would be paying for
     * a problem this surface does not have.
     *
     * `select()` as well as `focus()`: the constructor seeds the field from `defaultQuery`,
     * which picks up a short selection, so Ctrl+F on a highlighted word gives the standard
     * pre-filled-and-selected field where typing replaces rather than appends. That seeding was
     * already here and was unreachable without this line.
     */
    this.input.focus()
    this.input.select()
  }

  update(update: ViewUpdate): void {
    // The controls are three attribute writes and are refreshed unconditionally. The count
    // is not: it walks the document, so it runs only when the document or the query moved.
    // An undo is a `docChanged` transaction, so it is covered — that was the case worth
    // checking, since a stale count after an undo is exactly when a number misleads.
    this.sync()
    /*
     * …and when the search *moved the selection*, which is what the ordinal follows.
     *
     * Gated on `select.search` — the `userEvent` `findNext`/`findPrevious` stamp on their own
     * dispatch — and deliberately **not** on bare `update.selectionSet`. A held arrow key changes
     * the selection thirty times a second, and recounting on each would put the document walk
     * that `scheduleRecount` exists to coalesce back onto the keystroke path.
     *
     * The consequence, stated rather than hidden: arrowing away from a match leaves `3 of 12` on
     * screen until something else recounts. That is deliberate and it is what VS Code does — the
     * number answers *which match you were taken to*, not where the caret is now, and a counter
     * that blanked itself the moment you started reading the hit would be answering a question
     * nobody asked.
     *
     * A held F3 does reach here, and the 120 ms debounce below is the answer: the walks coalesce
     * and the number settles on the match the user stopped at.
     */
    if (
      update.docChanged ||
      update.transactions.some(
        (tr) => tr.isUserEvent('select.search') || tr.effects.some((e) => e.is(setSearchQuery)),
      )
    ) {
      this.scheduleRecount()
    }
  }

  destroy(): void {
    if (this.recountTimer !== 0) clearTimeout(this.recountTimer)
    this.dom.removeEventListener('keydown', this.onKeyDown)
  }

  /**
   * Coalesce counting onto a short timer.
   *
   * `SearchCursor` has no budget, so counting a query with few matches walks the whole
   * document — on the 5 MB file the milestone tests with, that is hundreds of milliseconds.
   * Typing into the find field dispatches a query per keystroke, so without this the panel
   * would run one full scan per character. The counter lagging the field by a frame or two
   * is the right trade; the buffer never lags.
   */
  private scheduleRecount(): void {
    if (this.recountTimer !== 0) clearTimeout(this.recountTimer)
    this.recountTimer = window.setTimeout(() => {
      this.recountTimer = 0
      this.recount()
    }, 120)
  }

  /** Push the panel's controls into the editor's search state. */
  private commit(overrides: { caseSensitive?: boolean; regexp?: boolean } = {}): void {
    const current = getSearchQuery(this.view.state)
    const query = new SearchQuery({
      search: this.input.value,
      caseSensitive: overrides.caseSensitive ?? current.caseSensitive,
      regexp: overrides.regexp ?? current.regexp,
      wholeWord: current.wholeWord,
      replace: current.replace,
    })
    this.view.dispatch({ effects: setSearchQuery.of(query) })
  }

  /** The parts that cost nothing: the field's contents and the two toggles. */
  private sync(): void {
    const query = getSearchQuery(this.view.state)
    if (this.input.value !== query.search) this.input.value = query.search
    this.caseToggle.dataset.on = query.caseSensitive ? 'true' : 'false'
    this.regexpToggle.dataset.on = query.regexp ? 'true' : 'false'
    // An invalid regexp is a state the user passes through on the way to a valid one, so it
    // marks the field rather than clearing it or reporting zero matches.
    this.input.dataset.invalid = query.search.length > 0 && !query.valid ? 'true' : 'false'
  }

  private recount(): void {
    const query = getSearchQuery(this.view.state)
    if (query.search.length === 0) {
      this.count.textContent = ''
      return
    }
    const counted = countMatches(this.view, query)
    // An invalid regexp is a state the user passes through on the way to a valid one, which is
    // why it is a phrase rather than a zero: `0 matches` is a claim about the document.
    this.count.textContent = counted === null ? 'bad pattern' : countLabel(counted)
  }

  private readonly onKeyDown = (event: KeyboardEvent): void => {
    /*
     * The bridge CodeMirror's own panel has and this one did not.
     *
     * The panel's DOM is a *sibling* of `contentDOM`, and the editor's keymap is installed with
     * `EditorView.domEventHandlers` on `contentDOM` — so an event inside this box never passes
     * through the keymap at all. Upstream's `SearchPanel.keydown` bridges the gap by running the
     * `search-panel` scope by hand, and this line is that same bridge. Without it every binding
     * scoped `search-panel` is dead the moment the caret is in this field: F3, Shift+F3, Ctrl+G,
     * and Mod-f's re-focus-and-select.
     *
     * That is precisely where the caret is when a user reaches for F3, so this and the focus call
     * in `mount()` are two halves of one report rather than two bugs.
     *
     * Order matters and matches upstream: scope handlers first, then Enter. `searchKeymap` binds
     * Escape in this scope too, so the branch below is now belt and braces — kept because it
     * costs a line and because closing the bar is the one gesture that must never stop working.
     */
    if (runScopeHandlers(this.view, event, 'search-panel')) {
      event.preventDefault()
      return
    }
    if (event.key === 'Enter') {
      event.preventDefault()
      if (event.shiftKey) findPrevious(this.view)
      else findNext(this.view)
    } else if (event.key === 'Escape') {
      event.preventDefault()
      closeSearchPanel(this.view)
    }
  }
}

/**
 * Search, its keymap, and the match highlighting.
 *
 * **The bindings themselves live in `editorKeys.ts`, and the argument for each one lives there
 * with them.** This file imports `./EditorSurface.module.css`, so a check script cannot
 * `require` it — and a keymap nothing can load is a keymap whose chords have never been
 * resolved, only regexed. `searchBindings` is `searchKeymap` with `gotoLine` filtered out (cide
 * owns Go to line) and `Mod-d` re-homed to `Alt-j` (duplicate-line took Ctrl+D). Both edits are
 * by command identity rather than by key string; the reasoning is four paragraphs and it is over
 * there rather than summarised here.
 */
export function findExtensions(): Extension {
  return [
    search({ top: true, createPanel: (view) => new FindPanel(view) }),
    highlightSelectionMatches(),
    keymap.of(searchBindings),
  ]
}
