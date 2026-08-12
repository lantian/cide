/**
 * The status bar's branch control, and the popup behind it.
 *
 * > *"need a branch selector in bottom line with features like in idea. Currently it shows
 * > nothing and has no actions on click."*
 *
 * # Why it showed nothing
 *
 * Nothing was wrong with the backend. `git_branch_info` was registered, handled and correct;
 * `git.branchInfo` existed in `ipc/client.ts` — **and had no caller anywhere in the app**.
 * `chrome/StatusBar.tsx` takes the branch as a prop that defaults to `'—'`, and `App.tsx`
 * renders `<StatusBar claude={…} />` and passes nothing else. So the slot painted its
 * placeholder for ever, and clicking it did nothing because it was a `<span>`. One more
 * control wired to a command that already worked.
 *
 * # The two mount points, and why the popup is not inside the widget
 *
 * [`BranchSelector`] is the 24px label. It is the whole of what the status bar needs — one
 * line — and it owns no popup: clicking it calls `showOverlay('branches')`.
 *
 * [`BranchPopup`] is rendered by `overlays/OverlayHost`, like the file picker and the command
 * palette. That is what makes the feature reachable **without** a status bar edit at all: the
 * palette's *Switch branch…* opens the same overlay, so every action here has a keyboard route
 * that does not depend on a component this round does not own. It also means there is exactly
 * one popup instance in the window rather than one per mount site, so `Escape`, the scrim and
 * the `overlayOpen` key-context flag all behave the way they do for every other overlay.
 *
 * # The state is a module store, not props
 *
 * The palette dispatches from outside React (see `keys/dispatch.ts`), so the list has to be
 * reachable with `getState()`. The widget subscribes for its label, refreshes on
 * `cide://git-status` — which every mutation in `cmd/git.rs` broadcasts, including the ones
 * this file makes — and the popup reads the same store.
 *
 * Every decision about *which* rows appear and *what a refusal says* is in `branchModel.ts`,
 * which has no React in it and is driven directly by `ui/scripts/check-branches.mjs`.
 */
import { useEffect, useRef, useState } from 'react'
import { create } from 'zustand'
import {
  branch as branchApi,
  events,
  git as gitApi,
  pendingCommand,
  type BranchList,
  type BranchRef,
  type CheckoutMode,
  type ProjectId,
  type RepoId,
} from '@/ipc/client'
import { activeProjectIdOf } from '@/keys/target'
import { showOverlay } from '@/overlays/store'
import { useWorkspace } from '@/store/workspace'
import {
  LOADING,
  actionsFor,
  canPull,
  canPush,
  checkoutNote,
  explain,
  fetchNote,
  headLabel,
  headTitle,
  kindOf,
  listFor,
  primaryRepo,
  refusalOf,
  visibleBranches,
  type Refusal,
} from './branchModel'
import styles from './BranchSelector.module.css'

// --- the store --------------------------------------------------------------------------

interface BranchStore {
  project: ProjectId | null
  lists: BranchList[]
  /** False until the first answer lands, so the label can say `…` rather than `no repo`. */
  loaded: boolean
  /** Which repository the popup is browsing. `null` means the primary one. */
  repo: RepoId | null
  /** The last thing an action had to say. Cleared by the next one. */
  note: string | null
  busy: boolean
  /**
   * Which panel the popup should open on.
   *
   * Lives in the store rather than in a prop because the palette opens this popup from outside
   * React — `git.branch.new` has to be able to say "open, and start on the name field" with
   * nothing to pass it through. Read once on mount and reset, so reopening from the widget
   * lands on the list.
   */
  intent: 'list' | 'new'

  /**
   * Re-read the lists. `force` skips the in-flight coalescing.
   *
   * A mutation must always force: a walk that started a moment before the checkout landed is
   * a snapshot of the old refs, and joining it would leave the popup showing the branch the
   * user just left as current.
   */
  load: (project: ProjectId | null, force?: boolean) => Promise<void>
  pick: (repo: RepoId) => void
  say: (note: string | null) => void
  /** Run a mutation, put its message in `note`, and reload. Never throws. */
  run: (work: () => Promise<string>) => Promise<void>
}

/**
 * The walk currently in flight, keyed by the project it is for.
 *
 * The widget and the popup both drive [`useBranchData`], so with both mounted every project
 * change and every `cide://git-status` would fire two identical `git_branch_list` calls —
 * and after a checkout, which broadcasts, four. Sharing the promise makes the second caller
 * await the first rather than start a second walk of every repository in the project.
 *
 * Not a cache: it is cleared the moment the walk settles, so the next trigger really does
 * re-read. A stale branch list is the bug this whole component exists to end.
 */
let inFlight: { project: ProjectId; walk: Promise<void> } | null = null

export const useBranches = create<BranchStore>((set, get) => ({
  project: null,
  lists: [],
  loaded: false,
  repo: null,
  note: null,
  busy: false,
  intent: 'list',

  async load(project, force = false) {
    // Switching projects clears the rows first: the previous project's branch names over the
    // new project's repository is a wrong answer, and an empty list is a true one.
    if (project !== get().project) set({ project, lists: [], loaded: false, repo: null, note: null })
    if (project === null) {
      set({ loaded: true })
      return
    }
    if (!force && inFlight !== null && inFlight.project === project) return inFlight.walk

    const walk = (async () => {
      // Degrades rather than throws: a build without `git_branch_list` should show `no repo`,
      // not an unhandled rejection out of an effect. Mutations do the opposite on purpose —
      // their rejections are the answer the user needs.
      const lists = await pendingCommand('git_branch_list', () => branchApi.list(project), [])
      // The project may have changed while the walk ran.
      if (get().project === project) set({ lists, loaded: true })
    })()
    inFlight = { project, walk }
    try {
      await walk
    } finally {
      if (inFlight?.walk === walk) inFlight = null
    }
  },

  pick(repo) {
    set({ repo, note: null })
  },

  say(note) {
    set({ note })
  },

  async run(work) {
    set({ busy: true, note: null })
    try {
      const note = await work()
      set({ note: note === '' ? null : note })
    } catch (error) {
      // `explain` turns the tagged wire error into a sentence. `String(error)` would print
      // `[object Object]`, which is how an action ends up looking like it did nothing.
      set({ note: explain(error) })
    } finally {
      set({ busy: false })
      await get().load(get().project, true)
    }
  },
}))

/**
 * Open the branch popup from outside React — the palette's `git.branch.switch` and
 * `git.branch.new`, and the status bar widget's click.
 *
 * `showOverlay`, not `toggle`: a caller that names what it wants means it, and the palette has
 * already closed itself by the time this runs.
 */
export function openBranchPopup(intent: 'list' | 'new' = 'list'): void {
  useBranches.setState({ intent, note: null })
  showOverlay('branches')
}

/**
 * Keep the store pointed at the window's project and fresh against git.
 *
 * Both surfaces call it: the widget so its label is right without anything being open, and the
 * popup so it works in a window whose status bar does not mount the widget.
 */
function useBranchData(): { project: ProjectId | null; lists: BranchList[]; loaded: boolean } {
  const boot = useWorkspace((s) => s.boot)
  const project = activeProjectIdOf(boot)
  const lists = useBranches((s) => s.lists)
  const loaded = useBranches((s) => s.loaded)

  useEffect(() => {
    void useBranches.getState().load(project)
  }, [project])

  // Every mutation in `cmd/git.rs` broadcasts this, and so does the watcher's view of `HEAD`
  // and the refs — which is how a `git checkout` in a bash pane moves the label.
  useEffect(() => {
    const stop = events.onGitStatus((changed: ProjectId) => {
      if (changed === useBranches.getState().project) void useBranches.getState().load(changed)
    })
    return () => void stop.then((off: () => void) => off())
  }, [])

  return { project, lists, loaded }
}

// --- the status bar's widget ---------------------------------------------------------------

/**
 * The 24px control. **One line in `chrome/StatusBar.tsx` replaces its `.branch` span.**
 *
 * A `<button>`, not a span with an onClick: it is reachable by Tab, it answers Space and
 * Enter, and a screen reader calls it a button — none of which a clickable span does.
 */
export function BranchSelector() {
  const { lists, loaded } = useBranchData()
  const primary = primaryRepo(lists)
  const head = primary?.head ?? null

  const label = !loaded ? LOADING : headLabel(head)
  return (
    <button
      type="button"
      className={styles.widget}
      data-audit="branchSelector"
      title={loaded ? headTitle(head, lists.length) : 'Reading branches…'}
      // Nothing to show and nothing to do. Disabled rather than hidden: the slot keeps its
      // place in the bar, so the readouts beside it do not jump when a project opens.
      disabled={loaded && head === null}
      // Wrapped rather than passed by reference: React hands the click event to the handler,
      // and `openBranchPopup`'s first parameter is the intent — a bare reference would open
      // the popup on a `MouseEvent` where `'list' | 'new'` was expected.
      onClick={() => openBranchPopup('list')}
    >
      ⑂ {label}
    </button>
  )
}

// --- the popup -------------------------------------------------------------------------------

/** Which panel the popup is showing. The list is the resting state; the rest are answers. */
type Mode =
  | { kind: 'list' }
  | { kind: 'new'; from: string | null }
  | { kind: 'rename'; from: string }
  | { kind: 'delete'; name: string; force: boolean }
  /** A checkout Rust refused, with the files it named. The reason this feature exists. */
  | { kind: 'refusal'; refusal: Refusal }

export interface BranchPopupProps {
  onDismiss: () => void
}

export function BranchPopup({ onDismiss }: BranchPopupProps) {
  const { project, lists, loaded } = useBranchData()
  const chosen = useBranches((s) => s.repo)
  const note = useBranches((s) => s.note)
  const busy = useBranches((s) => s.busy)

  const [query, setQuery] = useState('')
  // Read once, from the store, so `git.branch.new` in the palette lands on the name field.
  // The lazy initialiser is what keeps it a *starting* mode rather than one that snaps back
  // every time the store notifies.
  const [mode, setMode] = useState<Mode>(() =>
    useBranches.getState().intent === 'new' ? { kind: 'new', from: null } : { kind: 'list' },
  )
  const [openRow, setOpenRow] = useState<string | null>(null)
  const search = useRef<HTMLInputElement>(null)

  const list = listFor(lists, chosen)
  const rows = visibleBranches(list, query)
  const repo = list?.repo.id ?? null

  useEffect(() => {
    search.current?.focus()
    // Consumed: reopening from the widget starts on the list again.
    useBranches.setState({ intent: 'list' })
  }, [])

  /*
   * Escape backs out one level rather than closing outright when a panel is open: the panels
   * are answers to a question the list asked, and dismissing the whole popup would throw away
   * the refusal — the list of files the user has to decide about — with no way back to it.
   */
  const back = () => {
    if (mode.kind === 'list') onDismiss()
    else setMode({ kind: 'list' })
  }

  /**
   * Run a mutation against the chosen repository, and let the store own the sentence it
   * produces.
   *
   * Takes the two ids as arguments rather than closing over the nullable ones, so every call
   * site is a `null` check the compiler can see. Casting at the call sites — which is what a
   * `disabled` prop tempts you into — makes the one path where the project closes mid-click
   * into a runtime `undefined` inside an `invoke`.
   */
  const act = (work: (project: ProjectId, repo: RepoId) => Promise<string>) => {
    if (project === null || repo === null) return
    setMode({ kind: 'list' })
    setOpenRow(null)
    void useBranches.getState().run(() => work(project, repo))
  }

  /*
   * The refusal cannot travel through `store.run`, which turns every rejection into a
   * sentence. This one is not a sentence — it is a list of files plus a choice — so the
   * checkout is called directly here and only the *other* failures fall back to `explain`.
   */
  const tryCheckout = (name: string, how: CheckoutMode = 'refuse') => {
    if (project === null || repo === null || busy) return
    setOpenRow(null)
    // `busy` is set here and not left to `store.run`, and it is load-bearing rather than
    // cosmetic: the two stash routes are the one gesture in this popup that a double-click
    // could run twice, and running *Bring changes along* twice makes a second stash entry out
    // of a working tree the first one already emptied.
    useBranches.setState({ busy: true, note: null })
    void (async () => {
      try {
        const outcome = await branchApi.checkout(project, repo, name, how)
        setMode({ kind: 'list' })
        // A clean switch says nothing — see `checkoutNote`. `null`, not `''`: an empty string
        // would draw the note bar with nothing in it.
        const said = checkoutNote(outcome)
        useBranches.getState().say(said === '' ? null : said)
      } catch (error) {
        const refusal = refusalOf(error)
        if (refusal !== null) setMode({ kind: 'refusal', refusal })
        else useBranches.getState().say(explain(error))
      } finally {
        useBranches.setState({ busy: false })
        await useBranches.getState().load(project, true)
      }
    })()
  }

  return (
    <div
      className={styles.scrim}
      onMouseDown={onDismiss}
      onKeyDown={(e) => {
        if (e.key === 'Escape') {
          e.stopPropagation()
          back()
        }
      }}
    >
      {/* The click that opens a row menu must not reach the scrim and close the popup. */}
      <div
        className={styles.popup}
        role="dialog"
        aria-label="Branches"
        data-audit="branchPopup"
        onMouseDown={(e) => e.stopPropagation()}
      >
        {/*
         * The repository strip, only when there is more than one. In a superproject "the
         * branch" is genuinely ambiguous and a popup that silently picked one would act on a
         * repository the user never named.
         */}
        {lists.length > 1 && (
          <div className={styles.repos}>
            {lists.map((entry) => (
              <button
                key={entry.repo.id}
                type="button"
                className={entry.repo.id === repo ? styles.repoOn : styles.repo}
                onClick={() => useBranches.getState().pick(entry.repo.id)}
              >
                {entry.repo.name}
                <span className={styles.repoHead}>{entry.head.head}</span>
              </button>
            ))}
          </div>
        )}

        {mode.kind === 'list' && (
          <>
            <div className={styles.head}>
              <input
                ref={search}
                className={styles.search}
                placeholder="Search branches"
                value={query}
                spellCheck={false}
                onChange={(e) => setQuery(e.target.value)}
                onKeyDown={(e) => {
                  // ⏎ on a filtered list checks out the only remaining row. With more than
                  // one it does nothing rather than guessing — a checkout is not a gesture to
                  // resolve an ambiguity with.
                  if (e.key === 'Enter' && rows.length === 1 && rows[0] !== undefined) {
                    tryCheckout(rows[0].name)
                  }
                }}
              />
              <div className={styles.actions}>
                <button
                  type="button"
                  className={styles.action}
                  disabled={busy}
                  onClick={() => setMode({ kind: 'new', from: null })}
                >
                  New branch…
                </button>
                <button
                  type="button"
                  className={styles.action}
                  disabled={busy || project === null || repo === null}
                  onClick={() =>
                    act(async (p, r) => fetchNote(await branchApi.fetch(p, r)))
                  }
                >
                  Fetch
                </button>
                <button
                  type="button"
                  className={styles.action}
                  // Without an upstream this only ever fails, so it is not offered.
                  disabled={busy || !canPull(list?.head ?? null)}
                  onClick={() => act(async (p, r) => fetchNote(await branchApi.pull(p, r)))}
                  title="Fetch and fast-forward. cide never merges for you."
                >
                  Pull
                </button>
                <button
                  type="button"
                  className={styles.action}
                  disabled={busy || !canPush(list?.head ?? null)}
                  onClick={() =>
                    act(async (p, r) => {
                      // A branch with no upstream needs one, and this is the gesture that
                      // means "publish this branch" — the same thing `git push -u` does.
                      const publish = (list?.head.upstream ?? null) === null
                      const out = await gitApi.push(p, r, null, null, publish)
                      const said = out.output.trim()
                      return said === '' ? `Pushed to ${out.remote}` : said
                    })
                  }
                >
                  Push
                </button>
              </div>
            </div>

            <div className={styles.rows} role="listbox" aria-label="Branches">
              {!loaded && <div className={styles.empty}>Reading branches…</div>}
              {loaded && rows.length === 0 && (
                <div className={styles.empty}>
                  {query.trim() === '' ? 'No branches yet' : `No branch matches “${query.trim()}”`}
                </div>
              )}
              {rows.map((entry, index) => (
                <Row
                  key={`${entry.remote ? 'r' : 'l'}:${entry.name}`}
                  entry={entry}
                  // The section headings are drawn from the transition rather than by
                  // splitting the array: one flat list is one keyboard sequence, and the
                  // heading is a property of the boundary.
                  heading={sectionOf(entry, rows[index - 1])}
                  open={openRow === entry.name}
                  busy={busy}
                  onToggleMenu={() => setOpenRow(openRow === entry.name ? null : entry.name)}
                  onCheckout={() => tryCheckout(entry.name)}
                  onNewFrom={() => setMode({ kind: 'new', from: entry.name })}
                  onRename={() => setMode({ kind: 'rename', from: entry.name })}
                  onDelete={() => setMode({ kind: 'delete', name: entry.name, force: false })}
                />
              ))}
            </div>
          </>
        )}

        {mode.kind === 'new' && (
          <NameForm
            title={mode.from === null ? 'New branch from HEAD' : `New branch from ${mode.from}`}
            submit="Create and switch"
            secondary="Create only"
            busy={busy}
            onCancel={back}
            onSubmit={(name, alsoSwitch) =>
              act(async (p, r) => {
                await branchApi.create(p, r, name, mode.from, alsoSwitch)
                return alsoSwitch ? `Switched to ${name}` : `Created ${name}`
              })
            }
          />
        )}

        {mode.kind === 'rename' && (
          <NameForm
            title={`Rename ${mode.from}`}
            submit="Rename"
            initial={mode.from}
            busy={busy}
            onCancel={back}
            onSubmit={(name) =>
              act(async (p, r) => {
                await branchApi.rename(p, r, mode.from, name)
                return `Renamed to ${name}`
              })
            }
          />
        )}

        {mode.kind === 'delete' && (
          <div className={styles.panel}>
            <p className={styles.panelTitle}>Delete {mode.name}?</p>
            <p className={styles.panelBody}>
              {mode.force
                ? `${mode.name} has commits that are on no other branch. Deleting it loses them.`
                : 'Deleting a branch removes the name. The commits stay until git collects them.'}
            </p>
            <div className={styles.panelButtons}>
              <button type="button" className={styles.action} onClick={back}>
                Cancel
              </button>
              <button
                type="button"
                className={mode.force ? styles.danger : styles.primary}
                disabled={busy}
                onClick={() => {
                  if (project === null || repo === null) return
                  // Not `act`: a `branchNotMerged` refusal has to come back into *this* panel
                  // as the harder question, not out as a one-line note the user has to
                  // re-derive a gesture from.
                  useBranches.getState().say(null)
                  void (async () => {
                    try {
                      await branchApi.delete(project, repo, mode.name, mode.force)
                      setMode({ kind: 'list' })
                      useBranches.getState().say(`Deleted ${mode.name}`)
                    } catch (error) {
                      if (kindOf(error) === 'branchNotMerged' && !mode.force) {
                        setMode({ ...mode, force: true })
                      } else {
                        setMode({ kind: 'list' })
                        useBranches.getState().say(explain(error))
                      }
                    } finally {
                      await useBranches.getState().load(project, true)
                    }
                  })()
                }}
              >
                {mode.force ? 'Delete anyway' : 'Delete'}
              </button>
            </div>
          </div>
        )}

        {mode.kind === 'refusal' && (
          <div className={styles.panel}>
            <p className={styles.panelTitle}>
              Switching to {mode.refusal.branch} would overwrite{' '}
              {mode.refusal.paths.length === 1 ? '1 local change' : `${mode.refusal.paths.length} local changes`}
            </p>
            {/*
             * Named, never counted. "3 files" is exactly the thing the user cannot check, and
             * the whole reason `GitError::CheckoutWouldOverwrite` carries the list.
             */}
            <ul className={styles.paths}>
              {mode.refusal.paths.map((path) => (
                <li key={path}>{path}</li>
              ))}
            </ul>
            <p className={styles.panelBody}>
              Files that are the same on both branches come along by themselves — these are the
              ones that differ. There is no “discard and switch”: use the git panel’s rollback
              if you mean to throw the work away.
            </p>
            <div className={styles.panelButtons}>
              <button type="button" className={styles.action} onClick={back}>
                Cancel
              </button>
              <button
                type="button"
                className={styles.action}
                disabled={busy}
                onClick={() => tryCheckout(mode.refusal.branch, 'stash')}
                title="git stash -u, then switch. The changes stay in the stash."
              >
                Stash and switch
              </button>
              <button
                type="button"
                className={styles.primary}
                disabled={busy}
                onClick={() => tryCheckout(mode.refusal.branch, 'stashAndRestore')}
                title="Stash, switch, and put the changes back on the new branch."
              >
                Bring changes along
              </button>
            </div>
          </div>
        )}

        {/*
         * One line, and it is the only place an outcome or a failure appears. Kept below the
         * body rather than replacing it so the list is still there to act on afterwards.
         */}
        {note !== null && (
          <div className={styles.note} data-audit="branchNote">
            {note}
          </div>
        )}
      </div>
    </div>
  )
}

/** `Local` / `Remote`, on the row where the section changes. */
function sectionOf(entry: BranchRef, previous: BranchRef | undefined): string | null {
  if (previous === undefined) return entry.remote ? 'Remote' : 'Local'
  if (previous.remote === entry.remote) return null
  return entry.remote ? 'Remote' : 'Local'
}

interface RowProps {
  entry: BranchRef
  heading: string | null
  open: boolean
  busy: boolean
  onToggleMenu: () => void
  onCheckout: () => void
  onNewFrom: () => void
  onRename: () => void
  onDelete: () => void
}

function Row({ entry, heading, open, busy, ...on }: RowProps) {
  const actions = actionsFor(entry)
  const ahead = entry.ahead > 0 ? `↑${entry.ahead}` : ''
  const behind = entry.behind > 0 ? `↓${entry.behind}` : ''

  return (
    <>
      {heading !== null && <div className={styles.section}>{heading}</div>}
      <div className={entry.current ? styles.rowCurrent : styles.row}>
        <button
          type="button"
          className={styles.rowMain}
          role="option"
          aria-selected={entry.current}
          // The current branch cannot be checked out again — the whole row is inert rather
          // than clickable-and-silent.
          disabled={busy || entry.current}
          onClick={on.onCheckout}
          title={entry.current ? 'You are on this branch' : `Switch to ${entry.name}`}
        >
          <span className={styles.name}>{entry.name}</span>
          <span className={styles.counts}>
            {ahead}
            {behind}
          </span>
          <span className={styles.subject}>{entry.subject}</span>
        </button>
        <button
          type="button"
          className={styles.more}
          aria-label={`Actions for ${entry.name}`}
          aria-expanded={open}
          onClick={on.onToggleMenu}
        >
          ⋯
        </button>
      </div>
      {open && (
        <div className={styles.rowMenu}>
          {actions.includes('checkout') && (
            <button type="button" className={styles.menuItem} onClick={on.onCheckout}>
              Checkout
            </button>
          )}
          {actions.includes('newFrom') && (
            <button type="button" className={styles.menuItem} onClick={on.onNewFrom}>
              New branch from here
            </button>
          )}
          {actions.includes('rename') && (
            <button type="button" className={styles.menuItem} onClick={on.onRename}>
              Rename
            </button>
          )}
          {actions.includes('delete') && (
            <button type="button" className={styles.menuItem} onClick={on.onDelete}>
              Delete
            </button>
          )}
        </div>
      )}
    </>
  )
}

interface NameFormProps {
  title: string
  submit: string
  secondary?: string | undefined
  initial?: string | undefined
  busy: boolean
  onCancel: () => void
  onSubmit: (name: string, alsoSwitch: boolean) => void
}

/**
 * The one text field this popup has, used by *New branch* and *Rename*.
 *
 * The name is **not** validated here beyond "not empty". `git check-ref-format`'s rules are
 * long and Rust already refuses with `invalidBranchName`, which `explain` turns into a
 * sentence naming what is wrong — a second, approximate copy of those rules in TypeScript is
 * how a legal name ends up rejected by the UI that could have created it.
 */
function NameForm({ title, submit, secondary, initial, busy, onCancel, onSubmit }: NameFormProps) {
  const [name, setName] = useState(initial ?? '')
  const field = useRef<HTMLInputElement>(null)

  useEffect(() => {
    field.current?.focus()
    field.current?.select()
  }, [])

  const ready = name.trim() !== '' && !busy
  return (
    <div className={styles.panel}>
      <p className={styles.panelTitle}>{title}</p>
      <input
        ref={field}
        className={styles.search}
        value={name}
        spellCheck={false}
        placeholder="Branch name"
        onChange={(e) => setName(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && ready) onSubmit(name.trim(), secondary !== undefined)
        }}
      />
      <div className={styles.panelButtons}>
        <button type="button" className={styles.action} onClick={onCancel}>
          Cancel
        </button>
        {secondary !== undefined && (
          <button
            type="button"
            className={styles.action}
            disabled={!ready}
            onClick={() => onSubmit(name.trim(), false)}
          >
            {secondary}
          </button>
        )}
        <button
          type="button"
          className={styles.primary}
          disabled={!ready}
          onClick={() => onSubmit(name.trim(), secondary !== undefined)}
        >
          {submit}
        </button>
      </div>
    </div>
  )
}
