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
import { Button, IconButton } from '@/kit/components/Button'
import { useEffect, useLayoutEffect, useRef, useState } from 'react'

import { notify } from '@/chrome/notices'
import { openPushDialog } from '@/chrome/pushRun'
import { trackGitOp } from '@/chrome/gitOpStore'
import { create } from 'zustand'
import {
  branch as branchApi,
  events,
  pendingCommand,
  type BranchInfo,
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
  mergeConfirm,
  mergeNote,
  primaryRepo,
  refusalOf,
  visibleBranches,
  alreadyOn,
  type BranchFocus,
  clampFocus,
  enterAction,
  FILTER,
  navigate,
  type GitOp,
  type Refusal,
} from './branchModel'
import { showConflicts } from './conflictsStore'
import { divergenceOf, strategyAsk } from './pullStrategyModel'
import { requestPullStrategy } from './pullStrategyStore'
import { Icon } from '@/icons/Icon'

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
  /**
   * Run a mutation, put its message in `note`, and reload. Never throws.
   *
   * `op` is which gesture the failure belongs to, and it exists for exactly one variant:
   * `CheckoutWouldOverwrite` is raised by *Pull* as well as by a checkout, and told to
   * someone who pressed Pull the checkout wording reads "Switching to main would overwrite
   * local changes" — a switch nobody asked for, to the branch they are already on.
   */
  run: (work: () => Promise<string>, op?: GitOp) => Promise<void>
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

  async run(work, op = 'checkout') {
    set({ busy: true, note: null })
    try {
      const note = await work()
      set({ note: note === '' ? null : note })
    } catch (error) {
      // `explain` turns the tagged wire error into a sentence. `String(error)` would print
      // `[object Object]`, which is how an action ends up looking like it did nothing.
      set({ note: explain(error, op) })
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

  // Every mutation in `cmd/git.rs` broadcasts this — a checkout, a branch created or deleted,
  // a commit — so it is what moves the label for anything cide itself did.
  useEffect(() => {
    const stop = events.onGitStatus((changed: ProjectId) => {
      if (changed === useBranches.getState().project) void useBranches.getState().load(changed)
    })
    return () => void stop.then((off: () => void) => off())
  }, [])

  /*
   * The watcher, for a checkout cide did not make. (M17)
   *
   * The comment above used to claim this second half — *"and so does the watcher's view of
   * `HEAD` and the refs, which is how a `git checkout` in a bash pane moves the label"* — and it
   * was false in the way that is hardest to notice: the mechanism it described exists, it is
   * just on a different event. `cide://git-status` is emitted only by `cmd/git.rs::refreshed`.
   * The watcher's view arrives as `cide://fs-changed`, and nothing here was listening, so a
   * `git checkout` typed into a bash pane left the status bar naming the branch you left.
   *
   * **Gated on `change.git`, which is the opposite of the choice the Git panel makes** and is
   * right for the opposite reason. A label only moves when `HEAD`, a ref or `packed-refs` moves,
   * and `cide_fs`'s watcher raises this flag for exactly those and deliberately not for `.cide/`
   * (a ticked task is not a commit). Without the gate every keystroke-triggered file save would
   * cost a `git_branches` walk of every repository in the project, for an answer that cannot
   * have changed.
   */
  useEffect(() => {
    const stop = events.onFsChanged((changed: ProjectId, change) => {
      if (!change.git) return
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
      <Icon name="git-branch" size={1} />
      {/* The label is its own box because the *button* is now a flex row: `text-overflow`
          truncates a block container's inline content, so it has to live on the thing holding
          the text rather than on the thing holding the mark and the text. */}
      <span className={styles.widgetLabel}>{label}</span>
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
  /** The merge confirmation. `name` is the source — the row the "…" was opened on. */
  | { kind: 'merge'; name: string }
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
  /*
   * Where the arrows have walked to. `FILTER` is a *place*, not the absence of a selection —
   * "Up from the first branch returns to the filter" is then a transition the reducer states
   * rather than an off-by-one somebody clamps away. See `branchModel::navigate`.
   */
  const [focus, setFocus] = useState<BranchFocus>(FILTER)
  const search = useRef<HTMLInputElement>(null)
  const popupEl = useRef<HTMLDivElement>(null)
  const rowsEl = useRef<HTMLDivElement>(null)

  const list = listFor(lists, chosen)
  const rows = visibleBranches(list, query)
  const repo = list?.repo.id ?? null

  useEffect(() => {
    // Consumed: reopening from the widget starts on the list again.
    useBranches.setState({ intent: 'list' })
  }, [])

  /*
   * Focus follows the panel, and it has to.
   *
   * Every key this popup answers — the arrows and Enter on the search field, Escape on the
   * scrim — is a React handler on an element *inside* the scrim. A panel that unmounts the
   * field leaves focus on `document.body`, which is outside that subtree, and all three go
   * dead at once with nothing on screen to say so. Enter put that on the ordinary keyboard
   * route: it can raise the refusal panel, and Escape then failed to back out of the panel
   * the keystroke had just produced.
   *
   * Keyed on `mode.kind` rather than `mode`, so the delete panel hardening its question
   * (`force` flips, `kind` does not) does not yank focus back mid-read. `new` and `rename`
   * are deliberately absent: they are `NameForm`, which focuses its own field on mount, and a
   * child's effect runs before its parent's — reaching in from here would take focus straight
   * back off the name box. The other two panels have no field to want it, so the dialog takes
   * it; it is `tabIndex={-1}`, focusable programmatically and skipped by Tab.
   */
  useEffect(() => {
    if (mode.kind === 'list') search.current?.focus()
    else if (mode.kind === 'delete' || mode.kind === 'merge' || mode.kind === 'refusal')
      popupEl.current?.focus()
  }, [mode.kind])

  /*
   * The highlight, made safe against a list that changed underneath it.
   *
   * Derived every render rather than corrected in an effect: an effect leaves one frame in
   * which the popup draws a highlight on a row that is not there, and a keystroke arriving in
   * that frame reads the stale index. Two live routes shorten the list — typing into the
   * filter, and `cide://git-status`, which fires whenever anything touches the refs, so a
   * `git branch -d` in a terminal pane re-renders this popup with fewer rows.
   */
  const at = clampFocus(focus, rows.length)
  const rowId = (index: number) => `branch-row-${index}`

  /*
   * Keep the highlighted row on screen. `.rows` scrolls, and arrows that walk the highlight
   * out of sight look like a list that is not moving at all.
   *
   * `useLayoutEffect` so the scroll lands in the same frame as the highlight; `block: 'nearest'`
   * so a highlight already in view does not re-centre the list under the pointer.
   */
  useLayoutEffect(() => {
    if (at.kind !== 'row') return
    rowsEl.current?.querySelector(`#${rowId(at.index)}`)?.scrollIntoView({ block: 'nearest' })
  }, [at.kind, at.kind === 'row' ? at.index : -1])

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
  const act = (work: (project: ProjectId, repo: RepoId) => Promise<string>, op: GitOp = 'checkout') => {
    if (project === null || repo === null) return
    setMode({ kind: 'list' })
    setOpenRow(null)
    /*
     * And take focus back, which the effect above cannot be relied on to do.
     *
     * Fetch, Pull and Push are reachable only *from* the list, so the line above writes the
     * value `mode.kind` already held. That effect is keyed on `mode.kind` — deliberately, so a
     * panel re-render does not yank focus mid-read — so it does not re-run. Focus stays on the
     * button the pointer pressed, and every arrow key is a handler on the search field, so one
     * click on Fetch silently killed the keyboard navigation this popup exists for.
     */
    search.current?.focus()
    void useBranches.getState().run(() => work(project, repo), op)
  }

  /*
   * Pull, which — like `tryCheckout` below — has one rejection that is **not a sentence**.
   *
   * A divergence with nothing configured is a question, and routing it through `store.run`
   * would print *"main is 2 ahead and 5 behind…"* into the note bar: a true sentence, offering
   * no way to act on it, from the one control in the app that could have offered one. So it is
   * unpacked here and handed to the same dialog Ctrl+T raises, and everything else falls back
   * to `explain` exactly as before. Same shape, same reason, as `tryCheckout`'s stash panel.
   *
   * The gate is mounted at `App.tsx` level and `OverlayCard` draws its own scrim above this
   * popup's, so the dialog is legible from here without the popup having to close.
   */
  const tryPull = async () => {
    if (project === null || repo === null || busy) return
    const p = project
    const r = repo
    /*
     * **Both** pull roads are tracked, and the second is the one that is easy to miss. (M65)
     *
     * `one` is re-issued from the divergence dialog's `proceed` below, long after the `finally`
     * at the foot of this function has run — and it is the *slower* road, because it is the one
     * that actually merges or rebases. Tracking only the opening fetch-and-fast-forward would
     * leave the bar silent for exactly the pull worth showing.
     *
     * The wrap goes round `branchApi.pull` and not round `store.run`, so the indicator clears
     * when the pull answers rather than when the refresh walk that follows it does. `run`'s
     * promise never rejects — it catches into `note` — and resolves only after `load()`, so
     * wrapping it would have said *Pulling* through a status walk of every repository.
     */
    const one = (request: Parameters<typeof branchApi.pull>[2]) =>
      useBranches
        .getState()
        .run(async () => fetchNote(await trackGitOp(p, 'pull', branchApi.pull(p, r, request))), 'pull')

    search.current?.focus()
    try {
      await trackGitOp(p, 'pull', branchApi.pull(p, r, { skipFetch: false, remember: false }))
        .then((outcome) => useBranches.getState().say(fetchNote(outcome)))
    } catch (error) {
      const diverged = divergenceOf(error)
      if (diverged === null) {
        useBranches.getState().say(explain(error, 'pull'))
        return
      }
      const name = list?.repo.name ?? 'this repository'
      const asked = [{ name, repo: String(r), diverged }]
      const ask = strategyAsk(asked)
      if (ask === null) return
      requestPullStrategy({
        ask,
        repos: asked,
        // `skipFetch`, for the reason `keys/dispatch.ts` gives at length: the counts the user
        // has just read describe refs that are already on disk.
        proceed: (strategy, remember) => void one({ strategy, skipFetch: true, remember }),
      })
    } finally {
      void useBranches.getState().load(p, true)
    }
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
        /*
         * "It should close after selection" — answered here, **after the checkout resolved**,
         * and deliberately not on the click.
         *
         * `CheckoutWouldOverwrite` is a real refusal this popup answers with a panel naming
         * the files that would be lost, and `explain` produces a sentence for every other
         * failure. A popup that had already closed would have thrown both away, leaving the
         * user standing on the branch they started from with nothing to say why. So the
         * dismiss lives on the success path only; the `catch` below leaves the popup up.
         *
         * The note goes to `chrome/notices` rather than the store's note bar, because that bar
         * is inside the popup that is being unmounted on the next line — a stash that was
         * taken along, or a local branch created from a remote, would otherwise be reported
         * into a component nobody can see. `restoreFailed` is an error: it means the stash was
         * made and could not be put back, which is the one outcome the user must act on.
         */
        const said = checkoutNote(outcome)
        if (said !== '') {
          notify(said, { kind: outcome.restoreFailed !== null ? 'error' : 'ok' })
        }
        onDismiss()
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

  /*
   * Merge, whose one non-sentence outcome goes to a different surface entirely.
   *
   * A conflicted merge is a success with work attached — real `MERGE_HEAD` state is on disk —
   * and the one component that can finish it is the resolver, so this dismisses the popup and
   * opens the conflicts list the way `keys/dispatch.ts` does after a conflicted pull, and for
   * its reason: the toast-or-note route says "2 files to resolve" and offers nothing that
   * resolves them. The popup closes first because the user's next act is resolving, not branch
   * picking. (The popup's own Pull leaves conflicts as a note instead — it already has the
   * divergence dialog stacked over it; unifying the two routes is a known follow-up.)
   *
   * A `checkoutWouldOverwrite` here goes through `explain`, never the refusal panel: that
   * panel's stash buttons are *checkout* gestures, and after a blocked merge they would stash
   * the user's work and then merge with it off screen.
   */
  const tryMerge = (name: string) => {
    if (project === null || repo === null || busy) return
    const p = project
    const r = repo
    const repoName = lists.length > 1 ? (list?.repo.name ?? '') : ''
    setMode({ kind: 'list' })
    // The row menu the panel was opened from, or the list comes back with it still open.
    setOpenRow(null)
    search.current?.focus()
    // Same load-bearing `busy` as `tryCheckout`: a double-click on Merge must not start a
    // second merge over the `MERGE_HEAD` the first one may be about to write.
    useBranches.setState({ busy: true, note: null })
    void (async () => {
      try {
        // Tracked round the call itself rather than through the `finally` below, so the
        // indicator clears when the merge answers and not when the refresh walk after it does.
        // The conflict path returns from inside this `try`, and an awaited wrap has already
        // settled by then. (M65)
        const outcome = await trackGitOp(p, 'merge', branchApi.merge(p, r, name))
        if (outcome.conflicts.length > 0) {
          onDismiss()
          const state = await branchApi.conflicts(p, r)
          if (state !== null) showConflicts({ project: p, repo: r, repoName, state })
          return
        }
        useBranches.getState().say(mergeNote(outcome))
      } catch (error) {
        useBranches.getState().say(explain(error, 'merge'))
      } finally {
        useBranches.setState({ busy: false })
        await useBranches.getState().load(p, true)
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
        ref={popupEl}
        /*
         * Focusable programmatically, skipped by Tab. The delete and refusal panels have no
         * field of their own, so the dialog takes focus itself — otherwise it lands on
         * `document.body`, outside the scrim, and Escape stops backing out of the very panel
         * the user is reading.
         */
        tabIndex={-1}
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
                role="combobox"
                aria-expanded
                aria-controls="branch-rows"
                /*
                 * DOM focus never leaves this field on the arrow route, so the field is what
                 * names the highlighted row for a screen reader. Real focus movement was the
                 * alternative and it loses twice: the current branch's row button is
                 * `disabled` and cannot be focused — making the branch you are on the one row
                 * the keyboard cannot reach — and a focused row swallows typing, so filtering
                 * would stop working the moment you pressed Down.
                 */
                aria-activedescendant={at.kind === 'row' ? rowId(at.index) : undefined}
                onChange={(e) => {
                  setQuery(e.target.value)
                  // Typing re-filters, so the old index means nothing. Back to the field.
                  setFocus(FILTER)
                }}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') {
                    const action = enterAction(rows, at)
                    if (action.kind === 'checkout') tryCheckout(action.name)
                    // Said rather than silent: the current branch is pinned first, so it is
                    // the row the first Down always lands on, and an ⏎ that did nothing at
                    // all there is how a user concludes the keyboard is not wired up.
                    else if (action.kind === 'already') useBranches.getState().say(alreadyOn(action.name))
                    return
                  }
                  const next = navigate(e.key, at, rows.length)
                  // `null` is "not ours, or nowhere to go", and the key is left to the field's
                  // own caret behaviour rather than eaten.
                  if (next === null) return
                  e.preventDefault()
                  setFocus(next)
                }}
              />
              <div className={styles.actions}>
                <Button size="sm"
                  disabled={busy}
                  onClick={() => setMode({ kind: 'new', from: null })}
                >
                  New branch…
                </Button>
                <Button size="sm"
                  disabled={busy || project === null || repo === null}
                  // Tracked, so the bar says `Fetching…` for the whole network round trip.
                  // `act` routes this through `store.run` as a `'checkout'` — `GitOp` has no
                  // `fetch` arm and deliberately gains none, because that type exists for a
                  // refusal shared by the operations that move the working tree. Which verb the
                  // indicator says is a separate question, asked here. (M65)
                  onClick={() =>
                    act(async (p, r) => fetchNote(await trackGitOp(p, 'fetch', branchApi.fetch(p, r))))
                  }
                >
                  Fetch
                </Button>
                <Button size="sm"
                  // Without an upstream this only ever fails, so it is not offered.
                  disabled={busy || !canPull(list?.head ?? null)}
                  onClick={() => void tryPull()}
                  title="Fetch, then integrate — fast-forward, merge or rebase."
                >
                  Pull
                </Button>
                <Button size="sm"
                  disabled={busy || !canPush(list?.head ?? null)}
                  /*
                   * Opens the dialog rather than pushing, since M31 — the same road the palette
                   * takes, because two routes to one gesture that disagreed about whether it
                   * asked first is exactly the class of split M20 already paid for here (the
                   * panel's dim note line against the palette's red box, over one rejection).
                   *
                   * The dialog covers the whole project and this button belongs to one
                   * repository's popup. That is not a mismatch to fix by narrowing it: the
                   * project is what `git.push` has always meant, the dialog names every row, and
                   * the row for the repository whose popup this is arrives ticked like the rest.
                   *
                   * `--set-upstream` is no longer decided here either. It was the only caller
                   * that passed it, worked out from `head.upstream === null`; the preview reads
                   * the same fact off the absent remote-tracking ref, per row, and `pushPass`
                   * sends it.
                   */
                  onClick={() => {
                    if (project === null) return
                    onDismiss()
                    openPushDialog(project)
                  }}
                >
                  Push
                </Button>
              </div>
            </div>

            <div
              className={styles.rows}
              role="listbox"
              aria-label="Branches"
              id="branch-rows"
              ref={rowsEl}
            >
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
                  head={list?.head ?? null}
                  // The section headings are drawn from the transition rather than by
                  // splitting the array: one flat list is one keyboard sequence, and the
                  // heading is a property of the boundary.
                  heading={sectionOf(entry, rows[index - 1])}
                  id={rowId(index)}
                  active={at.kind === 'row' && at.index === index}
                  open={openRow === entry.name}
                  busy={busy}
                  onToggleMenu={() => setOpenRow(openRow === entry.name ? null : entry.name)}
                  onCheckout={() => tryCheckout(entry.name)}
                  onNewFrom={() => setMode({ kind: 'new', from: entry.name })}
                  onMerge={() => setMode({ kind: 'merge', name: entry.name })}
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
              {/*
               * Not "its commits are on no other branch" — Rust only checked whether HEAD can
               * reach the tip (`git branch -d`'s question), so a branch already merged into
               * `release` while you stand on `main` reaches this panel with nothing at stake.
               * A red button carrying a false claim is how a user learns to stop reading them.
               */}
              {mode.force
                ? `${mode.name} is not fully merged into the branch you are on. Anything only on it would be left unreachable — check it out, or merge it somewhere, if you are not sure.`
                : 'Deleting a branch removes the name. The commits stay until git collects them.'}
            </p>
            <div className={styles.panelButtons}>
              <Button size="sm" onClick={back}>
                Cancel
              </Button>
              <Button size="sm" variant={mode.force ? 'danger' : 'primary'}
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
              </Button>
            </div>
          </div>
        )}

        {mode.kind === 'merge' && (
          <MergePanel
            ask={mergeConfirm(mode.name, list?.head.head ?? 'the current branch')}
            busy={busy}
            onCancel={back}
            onMerge={() => tryMerge(mode.name)}
          />
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
            {/*
             * The scope sentence, and it is not decoration. Both buttons run `git stash -u`,
             * which takes the **whole** working tree — every modification and every untracked
             * file, including ones nothing above named. A panel that lists one file and then
             * empties the tree is a confirmation that understated what it was asking for, and
             * *Stash and switch* is the route that leaves the rest of it off disk.
             */}
            <p className={styles.panelBody}>
              Both stash buttons take the whole working tree, not only the files listed — every
              change and every untracked file goes into one stash entry. Nothing is discarded;{' '}
              <code>git stash list</code> has it either way.
            </p>
            <div className={styles.panelButtons}>
              <Button size="sm" onClick={back}>
                Cancel
              </Button>
              <Button size="sm"
                disabled={busy}
                onClick={() => tryCheckout(mode.refusal.branch, 'stash')}
                title="git stash -u, then switch. The whole working tree — including untracked files — stays in the stash."
              >
                Stash and switch
              </Button>
              <Button size="sm" variant="primary"
                disabled={busy}
                onClick={() => tryCheckout(mode.refusal.branch, 'stashAndRestore')}
                title="Stash the whole working tree, switch, then put it all back on the new branch."
              >
                Bring changes along
              </Button>
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
  /** The repository's HEAD — what a merge would merge *into*. `actionsFor` gates on it. */
  head: BranchInfo | null
  heading: string | null
  /** Stable per index, so the field's `aria-activedescendant` can name it. */
  id: string
  /** The arrows have walked here. Drawn, but never focused — see the field's comment. */
  active: boolean
  open: boolean
  busy: boolean
  onToggleMenu: () => void
  onCheckout: () => void
  onNewFrom: () => void
  onMerge: () => void
  onRename: () => void
  onDelete: () => void
}

function Row({ entry, head, heading, id, active, open, busy, ...on }: RowProps) {
  const actions = actionsFor(entry, head)
  /*
   * `null` rather than `''` when the count is zero, and the distinction is the whole point: an
   * arrow with no number beside it is a claim about direction with no magnitude, which is not
   * what "in sync" means. An empty string rendered nothing; so does `null`.
   */
  const ahead =
    entry.ahead > 0 ? (
      <span className={styles.count}>
        <Icon name="arrow-up" size={0} />
        {entry.ahead}
      </span>
    ) : null
  const behind =
    entry.behind > 0 ? (
      <span className={styles.count}>
        <Icon name="arrow-down" size={0} />
        {entry.behind}
      </span>
    ) : null

  return (
    <>
      {heading !== null && <div className={styles.section}>{heading}</div>}
      <div
        id={id}
        className={`${entry.current ? styles.rowCurrent : styles.row}${active ? ` ${styles.rowOn}` : ''}`}
      >
        <button
          type="button"
          className={styles.rowMain}
          role="option"
          /*
           * The *highlight*, not the current branch. `aria-selected` on a listbox option means
           * "this is the one you are choosing", which is what the arrows move; the branch you
           * are standing on is already spelled out in the row itself.
           */
          aria-selected={active}
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
        <IconButton
          icon="ellipsis"
          label={`Actions for ${entry.name}`}
          aria-expanded={open}
          onClick={on.onToggleMenu}
        />
      </div>
      {open && (
        <div className={styles.rowMenu}>
          {actions.includes('checkout') && (
            <Button size="sm" onClick={on.onCheckout}>
              Checkout
            </Button>
          )}
          {actions.includes('newFrom') && (
            <Button size="sm" onClick={on.onNewFrom}>
              New branch from here
            </Button>
          )}
          {actions.includes('merge') && (
            <Button size="sm" onClick={on.onMerge}>
              Merge into current branch
            </Button>
          )}
          {actions.includes('rename') && (
            <Button size="sm" onClick={on.onRename}>
              Rename
            </Button>
          )}
          {actions.includes('delete') && (
            <Button size="sm" onClick={on.onDelete}>
              Delete
            </Button>
          )}
        </div>
      )}
    </>
  )
}

interface MergePanelProps {
  /** `branchModel::mergeConfirm`'s question, built by the caller so the check can pin it. */
  ask: { title: string; body: string }
  busy: boolean
  onCancel: () => void
  onMerge: () => void
}

/**
 * The merge confirmation — the delete panel's shape, without delete's escalation: there is no
 * harder question a merge can come back with, because the refusals (`operationInProgress`,
 * `checkoutWouldOverwrite`, …) are sentences for the note bar, and a conflict is not a refusal
 * at all — it dismisses the popup and opens the resolver.
 */
function MergePanel({ ask, busy, onCancel, onMerge }: MergePanelProps) {
  return (
    <div className={styles.panel}>
      <p className={styles.panelTitle}>{ask.title}</p>
      <p className={styles.panelBody}>{ask.body}</p>
      <div className={styles.panelButtons}>
        <Button size="sm" onClick={onCancel}>
          Cancel
        </Button>
        <Button size="sm" variant="primary" disabled={busy} onClick={onMerge}>
          Merge
        </Button>
      </div>
    </div>
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
        <Button size="sm" onClick={onCancel}>
          Cancel
        </Button>
        {secondary !== undefined && (
          <Button size="sm"
            disabled={!ready}
            onClick={() => onSubmit(name.trim(), false)}
          >
            {secondary}
          </Button>
        )}
        <Button size="sm" variant="primary"
          disabled={!ready}
          onClick={() => onSubmit(name.trim(), secondary !== undefined)}
        >
          {submit}
        </Button>
      </div>
    </div>
  )
}
