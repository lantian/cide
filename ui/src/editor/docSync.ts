/**
 * Document sync: keeping a language server's copy of a file matching the user's buffer.
 *
 * # Why this is not optional
 *
 * Without it the feature looks built and is nearly inert, which is this repository's named worst
 * failure mode. Measured, not assumed — `cide-lsp`'s
 * `an_on_disk_edit_alone_never_refreshes_diagnostics` (the test was renamed; this reference named
 * a function that no longer existed) starts a real rust-analyzer over a broken crate, waits for
 * the error, repairs the file **on disk**, and waits two minutes:
 *
 * * the initial error **does** arrive — flycheck runs once when the workspace finishes loading, so
 *   a project opens with a correct list and the panel looks like it works;
 * * the fix is **never** picked up. We declare no `didChangeWatchedFiles`, and rust-analyzer's own
 *   fallback watcher does not re-run flycheck on its own.
 *
 * So without these notifications the panel shows the state of the tree at the moment the project
 * opened, for ever, while the user edits underneath it. Stale diagnostics are worse than none: the
 * user fixes an error, the row stays, and the language server takes the blame.
 *
 * `did_save` is what re-triggers flycheck, and `did_change` is what buys the thing an editor is
 * supposed to have over a terminal running `cargo check` — errors on *unsaved* text.
 *
 * # What M18 added, and what it deliberately did not add here
 *
 * Everything above is about the buffer the **user** has open. It says nothing about the file
 * Claude just rewrote in a tab nobody opened, which is most of what happens in this editor — and
 * that half was reaching no server at all. It now goes through Rust instead:
 * `cide_app::files::on_watch_event` → `FsEvents::files_changed` →
 * `ProjectDiagnostics::files_changed`, which sends `workspace/didChangeWatchedFiles` to gopls and
 * a debounced `rust-analyzer/runFlycheck` to rust-analyzer. Both are proven against the real
 * binaries by `crates/cide-lsp/tests/real_servers.rs`.
 *
 * Note what that means for this module: **do not add a `didSave` to `resetDoc`.** An agent's write
 * reaches the watcher whether or not a pane happened to have the file open, so a save synthesised
 * here would be a *second* signal for the same edit — a duplicate flycheck for every agent write
 * to an open file. The one place a save is sent stays `savedDoc`, from a real `file.write`.
 *
 * # Keyed by path, refcounted
 *
 * `EditorPane`'s own header describes the split case: two panes over one file are two buffers.
 * They are **one document** to a language server, and `didOpen` on an already-open URI is a
 * protocol violation — rust-analyzer would be holding two versions of one file with no way to say
 * which won. So the count is per path, the first pane opens, the last one closes, and the middle
 * ones do nothing. This is the same keying `revealRequest.ts` uses, for the same reason.
 *
 * # Every language, not just the ones we parse
 *
 * No extension gate here. `notify_document` in `cmd/diagnostics.rs` resolves the server by
 * language and drops what nothing handles, and that registry is the only place that knows which
 * servers are actually running. A second copy of the mapping in TypeScript would be a second thing
 * to keep in step, and it would be the copy that is wrong when a server is added.
 */
import { diagnostics as diagnosticsApi, type ProjectId } from '@/ipc/client'

/**
 * How long a burst of typing is allowed to collect before the server hears about it.
 *
 * A **trailing throttle, not a debounce**, and the distinction is the same one `gitStatusStore.ts`
 * argues at its own timer: a restarting debounce is starved by continuous input, so holding a key
 * down — or a long paste followed by more typing — would keep pushing the send out and the server
 * would see nothing at all for as long as the user kept going. A throttle sends on a fixed cadence
 * while the burst lasts, which is what "keep up with the buffer" has to mean. `cmd/diagnostics.rs`
 * says "debounced at 300 ms"; the interval is what it asked for, the edge is the safer one.
 */
const SYNC_MS = 300

interface Doc {
  project: ProjectId
  /** How many panes have this file open. `didClose` goes when it reaches zero. */
  refs: number
  /**
   * The LSP document version.
   *
   * Must increase on every `didChange` — a server that sees a version it has already applied is
   * entitled to ignore the edit. It is never reset while the document is open, and the entry is
   * dropped on close, so a reopened file legitimately starts again at 1.
   */
  version: number
  timer: ReturnType<typeof setTimeout> | null
  /**
   * How to read the current text when the timer fires.
   *
   * A closure rather than a string so the send carries what the buffer holds *then* — the same
   * reason `scheduleOutline` takes a reader. Storing the text at schedule time would send the
   * keystroke that opened the window and drop everything typed inside it.
   */
  pending: (() => string) | null
}

const docs = new Map<string, Doc>()

/**
 * Fire and forget, but never leave a rejection unhandled.
 *
 * These commands return `()` and cannot fail in Rust, but `invoke` itself rejects when the window
 * is tearing down or the project has just closed — both of which happen exactly while a timer is
 * in flight. Unhandled, that is a console error per keystroke during shutdown.
 */
function fire(call: Promise<void>): void {
  void call.catch(() => {})
}

/** Send whatever is pending for `path` now, and cancel the timer. */
function flush(path: string): void {
  void send(path, null)
}

/**
 * Send `text` — or whatever is pending — immediately, and say when the server has been told.
 *
 * The returned promise is the whole reason this function exists separately from [`flush`]; see
 * [`syncNow`]. Resolves immediately when there is nothing to send.
 */
function send(path: string, text: string | null): Promise<void> {
  const doc = docs.get(path)
  if (doc === undefined) return Promise.resolve()
  if (doc.timer !== null) {
    clearTimeout(doc.timer)
    doc.timer = null
  }
  const read = text !== null ? () => text : doc.pending
  doc.pending = null
  if (read === null) return Promise.resolve()
  doc.version += 1
  const call = diagnosticsApi.didChange(doc.project, path, doc.version, read())
  fire(call)
  // The same promise `fire` is already swallowing the rejection of. Handing it back here is safe
  // precisely because of that: `fire` attached a `.catch` first, so a caller that ignores this
  // one cannot produce an unhandled rejection.
  return call.catch(() => {})
}

/**
 * Bring the server's copy of this buffer up to date **now**, and resolve when it has been told.
 *
 * # Why completion cannot use the throttle
 *
 * Everything else here is allowed to be up to `SYNC_MS` behind, because a diagnostic that arrives
 * 300 ms late is still about the same code. A completion is not: it asks *"what can go at this
 * position"*, and against text that does not contain the position the question has no meaning.
 * The server would answer about the buffer as it was a moment ago, confidently, and the popup
 * would offer the members of whatever used to be under the caret.
 *
 * # Why awaiting this is enough to order the two messages
 *
 * `diagnostics_did_change` is a plain synchronous `#[tauri::command]`: it resolves the server,
 * builds the notification and pushes it into that server's bounded outbox before returning. The
 * completion request goes into **the same outbox**, and the pump drains it in order onto one
 * stdin. So the notification is queued ahead of the request by the time this promise settles, and
 * the ordering is a property of the channel rather than of a race we won.
 *
 * The caller passes text from CodeMirror's immutable `context.state`, so the text sent here and
 * the position asked about cannot disagree even if the user keeps typing.
 */
export function syncNow(path: string, text: string): Promise<void> {
  return send(path, text)
}

/**
 * Send this document's pending edit **and wait for it to land**. (M26)
 *
 * **The await is the load-bearing part**, and it is the same argument [`savedDoc`] makes for
 * `didSave` one step further: [`scheduleDoc`] is a 300 ms trailing throttle, so at any moment
 * the server may be holding text up to that old. Reformat code asks the server to rewrite the
 * buffer and then applies the answer to it — so formatting against stale text does not produce a
 * stale *result*, it produces a **corrupt** one: edits computed against text that is not what
 * they land on.
 *
 * Awaited rather than fired because ordering on the wire follows from ordering of the calls:
 * once this resolves the notification is on the outbox, and the format request joins the same
 * ordered channel behind it. Firing it instead would let the request overtake the notification
 * and reintroduce exactly the staleness this closes.
 *
 * It narrows the window; it does not close it. A keystroke landing *after* the flush is caught
 * on the other side, by `formatModel.isStillCurrent`, which is why both exist.
 */
export function flushDoc(path: string): Promise<void> {
  // `send(path, null)` and not a second implementation: `send` is completion's (M25), and it
  // already returns the promise this needs. One road to the server, one place the version is
  // bumped, and no way for the two to disagree about which notification is in flight.
  return send(path, null)
}

/** A pane opened this file. Only the first one tells the server. */
export function openDoc(project: ProjectId, path: string, text: string): void {
  const existing = docs.get(path)
  if (existing !== undefined) {
    existing.refs += 1
    return
  }
  const doc: Doc = { project, refs: 1, version: 1, timer: null, pending: null }
  docs.set(path, doc)
  fire(diagnosticsApi.didOpen(project, path, doc.version, text))
}

/** The user typed. Coalesced onto the next `SYNC_MS` tick. */
export function scheduleDoc(path: string, read: () => string): void {
  const doc = docs.get(path)
  if (doc === undefined) return
  doc.pending = read
  // Trailing edge: the first change in a burst arms the timer and later ones only replace the
  // reader, so the send lands `SYNC_MS` after the burst *started* rather than being pushed back by
  // every keystroke. See `SYNC_MS`.
  if (doc.timer !== null) return
  doc.timer = setTimeout(() => void flush(path), SYNC_MS)
}

/**
 * The buffer was replaced wholesale — a reload from disk, or an agent's edit landing under the
 * user. Sent immediately: this is not typing, there is no burst to coalesce, and it is the case
 * where the server's copy is most obviously wrong.
 */
export function resetDoc(path: string, text: string): void {
  void send(path, text)
}

/**
 * The file was written.
 *
 * **The flush is load-bearing.** `didSave` is what makes rust-analyzer re-run `cargo check`, and
 * it checks the document *it* holds. Sending it while a change is still sitting on the timer means
 * the server checks the text from up to `SYNC_MS` ago and publishes diagnostics whose line numbers
 * refer to a file that no longer exists — worse than being late, because it looks authoritative.
 */
export function savedDoc(path: string): void {
  const doc = docs.get(path)
  if (doc === undefined) return
  void flush(path)
  fire(diagnosticsApi.didSave(doc.project, path))
}

/** A pane closed this file. Only the last one tells the server. */
export function closeDoc(path: string): void {
  const doc = docs.get(path)
  if (doc === undefined) return
  doc.refs -= 1
  if (doc.refs > 0) return
  if (doc.timer !== null) clearTimeout(doc.timer)
  docs.delete(path)
  // Deliberately *not* flushed. The pending edit is unsaved text in a buffer that is going away,
  // and `didClose` tells the server to fall back to what is on disk — sending the edit first would
  // publish diagnostics for a version of the file nobody can see any more.
  fire(diagnosticsApi.didClose(doc.project, path))
}
