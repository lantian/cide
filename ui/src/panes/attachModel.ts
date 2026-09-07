/**
 * The attach-time decisions of `sessionSink.ts`, as pure rules a check script can run.
 *
 * Deliberately import-free: `check-attach.mjs` compiles this file standalone with the
 * TypeScript in `node_modules` and replays every branch, the same arrangement as
 * `renderStall.ts` / `check-render-stall.mjs`. Keep it that way.
 *
 * Two decisions live here, and each one shipped as a bug before it was a rule:
 *
 * * **What to do with the snapshot** `session_attach` returns. Writing it when the terminal
 *   already holds those bytes appends a second copy of the transcript (the split-remount
 *   bug); *not* writing it when the terminal missed a stretch of output shows a stale
 *   screen with no sign anything is missing (the frozen-after-project-switch bug — the sink
 *   used to die with the React mount while `hydrated` stayed true, so the recovery snapshot
 *   was thrown away every time).
 * * **When a live frame may be written.** The sink is registered on the Rust side the moment
 *   `session_attach` runs, so the channel can deliver frames while the command's own reply —
 *   the screen those frames continue from — is still in flight. Painting them in arrival
 *   order draws the continuation and then draws the screen it continues from on top of it.
 */

/** What the terminal already holds, against what the mirror snapshot says. */
export interface AttachState {
  /** This terminal already holds the mirror's bytes (see `PaneHost.hydrated`). */
  hydrated: boolean
  /** A released host coming back: the snapshot must replace, never append. */
  needsReset: boolean
  /** xterm's active buffer is the alternate one. */
  termOnAlt: boolean
  /** `session_in_alternate_screen`'s answer, asked after the attach. */
  mirrorOnAlt: boolean
  /** The snapshot carries no bytes at all. */
  snapshotEmpty: boolean
  /**
   * An evicted host's serialized buffer is waiting to be replayed (`paneHosts`' ledger).
   *
   * The mirror is one screen, so an evicted-and-recreated pane used to come back with no
   * scrollback at all — eviction silently cost the user their history. The serialized
   * buffer is taken at eviction time and replayed exactly once, on the fresh host's first
   * hydration.
   */
  savedScrollback: boolean
}

export interface HydrationPlan {
  /**
   * The snapshot phase runs: `needsReset` is consumed, the bytes are written when there
   * are any, and `hydrated` becomes true afterwards.
   */
  hydrate: boolean
  /** `term.reset()` first — a released host replaces its stale screen, never appends. */
  reset: boolean
  /**
   * The serialized pre-eviction buffer is written before the snapshot, restoring the
   * scrollback eviction dropped. The snapshot then opens with a clear-screen
   * (`vt100::write_contents_formatted` leads with one), which erases the replayed
   * *viewport* in place and paints the current screen — so the history above the fold
   * survives and the live rows are the mirror's, with no duplicated screenful between
   * them. What the clear erases is the eviction-time viewport, which in the common
   * nothing-happened-while-evicted case is exactly what the snapshot repaints.
   */
  replayScrollback: boolean
  /** The snapshot bytes are actually written (skipped when empty — nothing to paint). */
  writeSnapshot: boolean
  /**
   * A hydrated terminal was on the wrong buffer — a `\x1b[?1049h/l` the pane was not
   * attached to hear — so `hydrated` is dropped and the snapshot repaints. The caller owes
   * a diagnostic line when this fires; silence is the evidence the two halves agree.
   */
  dropHydratedForAltDrift: boolean
}

/**
 * Decide what the attach path does with the screen snapshot.
 *
 * `hydrate` is "exactly once per host, plus the drift repair": a terminal that has never
 * read the mirror reads it now, and a hydrated one reads it again only when the buffers
 * have provably diverged. `reset` rides on `needsReset` alone — a restarted pane keeps its
 * predecessor's transcript on screen (the flag is deliberately not set there), a released
 * one must not show its pre-detach screen followed by a second copy of the current one.
 *
 * The replay is confined to a host's **first** hydration (`!s.hydrated`, not merely
 * `hydrate`): the drift repair re-runs the phase on a terminal that already holds a
 * transcript, and replaying the saved buffer there would prepend a second copy of
 * everything — the split-remount duplication bug, rebuilt out of the fix for eviction.
 */
export function hydrationPlan(s: AttachState): HydrationPlan {
  const dropHydratedForAltDrift = s.hydrated && s.mirrorOnAlt !== s.termOnAlt
  const hydrate = !s.hydrated || dropHydratedForAltDrift
  return {
    hydrate,
    reset: hydrate && s.needsReset,
    replayScrollback: hydrate && !s.hydrated && s.savedScrollback,
    writeSnapshot: hydrate && !s.snapshotEmpty,
    dropHydratedForAltDrift,
  }
}

/**
 * Whether the attach should ask Rust for the mirror's retained scrollback in front of the
 * screen. (M42)
 *
 * Only a host that holds **no** transcript of its own: one that has never hydrated and has no
 * evicted buffer parked in the ledger. Both of the others already carry history — a hydrated
 * host refuses the snapshot outright, and an evicted one replays its own serialized scrollback
 * before the snapshot — and handing either of them the mirror's copy as well would paint the
 * transcript twice, which is the split-remount duplication bug in a new coat. Asked *before*
 * the attach, because the answer rides on the request; `hydrationPlan` is asked after and
 * decides what to do with what came back.
 */
export function historyRequest(s: { hydrated: boolean; savedScrollback: boolean }): boolean {
  return !s.hydrated && !s.savedScrollback
}

/**
 * The first-frame ordering: which live frames must wait for the snapshot.
 *
 * Frames are queued rather than dropped because they are already charged against this
 * sink's credit on the Rust side, and only delivery pays that back — a dropped frame is a
 * permanently poorer credit line, and enough of them is a choked sink.
 */
export interface AttachSequencer {
  /** A channel frame arrived. `queue` means hold it; its turn comes from `onSnapshotWritten`. */
  onFrame(seq: number): 'deliver' | 'queue'
  /** The snapshot is in the buffer. Returns the held frames, in arrival order, to deliver now. */
  onSnapshotWritten(): number[]
}

export function attachSequencer(): AttachSequencer {
  let painted = false
  const queued: number[] = []
  return {
    onFrame(seq: number): 'deliver' | 'queue' {
      if (painted) return 'deliver'
      queued.push(seq)
      return 'queue'
    },
    onSnapshotWritten(): number[] {
      painted = true
      const held = queued.slice()
      queued.length = 0
      return held
    },
  }
}
