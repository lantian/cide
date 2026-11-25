/**
 * Which tree a surface shows, and which events it adopts, now that one choice of agent checkout
 * drives the Explorer, the Git panel and the Log together. (M207)
 *
 * The rules are small and every one has a wrong version that type-checks and looks fine on a
 * screen until the moment it matters: a project tree that repaints on a checkout's watcher, a
 * Log that keeps walking the project after the Explorer moved to a checkout, a choice silently
 * dropped by a consumer whose list was older than the checkout the user just picked. So they
 * live here, pure and **import-free** — the same arrangement as `sidebar/groupRows.ts` and
 * `gitlog/branchFilter.ts` — and `ui/scripts/check-worktree-scope.mjs` compiles this file
 * standalone and drives it. Do not add an import.
 *
 * A *scope* is the checkout's path (`WorktreeInfo.repo.root`) or `null` for the project's own
 * tree. A *tag* is the `worktree` a `cide://fs-changed` / `cide://fs-status` /
 * `cide://git-status` payload carries: the checkout whose watcher (or mutation) produced it, or
 * `null` for the project's.
 */

/**
 * Does a listener scoped to `scope` adopt a burst tagged `tag`?
 *
 * Exactly when they name the same tree. The project's tree must not re-read on a checkout's
 * watcher — an agent's `cargo build` in its checkout would revalidate the user's tree once per
 * burst — and a checkout's tree must not adopt the project's, whose paths it does not hold.
 */
export function adoptsBurst(scope: string | null, tag: string | null): boolean {
  return scope === tag
}

/**
 * Does a listener that mirrors the **project** — its branch readout, its change count, the
 * worktree list itself — care about a burst tagged `tag`?
 *
 * Only an untagged one. A checkout's watcher cannot move the project's branch, cannot change
 * its count, and cannot add or remove a checkout (the project's own watcher sees
 * `.cide/worktrees/<name>` appear and go).
 */
export function projectHears(tag: string | null): boolean {
  return tag === null
}

/**
 * Does a surface showing `scope` hear a burst tagged `tag`, when that surface reads **git**
 * state rather than a file tree — the Git panel's changes, the Log's history?
 *
 * Untagged bursts always, plus its own checkout's. A checkout shares the project's refs and
 * object database, and the project's watcher is the one that sees `refs/heads/cide/<name>`
 * move when the checkout commits — whether or not the Explorer has a view of it open, which is
 * the only thing that produces tagged bursts. The checkout's own `HEAD` and `index`
 * (`<common>/worktrees/<name>/`) are seen only by that view's watcher, tagged. A project-scoped
 * surface (`scope === null`) hears untagged bursts only: a checkout's edits cannot change the
 * project's status.
 */
export function checkoutHears(scope: string | null, tag: string | null): boolean {
  return tag === null || tag === scope
}

/**
 * The shared choice after the checkout list was re-read.
 *
 * Kept while the list still names it, dropped to the project once it does not — Integrate
 * removed the checkout, or somebody deleted it — because every command would refuse its path
 * from then on. A list that could not be read (`null`) judges nothing: the choice stands.
 *
 * This is judged against the **one shared list** and nowhere else. With a list per consumer, a
 * consumer that loaded before a new checkout existed would see the user's fresh choice as stale
 * and drop it for everyone; that is the race the shared store exists to remove.
 */
export function keptChoice(
  chosen: string | null,
  listed: readonly { repo: { root: string } }[] | null,
): string | null {
  if (chosen === null || listed === null) return chosen
  return listed.some((wt) => wt.repo.root === chosen) ? chosen : null
}

/** The Log tab's scope when the shared choice is a checkout: that checkout's repository alone. */
export interface OneRepoScope {
  kind: 'one'
  repo: string
}

/**
 * The scope the Log tab walks for the shared choice, or `null` to fall through.
 *
 * A **Log tab** only: a History tab is one file in one repository already, and the checkout
 * choice does not re-point it. `git_log`'s `One` scope resolves a checkout's id through
 * `cide_git::worktree::find`, so *Current branch* then walks the checkout's `HEAD`.
 */
export function worktreeLogScope(
  logTab: boolean,
  current: { repo: { id: string } } | null,
): OneRepoScope | null {
  if (!logTab || current === null) return null
  return { kind: 'one', repo: current.repo.id }
}

/**
 * The Log's scope chain. A History tab's own repository wins, then a filter's (*View commits*,
 * folder history), then the shared checkout choice, then the project's repositories merged.
 *
 * A filter outranks the checkout and does **not** clear it: the choice is shared with the
 * Explorer and the Git panel, and a filter typed into the Log must not move either of them.
 */
export function logScopeChain<S>(
  one: S | null,
  pinned: S | null,
  worktree: S | null,
  merged: S | null,
): S | null {
  return one ?? pinned ?? worktree ?? merged
}
