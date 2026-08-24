/**
 * A file as one commit left it, read-only: the pane behind `TabKind::Revision`. (M19)
 *
 * The sibling of `GitDiffPane` and the answer to a different question. A diff asks *what
 * changed here*; this asks *what did this file look like then*, which is the question the blame
 * gutter's **Annotate previous revision** actually poses — `cide_git::blame::blame_with`
 * documents its `newest` argument as blaming the file *as it was at that revision*, and the
 * point of the hop is to read that file, not to diff it.
 *
 * # The tab carries identity and never content
 *
 * `TabKind::Revision` holds `(repo, path, rev)` and a trail, and this pane calls
 * `gitLog.fileAt` when it mounts. That is `DiffSpec`'s rule, for the reason `DiffSpec`'s header
 * gives at length: the tab is persisted inside `Workspace`, which `cide-core::persist` debounces
 * to `workspace.json`, so a blob passed through the tab would be a blob written to disk — and a
 * `workspace.json` holding megabytes of file text that nobody reads back is the *cheap* half of
 * the cost.
 *
 * The expensive half does not apply here, and it is worth saying why rather than leaving the
 * rule to be re-litigated: a working-tree diff written to disk at 10:00 and restored at 14:00
 * describes a file that no longer exists in that form, whereas **a commit is immutable**. A
 * revision tab restored tomorrow shows exactly the bytes it showed today. So the argument for
 * a key here is only the first one — but the first one is enough, and a second rule for one tab
 * kind ("this one may cache, that one may not") is how a persisted format grows a footgun.
 *
 * # Why `EditorSurface` and not a `<pre>`
 *
 * Four things, and this is the one view where all four are the whole point: Ctrl+F, code
 * folding, syntax highlighting, and a find bar that behaves like every other buffer in the app.
 * *Where did this come from* is a reading task, and a reader who cannot search the text they are
 * reading has been handed a screenshot. `readOnly` is what makes the surface a viewer — it
 * disables autosave by construction (`shouldAutosave` refuses a read-only buffer), and there is
 * no `onSave`, no dirty reporting and no tab id here for anything to save into.
 *
 * # The path and the identity are two different arguments, and this pane is why
 *
 * `EditorSurface` keys its per-buffer registrations — `registerReveal`, `viewTracker`,
 * `ctrlLink`, `navRecorder`, `claimCaret` — on an `identity` that defaults to the path, because
 * for every editor over a file on disk the two are the same thing. Here they are not: this is
 * `src/log.rs` **as `a1b2c3d` left it**, which is a different document at the same path, and two
 * documents sharing one key is two buffers fighting over one slot.
 *
 * The one that is not cosmetic is `registerReveal`. `revealRequest.ts::requestReveal` delivers to
 * *live receivers* and only parks when there are none — so a revision pane registered under the
 * real path makes a search-result click into that file stop working: the click parks nothing
 * (this pane is live), moves the caret in a commit nobody asked to see, opens a file tab, and
 * that tab is never told. Precisely the report `revealRequest.ts` exists to answer, reintroduced
 * by a read-only viewer nobody would think to suspect.
 *
 * So the surface gets **both**: `path` is the repo-relative path, and `identity` is
 * `a1b2c3d:src/log.rs` — git's own name for the object, exactly what `git show` takes, and a
 * string that can never equal a real path because every real path here is absolute.
 * `TabKind::Revision`'s own doc comment makes the same point from the other side: there is no
 * file on disk for this tab to point at, and an absolute path would be a claim that there is.
 *
 * This replaced a workaround that passed the identity **as** the path, which bought the same
 * collision-freedom and cost two things, both now recovered: the status bar's trail read
 * `a1b2c3d:src › log.rs` instead of `src › log.rs`, and every consumer that legitimately wants
 * the file — `languageName`, `loadLanguage`, `pathTrail` — was reading a name with a commit
 * glued to the front of it and getting away with it only because the basename survived.
 * `crumbTargets` still answers `null` for every crumb, because the path here is repo-relative and
 * no reveal root can contain it; the trail is drawn inert, which is the truth about where those
 * crumbs lead.
 *
 * One collision the split does not close, stated so it is not rediscovered as a surprise: the
 * buffer's context menu takes the path, so *Highlighting: syntax only* chosen here records a
 * session-only override against the working file. `editor/codeMenu.tsx` takes one path for five
 * items that all want the real one, and this pane pins `highlight="none"` anyway.
 *
 * # `project` is deliberately not passed to the surface
 *
 * Which turns off Ctrl+click and the mouse's Back-stack recorder, and both would be *wrong*
 * here rather than merely unavailable. Go-to-definition sends `(path, line, column)` to a
 * language server reading the file **as it is now**; a line number taken from a buffer as it was
 * forty commits ago names a different symbol in that file, and the jump lands somewhere
 * confidently incorrect. That is the same failure `EditorPane.onAnnotateParent` refuses for the
 * blame gutter — *nothing is worse than a column that is confidently misaligned* — and it is
 * worse here, because a wrong jump moves the user rather than mislabelling something in front of
 * them. Ctrl+F, folding and highlighting need nothing from the backend and are unaffected.
 */
import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react'
import { EditorSurface } from '@/editor/EditorSurface'
import { file as fileApi, git as gitApi, gitLog, revisionFile } from '@/ipc/client'
import type { ProjectId, RepoId, RevisionBlob } from '@/ipc/client'
import { describe, notify } from '@/chrome/notices'
import { Icon } from '@/icons/Icon'

import styles from './RevisionPane.module.css'

export interface RevisionPaneProps {
  /** The project the repository belongs to. Every call below needs it. */
  project: string
  repo: RepoId
  /** **Repo-relative** and slash-separated, the way `cide_ipc::git` spells every path. */
  path: string
  /** A full oid. `TabKind::Revision.rev` is resolved before it is persisted, so it always is. */
  rev: string
  /**
   * The walk that led here — newest **last**, `rev` itself excluded.
   *
   * `TabKind::Revision.from`'s documented order, and the order the strip below draws left to
   * right after `working tree`. Normalised in Rust (`cide_core::workspace::revision_chain`), so
   * this array is already free of duplicates and of `rev` itself and nothing here re-checks it.
   */
  from: readonly string[]
  /**
   * Whether this pane's tab is the one in front. Passed straight through to `EditorSurface`,
   * which says what reads it and why a hidden tab is otherwise indistinguishable from the
   * visible one — hidden tabs are `visibility: hidden`, never unmounted.
   */
  onScreen?: boolean | undefined
}

/** What the pane is showing instead of, or as well as, a buffer. */
type Load =
  | { kind: 'loading' }
  | { kind: 'failed'; why: string }
  | { kind: 'ready'; blob: RevisionBlob }

/**
 * Seven characters, which is what `git log --oneline` shows and what every other short oid in
 * this application is cut to — including `cmd::file::revision_title`, which builds the tab's
 * label. The two must agree: a tab reading `log.rs @ a1b2c3d` above a crumb reading `a1b2c3` is
 * two spellings of one commit in one glance.
 *
 * `slice` on a hex string is safe where `EditorSurface`'s callers are not (an oid has no
 * non-ASCII boundary to split), but this takes the same `Array.from` route anyway rather than
 * leaving a second rule about when slicing an oid is allowed.
 */
function shortRev(rev: string): string {
  return Array.from(rev).slice(0, 7).join('')
}

/**
 * `12.3 kB` — the real size of a blob the pane is only showing the top of.
 *
 * Decimal kB/MB and not KiB, because this number's whole job is to be compared against the
 * two-megabyte cap in `cide_git::revision::MAX_BLOB_BYTES` by a person, and a reader who has to
 * ask which kind of megabyte has not been told anything.
 */
function humanBytes(bytes: number): string {
  if (bytes < 1000) return `${bytes} bytes`
  if (bytes < 1000 * 1000) return `${(bytes / 1000).toFixed(1)} kB`
  return `${(bytes / (1000 * 1000)).toFixed(1)} MB`
}

/** One crumb in the trail. `to` is `null` for the working tree, which is not a revision. */
interface Crumb {
  readonly key: string
  readonly label: string
  readonly to: { readonly rev: string; readonly from: readonly string[] } | null
  /** The revision this pane is showing. Drawn lit, and clicking it re-activates this tab. */
  readonly current: boolean
}

export function RevisionPane({
  project,
  repo,
  path,
  rev,
  from,
  onScreen,
}: RevisionPaneProps): ReactNode {
  const [load, setLoad] = useState<Load>({ kind: 'loading' })

  /*
   * One fetch per identity, and no refetch ever after it.
   *
   * A commit is immutable, so unlike `GitDiffPane` — which follows `cide://git-status` and the
   * filesystem watcher because its subject moves under it — there is no event that could change
   * what this pane should be showing. Not subscribing is the correct behaviour and not an
   * omission: a revision pane that re-read on every agent write would run one
   * `git_file_at_revision` per open tab per keystroke, to arrive at the same bytes.
   *
   * `alive` and not an `AbortController`: `invoke` has no cancellation, so the guard is against
   * writing state after the identity changed, which is the failure that matters (a tab switched
   * to another revision showing the previous one's text).
   */
  useEffect(() => {
    let alive = true
    setLoad({ kind: 'loading' })
    gitLog
      .fileAt(project as ProjectId, repo, path, rev)
      .then((blob) => {
        if (alive) setLoad({ kind: 'ready', blob })
      })
      .catch((error: unknown) => {
        if (alive) setLoad({ kind: 'failed', why: describe(error) })
      })
    return () => {
      alive = false
    }
  }, [project, repo, path, rev])

  /**
   * Open the file as it is **now**.
   *
   * Two round trips, because a `RepoId` is a uuid derived from a canonical work tree and this
   * pane holds a repo-relative path — there is no absolute path anywhere in the tab to open.
   * `git.repos` is the cheapest git question there is by its own documentation (a
   * `Repository::discover` per root plus a submodule walk, no working-tree walk), which is why
   * it is asked on the click rather than cached at mount: a `git init` in a bash pane changes
   * the answer, and this gesture happens once in a while and never twice a second.
   *
   * `file.open` and not `file.openFromTerminal`: this path came out of the project's own
   * repository index, not out of a terminal's output, so there is nothing to confirm.
   */
  const openWorkingTree = useCallback(() => {
    void gitApi
      .repos(project as ProjectId)
      .then((repos) => {
        const root = repos.find((info) => info.id === repo)?.root
        if (root === undefined) {
          notify('That repository is no longer part of this project.', { kind: 'warn' })
          return undefined
        }
        return fileApi.open(project as ProjectId, `${root.replace(/\/+$/, '')}/${path}`)
      })
      .catch((error: unknown) => {
        notify(describe(error), { kind: 'error' })
      })
  }, [project, repo, path])

  /**
   * Walk to one of the revisions in the trail.
   *
   * The chain is passed **truncated at that point**, which is the whole reason walking back does
   * not grow it: going `rev → from[i]` hands over `from.slice(0, i)`, so the tab that opens has
   * exactly the trail it would have had if the user had stopped there in the first place. Rust
   * then keys open-or-activate on `(repo, path, rev)` and ignores the chain when a tab is
   * already up, so this is also how "activate the tab I am in" is spelled — the current crumb
   * passes its own `(rev, from)` and lands back here.
   *
   * The path does not change across a hop, and that is a known limitation rather than an
   * oversight: `TabKind::Revision.from` is a list of oids with no per-hop path, so a walk that
   * crossed a rename cannot spell the older name here. `git_file_at_revision` answers
   * `NotTracked` for a path that did not exist at that commit and the pane says so, which is a
   * legible failure. Fixing it properly means the chain carrying `(rev, path)` pairs, which is a
   * change to a persisted field and to every workspace already holding one.
   */
  const walkTo = useCallback(
    (target: { rev: string; from: readonly string[] }) => {
      void revisionFile
        .openTab(project as ProjectId, repo, path, target.rev, target.from)
        .catch((error: unknown) => {
          notify(describe(error), { kind: 'error' })
        })
    },
    [project, repo, path],
  )

  /*
   * The trail, drawn from persisted state and therefore the same after a restart — which is the
   * whole reason `from` is on the tab rather than in a component's `useState`. A user four hops
   * deep who restarts cide would otherwise land on a commit with no explanation of how they got
   * there and no route out but the log.
   *
   * `working tree` leads it because that is where every walk starts, and it is the one crumb
   * that is not a revision.
   */
  const crumbs = useMemo<Crumb[]>(
    () => [
      { key: 'working-tree', label: 'working tree', to: null, current: false },
      ...from.map((oid, index) => ({
        key: `${index}:${oid}`,
        label: shortRev(oid),
        to: { rev: oid, from: from.slice(0, index) },
        current: false,
      })),
      { key: `now:${rev}`, label: shortRev(rev), to: { rev, from }, current: true },
    ],
    [from, rev],
  )

  const goBack = useCallback(() => {
    const previous = from[from.length - 1]
    if (previous === undefined) {
      openWorkingTree()
      return
    }
    walkTo({ rev: previous, from: from.slice(0, from.length - 1) })
  }, [from, openWorkingTree, walkTo])

  const follow = useCallback(
    (crumb: Crumb) => {
      if (crumb.to === null) openWorkingTree()
      else walkTo(crumb.to)
    },
    [openWorkingTree, walkTo],
  )

  /*
   * `a1b2c3d:src/log.rs`. See the header: this is what keeps the buffer out of the per-document
   * registries the real path would collide with, and it is git's own name for the object.
   *
   * `EditorSurface`'s contract for this prop is that an identity is fixed for the life of a
   * mount, and this pane satisfies it structurally rather than by care: Rust keys
   * `TabKind::Revision` on `(repo, path, rev)` and `walkTo` opens or activates a tab per triple,
   * so another revision is another tab and therefore another mount.
   */
  const identity = useMemo(() => `${shortRev(rev)}:${path}`, [rev, path])

  return (
    <div className={styles.pane}>
      {/*
        * `data-pane-strip="fluid"`, and the value is load-bearing rather than decorative.
        *
        * `layout/PaneTitleBar.module.css` steps its floating ⊞ ⛶ ⧉ × cluster down past a top
        * strip *of known height* — which is how the find bar is handled on an editor pane. This
        * strip sits ABOVE the find bar, so it is the topmost one whenever it is drawn, and a
        * step sized for the find bar would put the cluster through the middle of it. Declaring
        * `fluid` excludes this frame from that rule and keeps the cluster where it is; the strip
        * reserves the band horizontally instead (`--pane-corner-clear` in the stylesheet), which
        * is what stops `⌫ Back` from being drawn under the × and answering its click.
        */}
      <div className={styles.trail} data-pane-strip="fluid">
        <div className={styles.crumbs}>
          {crumbs.map((crumb, index) => (
            <span key={crumb.key} className={styles.crumbSlot}>
              {index > 0 && (
                <span className={styles.arrow} aria-hidden="true">
                  <Icon name="chevron-left" size={1} />
                </span>
              )}
              <button
                type="button"
                className={crumb.current ? `${styles.crumb} ${styles.crumbCurrent}` : styles.crumb}
                {...(crumb.current ? { 'aria-current': 'page' as const } : {})}
                title={crumb.to === null ? path : `${path} at ${crumb.to.rev}`}
                onClick={() => follow(crumb)}
              >
                {crumb.label}
              </button>
            </span>
          ))}
        </div>
        <button type="button" className={styles.back} onClick={goBack}>
          ⌫ Back
        </button>
      </div>
      {load.kind === 'loading' && (
        <div className={styles.notice}>
          <div className={styles.noticeWhy}>
            Reading {path} at {shortRev(rev)}…
          </div>
        </div>
      )}
      {load.kind === 'failed' && (
        <div className={styles.notice}>
          <div className={styles.noticeTitle}>This revision could not be read</div>
          <div className={styles.noticeWhy}>{load.why}</div>
        </div>
      )}
      {/*
        * Three states, three sentences — deliberately not one message with the details swapped
        * in. They are different facts and the reader acts on each of them differently:
        *
        * * **Binary.** There is nothing to show at all, at any size. `RevisionBlob.text` is empty
        *   rather than a screenful of replacement characters, so a buffer here would be a blank
        *   editor claiming the file was empty at that commit — a false statement, not a missing
        *   one. `binary` and `truncated` are never both meaningful; `cide_git::revision` says so.
        * * **Truncated.** There is plenty to show and the top of it is what the reader came for,
        *   so the buffer is drawn *and* the real size is named. Saying only "too large" would
        *   hide text that is right there; showing it silently would let somebody scroll to a
        *   bottom that is not the bottom and believe it. `file_at_revision` backs the cut off to
        *   the last newline for the same reason — a half line reads as corruption.
        * * **Ordinary.** The buffer, and nothing above it.
        */}
      {load.kind === 'ready' && load.blob.binary && (
        <div className={styles.notice}>
          <div className={styles.noticeTitle}>This file is binary at {shortRev(rev)}</div>
          <div className={styles.noticeWhy}>
            {humanBytes(load.blob.bytes)} of binary content — there is nothing to show as text.
          </div>
        </div>
      )}
      {load.kind === 'ready' && !load.blob.binary && (
        <>
          {load.blob.truncated && (
            <div className={styles.truncated} role="status">
              Showing the first part of a {humanBytes(load.blob.bytes)} file.
            </div>
          )}
          <div className={styles.surface}>
            <EditorSurface
              /*
               * Both, and the pair is the point — see the header. `path` is what the file is
               * called, so the language, the trail and the find bar behave like every other
               * buffer; `identity` is what this *document* is, so nothing this pane registers can
               * be mistaken for the working file. Repo-relative is deliberate too: it is what the
               * tab holds, and there is no absolute path anywhere in a revision tab to pass.
               */
              path={path}
              identity={identity}
              doc={load.blob.text}
              readOnly
              /*
               * `highlight` is about **diagnostics**, not syntax — syntax highlighting is
               * unconditional in the surface and `loadLanguage` is driven by the path above. So
               * `none` is the honest setting rather than a downgrade: there are no diagnostics
               * for a commit's version of a file, nothing produces any, and the prop's own
               * documentation says `none` removes the lint gutter column instead of leaving an
               * empty one in a pane that has no use for it.
               */
              highlight="none"
              onScreen={onScreen}
            />
          </div>
        </>
      )}
    </div>
  )
}
