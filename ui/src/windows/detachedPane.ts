/**
 * What a `pane:<uuid>` window can show, and what it must refuse to show.
 *
 * One rule, pulled out of `DetachedPaneWindow.tsx` because it was wrong there in a way that
 * only a check script could have caught: the component asked `needsSession(pane)`, answered
 * `false` for an `editor` pane — correctly, an editor runs no child — and then rendered
 * `TerminalPane` anyway, because that was the only other branch. A detached editor pane would
 * therefore have **spawned a shell in a window titled `main.rs`**.
 *
 * That is "implemented and mis-wired", not "not implemented", and it is not unreachable: it is
 * only out of reach because `layout::take_pane` refuses the last pane of a tab and a file tab
 * opens with one. Split a file tab first — which `App.tsx` offers on the pane's own menu — and
 * Detach is right there.
 *
 * # Why an editor is refused rather than rendered
 *
 * `EditorPane` needs a `TabId`: the buffer is registered per tab in `editor/openBuffers.ts`, and
 * that is what `file.save`, the dirty dot and the status readout all key on. A detached window
 * holds a *pane*, not a tab — `Project.detached` is a map of panes, and the pane it holds has
 * been taken out of its tab's tree. Inventing a tab id here would give the file two buffers and
 * whichever saved second would silently discard the other's edits, which is the exact failure
 * `cmd::file::open_file_tab` reuses tabs to prevent. Floating an editor into its own window is
 * therefore the *tab's* move, not the pane's: `window_detach_tab` tears the whole tab out with
 * its id, so the one buffer travels with it — see `windows/windowTabs.ts` — and the refusal
 * below now names that road instead of a bare "yet".
 *
 * Import-free on purpose: `ui/scripts/check-detached.mjs` compiles this file on its own and runs
 * the table below. [`PaneLike`] is a structural subset of the generated `Pane`, declared rather
 * than imported, and TypeScript checks the real one against it at the call site.
 */

/** The part of the generated `Pane` this rule reads. */
export interface PaneLike {
  /** `claude` | `shell` | `diff` | `editor`. A string, so an unknown kind is representable. */
  readonly kind: string
  /** The child this pane is attached to, or `null` if it has none. */
  readonly session: string | null
}

/** What the window should put inside its `PaneFrame`. */
export type DetachedContent =
  | { readonly kind: 'terminal' }
  /** The pane runs a child and arrived without one. Nothing to attach to. */
  | { readonly kind: 'orphaned'; readonly message: string }
  /** This window cannot host this kind of pane at all. */
  | { readonly kind: 'unsupported'; readonly message: string }

/**
 * Whether this kind runs a child, and must therefore reach this window with one already
 * running.
 *
 * Written as the list of kinds that run *nothing*, so a `PaneKind` added later falls into the
 * default and is treated as spawning. The window then refuses to render rather than letting
 * `TerminalPane` start a second child — the wrong answer in that direction is a pane that says
 * it has no session, and in the other it is a duplicated conversation and a doubled bill.
 */
export function needsSession(pane: PaneLike): boolean {
  return pane.kind !== 'diff' && pane.kind !== 'editor'
}

/**
 * The one decision this window makes about its pane.
 *
 * Order matters and is the opposite of the obvious one: **unsupported is checked first**. A
 * `diff` or `editor` pane has no session by design, so asking "is it orphaned?" first would
 * answer "this pane has no session, redock it to start one" — which is both wrong (it will
 * never have one) and an instruction to press a button that fixes nothing.
 */
export function detachedContent(pane: PaneLike): DetachedContent {
  if (!needsSession(pane)) {
    return {
      kind: 'unsupported',
      message:
        pane.kind === 'editor'
          ? 'A lone file pane cannot live in a window of its own — Redock it, then detach the whole tab instead.'
          : 'A diff cannot be shown in a window of its own. Redock this pane to get it back.',
    }
  }
  if (pane.session === null) {
    return { kind: 'orphaned', message: 'This pane has no session. Redock it to start one.' }
  }
  return { kind: 'terminal' }
}

/*
 * Runtime values only, no imports — see the header. The same note is at the foot of
 * `terminal/pathMatch.ts`, `terminal/outsideOpen.ts` and `chrome/notices.ts`.
 */
