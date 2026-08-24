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
 * Fire-and-forget; the outcome is a notice either way. The notice is deliberate rather than a
 * silence: a re-run that finds the same problems changes *nothing* on screen, and without a line
 * saying it ran, a user who presses this because their diagnostics look stale learns that the
 * button does nothing.
 */
export function refreshDiagnostics(project: ProjectId): void {
  void diagnosticsApi
    .refresh(project)
    // The sentence is the server-side one: it names the analysers that were kicked, or says
    // plainly that none is running. Inventing a cheerful one here would mean claiming a re-run
    // happened in a project that has no language server at all.
    .then((sentence) => notify(sentence, { kind: 'ok' }))
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
export function restartDiagnosticSource(
  project: ProjectId,
  source: DiagnosticSourceId,
  label?: string,
): void {
  // The row's own label, which came from Rust with the report. It used to be a ternary special-
  // casing `rustAnalyzer`, which was the whole set of ids whose id and label differ — until an
  // extension could contribute a server, and a notice reading "Restarting sqls" happened to be
  // right while "Restarting rustAnalyzer" would have come back the moment anything else needed a
  // display name. Passing the label removes the guess.
  //
  // `ok`, and the wait is a `hint`: the restart is a gesture that worked, and the minutes of
  // re-indexing behind it are the thing the user has to know rather than the outcome itself.
  notify(`Restarting ${label ?? source}.`, {
    kind: 'ok',
    hint: 'It re-indexes from scratch, which can take a while.',
  })
  void diagnosticsApi.restart(project, source).catch(notifyFailure)
}

/**
 * Is this string a source id `restart` can act on?
 *
 * # Why this stopped being a membership test
 *
 * It was `=== 'rustAnalyzer' || === 'gopls' || …`, which was a complete list right up until an
 * extension could contribute a language server. `sqls` then reported findings into the panel, the
 * footer drew no Restart for it, and — the half that would have survived fixing only the footer —
 * this guard would have swallowed the click in silence. A closed list on an open set fails by
 * doing nothing, which is the failure mode with no symptom.
 *
 * `DiagnosticSourceId` is a `string` on the wire now (`cide_ipc::diagnostics` says why at length),
 * so there is no enum left to narrow to and the honest test is the one property the id must have:
 * it is not empty. The *set* is guarded where it can be — `sourceRows` decides which rows offer a
 * Restart at all, and Rust refuses an id that names no running server.
 */
export function isDiagnosticSourceId(value: string): value is DiagnosticSourceId {
  return value !== ''
}
