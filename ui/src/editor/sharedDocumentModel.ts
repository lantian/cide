/** One document and undo stack, with view-local selections. No React or IPC in this model. */
import { ChangeSet, EditorState, Transaction, type EditorSelection } from '@codemirror/state'
import { history, historyField, isolateHistory, redo, redoDepth, undo, undoDepth } from '@codemirror/commands'
import { diff } from '@codemirror/merge'

export function changesBetween(before: string, after: string): ChangeSet {
  return ChangeSet.of(diff(before, after).map(c => ({
    from: c.fromA, to: c.toA, insert: after.slice(c.fromB, c.toB),
  })), before.length)
}

export interface DocumentSnapshot {
  version: number
  state: string
  savedText: string
  stamp: { mtimeNanos: string; len: number } | null
}

type Operation =
  | { kind: 'edit'; before: string; changes: ChangeSet; selection?: EditorSelection; userEvent: string; isolated: boolean }
  | { kind: 'undo' }
  | { kind: 'redo' }
  | { kind: 'saved'; text: string; stamp: DocumentSnapshot['stamp'] }
  | { kind: 'stamp'; stamp: DocumentSnapshot['stamp'] }
  | { kind: 'reload'; text: string; stamp: DocumentSnapshot['stamp'] }

const fields = { history: historyField }
const extensions = [history()]

export class SharedDocumentModel {
  state: EditorState
  savedText: string
  stamp: DocumentSnapshot['stamp']
  version: number | null = null
  private pending: Operation[] = []
  private listeners = new Set<(source?: unknown) => void>()

  constructor(text: string, stamp: DocumentSnapshot['stamp'] = null) {
    this.state = EditorState.create({ doc: text, extensions })
    this.savedText = this.state.doc.toString()
    this.stamp = stamp
  }

  get dirty(): boolean { return this.state.doc.toString() !== this.savedText }
  get undoDepth(): number { return undoDepth(this.state) }
  get redoDepth(): number { return redoDepth(this.state) }
  get hasPending(): boolean { return this.pending.length > 0 }
  subscribe(listener: (source?: unknown) => void): () => void {
    this.listeners.add(listener)
    return () => { this.listeners.delete(listener) }
  }
  private publish(source?: unknown): void { for (const fn of this.listeners) fn(source) }

  edit(tr: Transaction, source?: unknown): void {
    if (!tr.docChanged) return
    const op: Operation = { kind: 'edit', before: tr.startState.doc.toString(), changes: tr.changes,
      selection: tr.newSelection, userEvent: tr.annotation(Transaction.userEvent) ?? 'input',
      isolated: tr.annotation(isolateHistory) !== undefined }
    this.apply(op)
    this.pending.push(op)
    this.publish(source)
  }

  history(direction: 'undo' | 'redo'): boolean {
    if ((direction === 'undo' ? this.undoDepth : this.redoDepth) === 0) return false
    const op: Operation = { kind: direction }
    this.apply(op)
    this.pending.push(op)
    this.publish()
    return true
  }

  saved(text: string, stamp: DocumentSnapshot['stamp']): void {
    const op: Operation = { kind: 'saved', text, stamp }
    this.apply(op)
    this.pending.push(op)
    this.publish()
  }

  acceptStamp(stamp: DocumentSnapshot['stamp']): void {
    const op: Operation = { kind: 'stamp', stamp }
    this.apply(op)
    this.pending.push(op)
    this.publish()
  }

  reload(text: string, stamp: DocumentSnapshot['stamp']): void {
    const op: Operation = { kind: 'reload', text, stamp }
    this.apply(op)
    this.pending.push(op)
    this.publish()
  }

  private apply(op: Operation): void {
    if (op.kind === 'saved') {
      this.savedText = EditorState.create({ doc: op.text }).doc.toString()
      this.stamp = op.stamp
    } else if (op.kind === 'stamp') {
      this.stamp = op.stamp
    } else if (op.kind === 'reload') {
      this.state = EditorState.create({ doc: op.text, extensions })
      this.savedText = this.state.doc.toString()
      this.stamp = op.stamp
    } else if (op.kind === 'undo' || op.kind === 'redo') {
      (op.kind === 'undo' ? undo : redo)({ state: this.state, dispatch: tr => { this.state = tr.state } })
    } else {
      const current = this.state.doc.toString()
      // A different window won the CAS. Transform this edit over its changes, preserving
      // both windows' insertions rather than replacing the file with a stale full buffer.
      const remote = changesBetween(op.before, current)
      const changes = remote.empty ? op.changes : op.changes.map(remote)
      const selection = remote.empty ? op.selection : undefined
      this.state = this.state.update({ changes,
        ...(selection ? { selection } : {}),
        annotations: [Transaction.userEvent.of(op.userEvent),
          ...(op.isolated ? [isolateHistory.of('full')] : [])],
      }).state
    }
  }

  snapshot(): DocumentSnapshot {
    return { version: this.version ?? 0, state: JSON.stringify(this.state.toJSON(fields)),
      savedText: this.savedText, stamp: this.stamp }
  }

  /** The submitted prefix is acknowledged only when our expected revision won. */
  submission(): { count: number; expected: number | null; next: DocumentSnapshot } {
    return { count: this.pending.length, expected: this.version, next: this.snapshot() }
  }

  receive(snapshot: DocumentSnapshot, acknowledged = 0): void {
    if (this.version !== null && snapshot.version < this.version) return
    this.pending.splice(0, acknowledged)
    this.version = snapshot.version
    // The accepted snapshot already describes this state, plus newer local edits.
    // Deserializing on every acknowledgement loses CodeMirror's typing-group timestamp.
    if (acknowledged > 0) return
    this.state = EditorState.fromJSON(JSON.parse(snapshot.state), { extensions }, fields)
    this.savedText = snapshot.savedText
    this.stamp = snapshot.stamp
    for (const op of this.pending) this.apply(op)
    this.publish()
  }
}
