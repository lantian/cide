/**
 * Find-in-terminal: the bar Ctrl+F opens over a Claude or shell pane.
 *
 * Asked for as *"Need to add CTRL+F (search bar) for claude and bash panels to be able to search
 * text"*. `@xterm/addon-search` has been a pinned dependency of this app since M0 and was loaded
 * **nowhere** — the primitive was paid for and never wired, which is this project's recurring
 * defect in its cheapest form.
 *
 * # Where the pieces are, and why they are not all here
 *
 * * `terminal/findModel.ts` — the counter's four sentences, the alt-screen note and the field's
 *   keyboard. Import-free, so `ui/scripts/check-terminal-find.mjs` can compile it standalone and
 *   run every branch; this file needs React and a mounted pane and no check script can touch it.
 * * `terminal/findStore.ts` — which panes have a bar up. The chord arrives from outside React
 *   twice over (xterm's key handler, `keys/dispatch.ts`), so it cannot be a prop.
 * * `terminal/xterm.ts` — `ensureSearch` loads the addon on first use, `searchOptions` resolves
 *   what a match is painted with.
 * * `terminal/keys.ts` — whether a keystroke is the chord at all, focus-scoped, beside Ctrl+C
 *   and Ctrl+V and for the same reason.
 *
 * # The counting contract, which is easy to get wrong silently
 *
 * `SearchAddon` reports `{resultIndex, resultCount}` through `onDidChangeResults` **only when
 * the search that produced them carried a `decorations` block** — `ResultTracker.fireResultsChanged`
 * returns immediately otherwise. So every call below passes `searchOptions()`, and a future edit
 * that "turns off the highlighting to make it cheaper" turns off the count with it and leaves
 * this bar showing a stale number for ever. `searchOptions`' own comment says so at the other
 * end.
 *
 * # What happens on the alternate screen
 *
 * The addon searches `buffer.active` and nothing else, and on the alternate buffer that *is* the
 * visible screen — a full-screen TUI has no scrollback by construction. The search still works
 * and still finds what is on screen; what it cannot do is reach the transcript above. The bar
 * says so rather than quietly finding nothing, which is `findScopeNote`'s whole job, and that
 * function's comment records why switching the buffer to get at the scrollback would be worse.
 */
import { useEffect, useRef, useState } from 'react'
import { peekHost } from '@/layout/paneHosts'
import { ensureSearch, searchOptions } from '@/terminal/xterm'
import {
  findCountLabel,
  findFieldKey,
  findScopeNote,
  NO_RESULTS,
  type FindResults,
} from '@/terminal/findModel'
import { closeTerminalFind, useTerminalFind } from '@/terminal/findStore'
import styles from './TerminalFindBar.module.css'

export interface TerminalFindBarProps {
  paneId: string
}

/**
 * The bar, or nothing.
 *
 * Rendered unconditionally by `TerminalPane` and returning `null` while closed, rather than
 * being conditionally mounted by the pane: the hooks below have to run in the same order on
 * every render of the pane, and the store subscription is what tells this component it has been
 * opened at all. (Rule 2 of `layout/paneHosts.ts` — never conditionally render a pane — is about
 * the pane's own slot, not about chrome beside it; the restart bar next door is conditional in
 * the same way.)
 */
export function TerminalFindBar({ paneId }: TerminalFindBarProps): React.ReactElement | null {
  /*
   * How many times this pane has been asked to open its bar. `undefined` is closed.
   *
   * A counter rather than a boolean so a second Ctrl+F re-focuses the field — see
   * `findStore.ts`, which records the bug in `@codemirror/search`'s equivalent that this shape
   * avoids.
   */
  const nonce = useTerminalFind((s) => s.open[paneId])
  const open = nonce !== undefined

  const fieldRef = useRef<HTMLInputElement | null>(null)
  const [query, setQuery] = useState('')
  const [results, setResults] = useState<FindResults>(NO_RESULTS)
  const [alternate, setAlternate] = useState(false)

  /*
   * Which buffer the child is painting into, kept live for as long as the bar is up.
   *
   * Read once on open *and* subscribed, because both directions happen under a search: Claude
   * Code enters its full-screen view when a turn starts and leaves it when the turn ends, and a
   * note that was only correct at the moment the bar opened would be a lie a few seconds later —
   * in whichever direction is worse, since it would claim the scrollback is searchable exactly
   * when it has stopped being.
   */
  useEffect(() => {
    if (!open) return
    const handle = peekHost(paneId)?.terminal
    if (handle === undefined) return
    const term = handle.term
    setAlternate(term.buffer.active.type === 'alternate')
    const sub = term.buffer.onBufferChange((buffer) => setAlternate(buffer.type === 'alternate'))
    return () => sub.dispose()
  }, [open, paneId])

  /*
   * Report every result change the addon publishes, then re-run whatever the field is holding.
   *
   * Subscribed for the life of the open bar rather than read back after each call, because the
   * addon re-runs the last query 200 ms after every write batch (`SearchAddon._updateMatches`)
   * so that the count stays true while the child keeps printing. A number read once at the end
   * of `findNext` would be right for a frame and stale for the rest of the turn.
   *
   * The re-run is not tidiness, and **it has to be inside this effect, after the subscription**.
   * Closing the bar calls `clearDecorations`, which drops the highlights *and* the addon's
   * cached term, while this component stays mounted and keeps its `query` and its `results`. So
   * reopening would otherwise show `3 of 12` beside a field whose query highlights nothing, and
   * the next Enter would jump somewhere the number had already claimed the user was. Running it
   * from a separate effect declared above this one would have been worse than not running it:
   * effects fire in declaration order, so the count it produced would land before anything was
   * listening and the label would stay stale anyway.
   *
   * `incremental` so a match that still matches keeps the selection it had, rather than the bar
   * walking the viewport every time it is reopened.
   */
  useEffect(() => {
    if (!open) return
    const handle = peekHost(paneId)?.terminal
    if (handle === undefined) return
    const search = ensureSearch(handle)
    const sub = search.onDidChangeResults((event) => {
      // Only when the numbers actually move. The addon re-highlights 200 ms after the last
      // write batch for as long as a query is cached, so in a Claude pane that is printing
      // this fires whenever the output pauses — and a fresh object every time would re-render
      // the bar on each one for a label that has not changed.
      setResults((previous) =>
        previous.count === event.resultCount && previous.index === event.resultIndex
          ? previous
          : { count: event.resultCount, index: event.resultIndex },
      )
    })
    // `fieldRef.current?.value` rather than `query`, for the same reason `run` takes its term as
    // an argument: the DOM is committed before an effect runs and is never a render behind.
    const term = fieldRef.current?.value ?? ''
    if (term !== '') search.findNext(term, { ...searchOptions(), incremental: true })
    return () => sub.dispose()
  }, [open, paneId])

  /* Focus and select on every open request, which is what makes the second Ctrl+F a gesture. */
  useEffect(() => {
    if (!open) return
    const field = fieldRef.current
    if (field === null) return
    field.focus()
    // Selected, not just focused: Ctrl+F with a query already in the box has to let the user
    // type over it. This is what `@codemirror/search`'s `selectSearchInput` does for the editor's
    // bar and it is the behaviour every find bar in every editor has.
    field.select()
  }, [open, nonce])

  /*
   * Give the terminal its pixels back when the bar goes.
   *
   * On unmount as well as on close, and the difference matters: a split remounts the surviving
   * leaf, so this component goes away while the *terminal* — which `paneHosts` keeps outside
   * React entirely — carries on with every match still highlighted and no bar left to clear
   * them. `clearDecorations` also drops the addon's cached query, which is what stops its
   * `onWriteParsed` listener re-running a search nobody is watching for the rest of the session.
   */
  useEffect(() => {
    if (!open) return
    return () => {
      const handle = peekHost(paneId)?.terminal
      // The host is gone — the pane was closed rather than merely unmounted — and with it the
      // terminal, its addon and every decoration on it. Nothing to clear, and calling
      // `ensureSearch` here would build an addon for a terminal that no longer exists.
      if (handle === undefined) return
      ensureSearch(handle).clearDecorations()
    }
  }, [open, paneId])

  /* A pane that leaves the tree takes its entry in the store with it. */
  useEffect(() => () => closeTerminalFind(paneId), [paneId])

  if (!open) return null

  /**
   * Run one search. The one place this component talks to the addon.
   *
   * `term` is passed in rather than read from `query`, because the typing path calls this from
   * inside `onChange`, where `query` still holds the *previous* value — `setQuery` schedules a
   * render, it does not write the variable. Searching the stale value is a class of bug that
   * shows up as a find bar one character behind the field, and it is invisible in review.
   *
   * `incremental` is set for typing and for nothing else. It makes `findNext` extend the current
   * match while the term still matches, so typing `err` does not walk the viewport once per
   * character. The addon documents it as affecting `findNext` only, which is why the direction
   * check comes first and `findPrevious` never sees it.
   */
  const run = (direction: 'next' | 'previous', term: string, incremental: boolean): void => {
    const handle = peekHost(paneId)?.terminal
    if (handle === undefined) return
    const search = ensureSearch(handle)

    if (term === '') {
      // `clearDecorations` rather than a search with an empty term. The addon rejects an empty
      // term and clears either way, but only this path also drops its *cached* query — so the
      // 200 ms re-highlight that follows every write batch stops, instead of re-running an
      // invalid search for the rest of the session.
      search.clearDecorations()
      setResults(NO_RESULTS)
      return
    }

    const options = searchOptions()
    if (direction === 'next') search.findNext(term, { ...options, incremental })
    else search.findPrevious(term, options)
  }

  const close = (): void => {
    closeTerminalFind(paneId)
    // The keyboard goes back where it came from. Without this, Escape leaves the caret in a
    // field that is no longer on screen and the next keystroke reaches nothing at all —
    // indistinguishable, from the user's chair, from a pane that has stopped taking input.
    peekHost(paneId)?.terminal?.term.focus()
  }

  const scope = findScopeNote(alternate)
  const label = findCountLabel(query, results)
  const idle = query === ''

  return (
    <div className={styles.bar} data-audit="terminalFind">
      <input
        ref={fieldRef}
        className={styles.field}
        type="text"
        value={query}
        placeholder="Find in terminal"
        spellCheck={false}
        aria-label="Find in terminal"
        onChange={(event) => {
          setQuery(event.target.value)
          run('next', event.target.value, true)
        }}
        onKeyDown={(event) => {
          const action = findFieldKey(event)
          if (action === null) return
          // Both, always. `preventDefault` stops Enter submitting anything the field may one day
          // sit inside; `stopPropagation` keeps Escape from also reaching an overlay or the
          // pane's own listeners, which would close two things for one press.
          event.preventDefault()
          event.stopPropagation()
          if (action === 'close') return close()
          if (action === 'refocus') {
            event.currentTarget.select()
            return
          }
          run(action, query, false)
        }}
        onBlur={() => {
          // The addon's own advice, and it is about what you can *see*: the active-match
          // decoration paints on top of the selection, so leaving it up while the field has lost
          // focus hides the fact that the match is selected and copyable.
          const handle = peekHost(paneId)?.terminal
          if (handle !== undefined) ensureSearch(handle).clearActiveDecoration()
        }}
      />
      <span className={styles.count}>{label}</span>
      {scope !== null && (
        <span
          className={styles.scope}
          title={
            'This program is drawing a full-screen view, which has no scrollback: the search ' +
            'covers the rows on screen. Quit or interrupt it to search the transcript again.'
          }
        >
          {scope}
        </span>
      )}
      <button
        type="button"
        className={styles.button}
        disabled={idle}
        title="Previous match (Shift+Enter)"
        // `mousedown` prevented rather than left alone: a click on a button takes focus off the
        // field, and the next Enter would go nowhere. The editor's find bar does the same thing
        // for the same reason (`editor/find.ts::button`).
        onMouseDown={(event) => event.preventDefault()}
        onClick={() => run('previous', query, false)}
      >
        ↑
      </button>
      <button
        type="button"
        className={styles.button}
        disabled={idle}
        title="Next match (Enter)"
        onMouseDown={(event) => event.preventDefault()}
        onClick={() => run('next', query, false)}
      >
        ↓
      </button>
      <button
        type="button"
        className={styles.button}
        title="Close (Escape)"
        onMouseDown={(event) => event.preventDefault()}
        onClick={close}
      >
        ✕
      </button>
    </div>
  )
}
