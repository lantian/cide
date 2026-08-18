/**
 * The two gestures the Problems panel can start, as one function each. (M18)
 *
 * # Why these are not written inline at their call sites
 *
 * Each of them has two call sites and always will: the panel's own button, and a command id in
 * `ui/src/keys/dispatch.ts` that the palette and any user binding reach. Writing the `invoke`
 * twice is how the button and the palette row drift into doing subtly different things — one of
 * them reporting the outcome, the other swallowing it — and a user cannot tell which they used.
 *
 * # Why they report
 *
 * Both spend their effect in another process over the following seconds, so on screen they are
 * indistinguishable from a control wired to nothing unless they say something. That is not a
 * hypothetical in this codebase: `diagnostics.restart` shipped in M12 fully implemented, fully
 * tested, registered in `contract/commands.json` and wrapped in `client.ts`, and had **no caller
 * anywhere in the app** until this file. The sentence a refresh returns is built in Rust, where
 * the facts are — which servers are running, and whether any is — for the same reason
 * `cmd::file::not_connected`'s is.
 *
 * Kept out of `model.ts`, which imports nothing at all so that `check-problems.mjs` can compile it
 * standalone under `tsc`. Anything that touches IPC belongs here.
 */
import { notify, notifyFailure } from '@/chrome/notices'
import { diagnostics as diagnosticsApi, type DiagnosticSourceId, type ProjectId } from '@/ipc/client'

/**
 * Ask every running analyser to check this project again.
 *
 * Fire-and-forget; the outcome is a notice either way. The `info` notice is deliberate rather than
 * a silence: a re-run that finds the same problems changes *nothing* on screen, and without a line
 * saying it ran, a user who presses this because their diagnostics look stale learns that the
 * button does nothing.
 */
export function refreshDiagnostics(project: ProjectId): void {
  void diagnosticsApi
    .refresh(project)
    // The sentence is the server-side one: it names the analysers that were kicked, or says
    // plainly that none is running. Inventing a cheerful one here would mean claiming a re-run
    // happened in a project that has no language server at all.
    .then((sentence) => notify(sentence, { kind: 'info' }))
    .catch(notifyFailure)
}

/**
 * Stop one analyser and start it again.
 *
 * The heavier of the two, and the one that recovers a source the supervisor gave up on after three
 * crashes — which is what makes that give-up rule tolerable. The notice goes out *before* the
 * await, because a rust-analyzer restart on a large workspace is minutes of re-indexing and the
 * panel's own row will sit at `Starting…` for all of it.
 */
export function restartDiagnosticSource(project: ProjectId, source: DiagnosticSourceId): void {
  notify(`Restarting ${source === 'rustAnalyzer' ? 'rust-analyzer' : source}. It re-indexes from scratch, which can take a while.`, {
    kind: 'info',
  })
  void diagnosticsApi.restart(project, source).catch(notifyFailure)
}

/**
 * Is this string one of the source ids `restart` accepts?
 *
 * The panel's `SourceRow.id` is a plain `string` — `model.ts` cannot import the wire enum, because
 * it imports nothing — so the narrowing happens here, at the one place the two meet. A membership
 * test and not a cast: the value originates in another process, and a cast would be a promise
 * about it that nothing checks.
 */
export function isDiagnosticSourceId(value: string): value is DiagnosticSourceId {
  return value === 'rustAnalyzer' || value === 'gopls' || value === 'treeSitter' || value === 'claude'
}
