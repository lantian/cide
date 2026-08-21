/**
 * *Your branch has diverged. Merge, or rebase?* — the rules behind that dialog. (M20)
 *
 * # Why this is its own module
 *
 * `chrome/branchModel.ts` is *every decision the branch selector makes*, and this dialog is not
 * the selector's: it is raised by the key gate, from `keys/dispatch.ts`, in both window kinds.
 * Its own module also means its own check (`ui/scripts/check-pull-strategy.mjs`) rather than
 * another two hundred assertions in `check-branches.mjs`, where a failure would be harder to
 * read.
 *
 * **Zero imports, not even type-only ones.** That is `chrome/logActions.ts`'s rule rather than
 * `branchModel.ts`'s looser one, and it is why the plural helpers at the foot are duplicated
 * rather than shared: a value import — even from another import-free module — is what stops the
 * check compiling this file on its own. The shapes below are declared structurally and pinned
 * against `ui/src/ipc/generated.ts` by the check, which is the same guarantee from the other
 * side.
 *
 * # The two rules this file exists to keep honest
 *
 * **Merge is `choices[0]`.** `ConfirmDestructive` resolves an unknown `chosen` — including the
 * `null` a dialog opens with — to the first choice, and its header orders choices
 * least-destructive-first. A merge writes a commit and leaves every existing commit reachable,
 * undone exactly by `git reset --hard ORIG_HEAD`. A rebase **rewrites the commits you have not
 * pushed**: new oids, the originals reachable only from the reflog until it expires, and a
 * guaranteed rejection on the next push if the branch was already published. git's own
 * `pull.rebase` default is `false` too, so this is also the answer that agrees with the tool
 * underneath.
 *
 * **The remember checkbox starts unticked, and that inverts `ConfirmDestructive`'s house
 * rule.** That component ticks its option by default — *"the same instinct that puts focus on
 * Cancel, one step further in"* — but that rule is about an option which makes the act
 * **recoverable** (reset's *shelve my changes first*). This one is the opposite category: it
 * makes the *answer* permanent by writing `pull.rebase` into the repository's own config, a
 * file the `git pull` typed into a pane two lines below will also obey and which nothing
 * outside Settings will ever remind them about. Doing nothing must not silently reconfigure a
 * repository. A reviewer who knows the component's rule would otherwise read this as a bug,
 * which is why it is written down here rather than left to the call site.
 */

/** What the dialog can answer with. The generated `PullStrategy` minus `fastForward`. */
export type PullStrategy = 'merge' | 'rebase'

/** The generated `PulledCommit`, structurally. */
export interface DivergedCommit {
  readonly shortOid: string
  readonly summary: string
  readonly author: string
}

/** The generated `Divergence`, structurally — `GitError::PullNeedsStrategy`'s payload. */
export interface DivergenceLike {
  readonly branch: string
  readonly remote: string
  readonly upstream: string
  readonly ahead: number
  readonly behind: number
  readonly incoming: readonly DivergedCommit[]
  readonly moreIncoming: number
  readonly local: readonly DivergedCommit[]
  readonly moreLocal: number
}

/** One repository that asked, labelled. `RepoFetch`'s shape, for `RepoFetch`'s reason. */
export interface RepoDivergence {
  /** `RepoInfo.name`. Ignored when there is only one repository. */
  readonly name: string
  /** The `RepoId`, carried so the retry knows which repositories to re-issue for. */
  readonly repo: string
  readonly diverged: DivergenceLike
}

/** One mode, in the shape `ConfirmChoice` takes. */
export interface ConfirmChoiceLike {
  readonly id: string
  readonly label: string
  readonly body: string
  readonly files: readonly string[]
  readonly confirmLabel: string
  readonly danger: boolean
}

/** Everything `PullStrategyGate` hands to `ConfirmDestructive`, minus the controlled bits. */
export interface PullStrategyAsk {
  readonly title: string
  /** Ignored while `choices` is set; present because `ConfirmState` requires it. */
  readonly body: string
  /** `↻`, never `−`: nothing is removed under either answer. */
  readonly mark: string
  /** `false` — these entries are commits, not paths. See `ConfirmState.split`. */
  readonly split: false
  readonly choices: readonly ConfirmChoiceLike[]
}

/**
 * Unpack a `pullNeedsStrategy` rejection, or answer `null` for anything else.
 *
 * The exact shape of `branchModel.refusalOf`, which is this codebase's established pattern for
 * *a typed refusal the UI turns into a panel rather than printing*.
 *
 * Every read is guarded, because this runs inside a `catch` and the value can be anything a
 * rejected promise carries — `null`, a string, a number, an object with no `kind`. A catch
 * block that itself threw would turn a recoverable question into an unhandled rejection, which
 * is the one failure this whole surface exists to prevent.
 */
export function divergenceOf(error: unknown): DivergenceLike | null {
  if (error === null || typeof error !== 'object') return null
  const kind = (error as { kind?: unknown }).kind
  if (kind !== 'pullNeedsStrategy') return null
  const detail = (error as { detail?: unknown }).detail
  if (detail === null || typeof detail !== 'object') return null
  const d = detail as Record<string, unknown>
  const branch = str(d['branch'])
  const ahead = num(d['ahead'])
  const behind = num(d['behind'])
  if (branch === null || ahead === null || behind === null) return null
  return {
    branch,
    remote: str(d['remote']) ?? 'the remote',
    upstream: str(d['upstream']) ?? 'its upstream',
    ahead,
    behind,
    incoming: commitList(d['incoming']),
    moreIncoming: num(d['moreIncoming']) ?? 0,
    local: commitList(d['local']),
    moreLocal: num(d['moreLocal']) ?? 0,
  }
}

/**
 * The dialog for every repository that asked, or `null` when there is nothing to ask about.
 *
 * `null` for an empty list, and entries with `ahead === 0` are dropped first — which is
 * `ConfirmDestructive`'s rule 2 (*it only appears when something is at risk*) made
 * unrepresentable rather than merely obeyed. A branch that is not actually ahead cannot need
 * this question: a fast-forward would have taken it.
 */
export function strategyAsk(repos: readonly RepoDivergence[]): PullStrategyAsk | null {
  const asking = repos.filter((r) => r.diverged.ahead > 0)
  const first = asking[0]
  if (first === undefined) return null
  const many = asking.length > 1

  const title = many
    ? `${asking.length} repositories have diverged from their upstreams`
    : `${first.diverged.branch} has diverged from ${first.diverged.upstream}`

  const counts = many
    ? asking
        .map((r) => `${r.name}: ${r.diverged.ahead} ahead, ${r.diverged.behind} behind`)
        .join(' · ')
    : `${commits(first.diverged.ahead)} of yours, ${commits(first.diverged.behind)} of theirs.`

  return {
    title,
    body: counts,
    mark: '↻',
    split: false,
    choices: [
      {
        id: 'merge',
        label: 'Merge',
        body: `${counts} Merging keeps both histories and adds a merge commit. Nothing you have already committed is rewritten, and \`git reset --hard ORIG_HEAD\` undoes it exactly.`,
        // Empty for one repository, and the emptiness *is* the argument — the same thing
        // `resetChoices`' `soft` arm says by listing nothing. With several, the list names
        // which repositories this one answer covers, which is what makes a single answer for
        // all of them honest.
        files: many ? asking.map((r) => `${r.name}: ${r.diverged.branch}`) : [],
        confirmLabel: many ? `Merge in ${asking.length} repositories` : 'Merge',
        danger: false,
      },
      {
        id: 'rebase',
        label: 'Rebase',
        // Your commits, because they are what a rebase puts at risk. Rule 1 holds *per choice*,
        // and listing the incoming commits here would name things that are at risk under
        // neither answer.
        body: `${counts} Rebasing replays your commits on top of theirs, giving each of them a new identity. If this branch is already pushed, the next push will be refused.`,
        files: rewrittenList(asking, many),
        confirmLabel: many ? `Rebase in ${asking.length} repositories` : 'Rebase',
        danger: true,
      },
    ],
  }
}

/** The commits a rebase would rewrite, one line each, labelled by repository when there are several. */
function rewrittenList(asking: readonly RepoDivergence[], many: boolean): readonly string[] {
  const out: string[] = []
  for (const repo of asking) {
    const prefix = many ? `${repo.name}: ` : ''
    for (const commit of repo.diverged.local) {
      const summary = commit.summary === '' ? '(no summary)' : commit.summary
      const author = commit.author === '' ? '' : ` — ${commit.author}`
      out.push(`${prefix}${commit.shortOid}  ${summary}${author}`)
    }
    // The list is capped in Rust (`PULL_COMMIT_CAP`), so the overflow line reports a number this
    // side never had the rows for. Deliberate: the alternative was putting four hundred commits
    // on the wire to drop three hundred and ninety here.
    if (repo.diverged.moreLocal > 0) {
      out.push(`${prefix}… and ${commits(repo.diverged.moreLocal)} more`)
    }
  }
  return out
}

/**
 * The radio's id as a strategy.
 *
 * Anything unrecognised — **including the `null` the dialog opens with** — resolves to `merge`,
 * which is `logMenu.ts`'s *"anything unrecognised is the safe one"* and is the same fallback
 * `ConfirmDestructive` applies to the radio itself. The two have to agree or the button would
 * do something the selected radio does not say.
 */
export function strategyOf(id: string | null | undefined): PullStrategy {
  return id === 'rebase' ? 'rebase' : 'merge'
}

/**
 * The checkbox's wording, which changes with the radio.
 *
 * It names the act rather than saying *"remember this choice"*, because what is about to be
 * made permanent is a line in a config file the user will not think to look for. `LogTab`'s
 * `optionFor` sets the precedent for an option label that follows the selection.
 */
export function rememberLabel(
  repos: readonly RepoDivergence[],
  chosen: string | null | undefined,
): string {
  const asking = repos.filter((r) => r.diverged.ahead > 0)
  const verb = strategyOf(chosen)
  const where =
    asking.length > 1
      ? `in these ${asking.length} repositories`
      : `in ${asking[0]?.name ?? 'this repository'}`
  return `Always ${verb} ${where} (git config pull.rebase)`
}

// --- helpers ---------------------------------------------------------------------------------

function str(value: unknown): string | null {
  return typeof value === 'string' ? value : null
}

function num(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function commitList(value: unknown): readonly DivergedCommit[] {
  if (!Array.isArray(value)) return []
  const out: DivergedCommit[] = []
  for (const entry of value) {
    if (entry === null || typeof entry !== 'object') continue
    const e = entry as Record<string, unknown>
    out.push({
      shortOid: str(e['shortOid']) ?? '',
      summary: str(e['summary']) ?? '',
      author: str(e['author']) ?? '',
    })
  }
  return out
}

/**
 * `1 commit` / `4 commits`.
 *
 * Duplicated from `branchModel.ts` rather than imported, and the duplication is the point: see
 * this file's header, and the identical note at the foot of `chrome/logActions.ts`. A value
 * import — even from another import-free module — is what stops the check compiling this file
 * on its own.
 */
function commits(n: number): string {
  return `${n} ${n === 1 ? 'commit' : 'commits'}`
}
