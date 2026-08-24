/**
 * The context flags `when` clauses are evaluated against — the half that any module can
 * compute, merged under the half only React knows.
 *
 * # The bug this exists for
 *
 * `cide_core::commands` gated every git command on `repoOpen`, and nothing in the frontend
 * ever set `repoOpen`. An unknown identifier evaluates to `false` (see `when.ts`, and it is
 * the right default), so all four git rows were filtered out of the palette on every
 * platform and their keys resolved to nothing — with no error anywhere, because a flag that
 * is never set looks exactly like a flag that is currently false. The context was assembled
 * in `App.tsx` from what that component happened to have in scope, so the vocabulary the
 * registry may use was, in effect, "whatever App.tsx remembered".
 *
 * So the derivable flags are computed here, from the workspace mirror, and
 * `ui/scripts/check-commands.mjs` asserts that every flag in `CONTEXT_FLAGS` (the Rust
 * vocabulary) is either derived here or listed in [`HOST_FLAGS`]. A clause naming a flag
 * nobody supplies now fails the build.
 *
 * # And the second, worse version of the same bug, which that fix did not catch
 *
 * `repoOpen` was then *supplied* — from `reposOf`, which read `ProjectRoot.repo`, a mirror
 * field Rust's one constructor sets to `None` and no code anywhere sets to anything else. So
 * the flag went from unsupplied-and-false to supplied-and-false, the gate above went green,
 * and the entire Git group stayed invisible in the palette for exactly the same reason as
 * before. A check that a flag is *named* by a supplier cannot see that the supplier is a
 * constant.
 *
 * There is no `repoOpen` now. The lesson it left is the rule this module is held to: a flag
 * here must be derivable from the mirror **as Rust actually fills it**, and a fact that only
 * the disk knows is not one of those — it is asked over IPC by the handler that needs it, at
 * the moment it needs it. `check-commands.mjs` now also asserts that its own bootstrap
 * fixture carries no field the generated DTO does not have, because a hand-written fixture
 * shaped like a DTO Rust never produces is what kept this green.
 *
 * # One owner per flag
 *
 * The split is by *kind of fact*, not by who happens to know it: [`DERIVED_FLAGS`] are
 * properties of the workspace mirror and are computed here; [`HOST_FLAGS`] are transient
 * chrome and come from the host. The host's object is the base and the derived flags are
 * spread over it, so where a host also computes a mirror fact, this one wins.
 *
 * That direction is not arbitrary — the other way round is a bug that was live. `App.tsx`
 * spells `terminalFocused` as `overlay === null && focused?.pane.kind !== 'editor'`, folding
 * the overlay state into a pane-kind flag. The command palette is an overlay, so with the
 * host winning, `terminalFocused` is false for as long as the palette is open and *Clear
 * terminal* and *Paste into terminal* can never be listed in it. Two flags describing two
 * different things is the fix, and `overlayOpen` is right there for any clause that wants it.
 *
 * Both consumers go through [`mergeContext`] — the key gate in `useKeyGate.ts` and the
 * palette in `overlays/CommandPalette.tsx` — so a command a key can reach is exactly a
 * command the palette offers. Two derivations would be two answers.
 */
import { useWorkspace } from '@/store/workspace'
import { registeredBuffers } from '@/editor/openBuffers'
import {
  activeProjectOf,
  claudeTargetOf,
  focusTarget,
  focusedTabPath,
  isClosableTab,
  windowProjectsOf,
} from './target'
import type { KeyContext } from './when'
import type { Bootstrap } from '@/ipc/client'

export type { KeyContext }

/**
 * Flags the host supplies because nothing else can: they are transient window chrome that
 * lives in React state and never reaches the workspace mirror.
 *
 * Listed as data rather than left implicit so `check-commands.mjs` can tell "supplied
 * elsewhere" apart from "supplied by nobody", which is the failure mode above.
 */
export const HOST_FLAGS = [
  'overlayOpen',
  'filePickerOpen',
  'contextMenuOpen',
  'sidebarFiles',
  'sidebarGit',
  'diffFocused',
] as const

/** Flags [`deriveContext`] computes. Every one is a fact about the workspace mirror. */
export const DERIVED_FLAGS = [
  'paneFocused',
  'claudePaneFocused',
  'terminalFocused',
  'editorFocused',
  'projectOpen',
  'multipleProjects',
  'multipleTabs',
  'closableTab',
  'editorOpen',
  'fileTabActive',
  'claudeTarget',
  'shellWindow',
] as const

/**
 * Derive every flag this module owns from one snapshot.
 *
 * Recomputed on each keystroke rather than memoised: it is a dozen property reads over a
 * mirror that is already in memory, and a stale context is how a binding fires in a state
 * its clause excludes — which costs more than the arithmetic saves. `App.tsx` says the same
 * thing about its own half.
 */
export function deriveContext(boot: Bootstrap | null): KeyContext {
  const project = activeProjectOf(boot)
  const focused = focusTarget(boot)
  const kind = focused?.pane.kind

  return {
    paneFocused: focused !== null,
    claudePaneFocused: kind === 'claude',
    // A Claude pane is an xterm too, so "a terminal has focus" is true for both — and false
    // for a diff pane, which has no terminal at all and which `App.tsx`'s
    // `kind !== 'editor'` spelling counted as one.
    terminalFocused: kind === 'claude' || kind === 'shell',
    editorFocused: kind === 'editor',

    projectOpen: project !== null,
    // This window's strip, not the workspace's project map: in `perProject` window mode the
    // map holds every project and the strip holds one, and Ctrl+Tab can only reach what the
    // strip holds. See `windowProjectsOf`.
    multipleProjects: windowProjectsOf(boot).length > 1,
    multipleTabs: (project?.tabs.length ?? 0) > 1,
    // `tabs[0]` is the pinned console and `close_tab` refuses it — see
    // `cide_core::workspace::close_tab`. Ctrl+W there is a refusal, not a close, so the
    // palette does not offer the row; `tab.close`'s handler enforces the same fact through
    // the same function, because the gate never reads a `Command::when`.
    closableTab: isClosableTab(boot),
    // Asked of the live registry, not of the tree: `file.saveAll` writes through the savers
    // `EditorPane` registers on mount, so "a file tab exists somewhere" is not the same
    // claim as "something here can save one".
    editorOpen: registeredBuffers().length > 0,
    // "the tab this window shows is about a file", which is **not** `editorFocused`: that one
    // is about the focused *pane*, and a file tab split with a shell pane is still a file tab.
    // `file.reveal` was gated on the pane flag and so was hidden from the palette in exactly
    // that arrangement — a command that works, filtered out of the only list that offers it.
    //
    // Derived from `focusedTabPath`, the same function the handler and the Explorer's button
    // call, so the row the palette offers is offered exactly when the button is enabled.
    fileTabActive: focusedTabPath(boot) !== null,
    claudeTarget: claudeTargetOf(boot) !== null,
    // There is no `repoOpen`, and its absence is deliberate — see the note at the foot of
    // `target.ts`. It was derived from `ProjectRoot.repo`, which Rust never filled, so it was
    // false for every user of every build and it hid the entire Git group from the palette.
    // Every git command is gated on `projectOpen` now and asks `git_repos` when it runs.
    shellWindow: boot?.role.kind === 'shell',
  }
}

/**
 * The whole context: the host's chrome flags, with the mirror's facts over the top.
 *
 * Kept a plain function so the key gate — which runs outside React — and the palette can
 * both call it with the same arguments and get the same answer.
 */
export function mergeContext(host: KeyContext): KeyContext {
  return { ...host, ...deriveContext(useWorkspace.getState().boot) }
}

/**
 * `mergeContext` for a React consumer, subscribed so the palette re-filters when the
 * workspace moves under it.
 *
 * Subscribing to `boot` — one reference — rather than to a computed object: zustand compares
 * selector results with `Object.is`, so a selector returning a fresh record would re-render
 * on every notification for ever.
 */
export function useMergedContext(host: KeyContext): KeyContext {
  const boot = useWorkspace((s) => s.boot)
  return { ...host, ...deriveContext(boot) }
}
