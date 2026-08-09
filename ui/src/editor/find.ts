/**
 * Find-in-file.
 *
 * `@codemirror/search` supplies the search itself — the cursor, the highlighting, the
 * next/previous commands — and its stock panel is replaced wholesale. Two reasons, and the
 * second is the one that matters: the default panel is styled by CodeMirror's base theme
 * with literal colours that survive a theme switch, and it has no match counter, which is
 * the one number a user looks for after typing a query.
 *
 * The counter is capped. Counting matches means walking the document, and on a 5 MB file a
 * one-character query matches often enough that an exact figure costs more than it tells
 * anyone; past the cap the panel says `999+` and stops walking.
 */
import {
  SearchQuery,
  closeSearchPanel,
  findNext,
  findPrevious,
  getSearchQuery,
  highlightSelectionMatches,
  search,
  searchKeymap,
  setSearchQuery,
} from '@codemirror/search'
import { keymap, type EditorView, type Panel, type ViewUpdate } from '@codemirror/view'
import type { Extension } from '@codemirror/state'
import styles from './EditorSurface.module.css'

/** Past this many matches the panel stops counting and says so. */
const COUNT_CAP = 999

/** How many matches a query has, capped. Returns `null` for an empty or invalid query. */
export function countMatches(view: EditorView, query: SearchQuery): number | null {
  if (!query.valid) return null
  const cursor = query.getCursor(view.state)
  let n = 0
  for (let step = cursor.next(); step.done !== true; step = cursor.next()) {
    n++
    if (n > COUNT_CAP) return COUNT_CAP + 1
  }
  return n
}

function button(label: string, title: string, onClick: () => void): HTMLButtonElement {
  const el = document.createElement('button')
  el.type = 'button'
  el.className = styles.findButton ?? ''
  el.textContent = label
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
    // The attribute `@codemirror/search` looks for to decide what to focus on open. It is a
    // plain DOM attribute rather than a class, so it survives CSS Modules' hashing.
    this.input.setAttribute('main-field', 'true')
    this.input.addEventListener('input', () => this.commit())

    this.count = document.createElement('span')
    this.count.className = styles.findCount ?? ''

    this.caseToggle = button('Aa', 'Match case', () => {
      this.commit({ caseSensitive: !getSearchQuery(this.view.state).caseSensitive })
    })
    this.regexpToggle = button('.*', 'Regular expression', () => {
      this.commit({ regexp: !getSearchQuery(this.view.state).regexp })
    })

    this.dom.append(
      this.input,
      this.count,
      this.caseToggle,
      this.regexpToggle,
      button('↑', 'Previous match (Shift+Enter)', () => findPrevious(this.view)),
      button('↓', 'Next match (Enter)', () => findNext(this.view)),
      button('×', 'Close (Escape)', () => closeSearchPanel(this.view)),
    )
  }

  mount(): void {
    this.sync()
    this.recount()
  }

  update(update: ViewUpdate): void {
    // The controls are three attribute writes and are refreshed unconditionally. The count
    // is not: it walks the document, so it runs only when the document or the query moved.
    // An undo is a `docChanged` transaction, so it is covered — that was the case worth
    // checking, since a stale count after an undo is exactly when a number misleads.
    this.sync()
    if (update.docChanged || update.transactions.some((tr) => tr.effects.some((e) => e.is(setSearchQuery)))) {
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
    const total = countMatches(this.view, query)
    if (total === null) {
      this.count.textContent = 'bad pattern'
    } else if (total > COUNT_CAP) {
      this.count.textContent = `${COUNT_CAP}+`
    } else {
      this.count.textContent = total === 1 ? '1 match' : `${total} matches`
    }
  }

  private readonly onKeyDown = (event: KeyboardEvent): void => {
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
 * `searchKeymap` is included whole rather than picked over. It carries Ctrl+F, Ctrl+G,
 * Ctrl+D and the replace bindings; the replace ones do nothing without a replace field in
 * the panel, which is a missing feature rather than a wrong one, and dropping them from the
 * keymap would make adding the field later a two-file change.
 */
export function findExtensions(): Extension {
  return [
    search({ top: true, createPanel: (view) => new FindPanel(view) }),
    highlightSelectionMatches(),
    keymap.of(searchKeymap),
  ]
}
