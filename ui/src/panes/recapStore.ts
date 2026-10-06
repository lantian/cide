/** One reader and navigation state per conversation/window, shared by mirrored panes.
 * The backend retains the transcript cache. This mirror is transient, and never persists a
 * second copy of the user's prompts or treats a pane id as the conversation id. */
import { create } from 'zustand'
import { sessionJournal, type UserInputPage } from '@/ipc/client'
import { mergeInputs, type PromptEntry } from './recapModel'

export interface RecapState {
  inputs: readonly PromptEntry[]
  total: number
  generation: number
  status: 'loading' | 'ready' | 'unavailable' | 'error'
  selected: number | null
  expanded: boolean
}
export const EMPTY_RECAP: RecapState = {
  inputs: [], total: 0, generation: 0, status: 'loading', selected: null, expanded: false,
}
interface Store { conversations: Readonly<Record<string, RecapState>> }
export const useRecaps = create<Store>(() => ({ conversations: {} }))
export function recapKey(project: string, id: string): string { return JSON.stringify([project, id]) }

function update(key: string, patch: Partial<RecapState>): void {
  useRecaps.setState((s) => ({ conversations: {
    ...s.conversations, [key]: { ...(s.conversations[key] ?? EMPTY_RECAP), ...patch },
  } }))
}
export function chooseInput(key: string, selected: number | null): void { update(key, { selected }) }
export function expandInput(key: string, expanded: boolean): void { update(key, { expanded }) }

interface Reader {
  project: string
  id: string
  refs: number
  historyRefs: number
  busy: boolean
  again: boolean
  timer: ReturnType<typeof setInterval>
}
const readers = new Map<string, Reader>()
let stopEvents: (() => void) | null = null

function accept(key: string, page: UserInputPage): void {
  const previous = useRecaps.getState().conversations[key] ?? EMPTY_RECAP
  if (page.available && page.generation < previous.generation) return
  const generation = page.available ? page.generation : previous.generation
  const reset = previous.generation !== generation
  // A historical page can finish after a newer tail request. Within a generation the
  // transcript only appends; only replacement/truncation can lower its prompt count.
  const total = page.available && !reset ? Math.max(previous.total, page.total) : page.total
  const merged = page.available ? mergeInputs(reset ? [] : previous.inputs, page.inputs) : []
  const inputs = !reset && merged.length === previous.inputs.length
    && merged.every((input, index) => input.id === previous.inputs[index]?.id && input.text === previous.inputs[index]?.text)
    ? previous.inputs : merged
  const status = page.available ? 'ready' : 'unavailable'
  if (inputs === previous.inputs && total === previous.total && status === previous.status && !reset) return
  update(key, {
    inputs, total, generation,
    status,
    ...(reset ? { selected: null, expanded: false } : {}),
  })
}

async function refresh(key: string, reader: Reader): Promise<void> {
  if (reader.busy) { reader.again = true; return }
  reader.busy = true
  try {
    const page = await sessionJournal.userInputs(reader.project, reader.id)
    if (readers.get(key) !== reader) return
    accept(key, page)
    // Highlighting and viewport following need prompts older than the first page. Load them once;
    // future updates merge the tail and never re-fetch the unchanged history.
    while (reader.historyRefs > 0 && readers.get(key) === reader) {
      const state = useRecaps.getState().conversations[key]
      const first = state?.inputs[0]?.ordinal ?? 1
      if (first <= 1 || state?.status !== 'ready') break
      const older = await sessionJournal.userInputs(reader.project, reader.id, first)
      if (readers.get(key) !== reader) return
      accept(key, older)
      if (older.inputs.length === 0) break
    }
  } catch (error) {
    if (readers.get(key) === reader) {
      update(key, { status: 'error' })
      console.debug('[cide] prompt history unavailable', error)
    }
  } finally {
    reader.busy = false
    if (reader.again && readers.get(key) === reader) {
      reader.again = false
      void refresh(key, reader)
    }
  }
}

/** Retention follows the feature's mount, rather than an interval per pane. A late response
 * from a released reader cannot overwrite a new reader for the same conversation. */
export function retainRecap(project: string, id: string, wholeHistory: boolean): () => void {
  const key = recapKey(project, id)
  let reader = readers.get(key)
  if (!reader) {
    const created: Reader = {
      project, id, refs: 0, historyRefs: 0, busy: false, again: false,
      // Retries missing transcripts/disabled hooks, and catches a dropped event or file removal.
      timer: setInterval(() => {
        if (readers.get(key) === created) void refresh(key, created)
      }, 2000),
    }
    reader = created
    readers.set(key, reader)
  }
  reader.refs++
  if (wholeHistory) reader.historyRefs++
  if (stopEvents === null) {
    let cancelled = false
    let unlisten: (() => void) | null = null
    stopEvents = () => { cancelled = true; unlisten?.() }
    void sessionJournal.onUserInputsChanged((p, conversation) => {
      const changedKey = recapKey(p, conversation)
      const changed = readers.get(changedKey)
      if (changed) void refresh(changedKey, changed)
    }).then((stop) => { if (cancelled) stop(); else unlisten = stop })
      .catch((error: unknown) => console.debug('[cide] prompt history listener unavailable', error))
  }
  void refresh(key, reader)
  const retained = reader
  return () => {
    retained.refs--
    if (wholeHistory) retained.historyRefs--
    if (retained.refs !== 0) return
    clearInterval(retained.timer)
    readers.delete(key)
    if (readers.size === 0) { stopEvents?.(); stopEvents = null }
  }
}

export async function loadPreviousInput(project: string, id: string, before: number): Promise<void> {
  const key = recapKey(project, id)
  const reader = readers.get(key)
  if (!reader) return
  try {
    const page = await sessionJournal.userInputs(project, id, before)
    if (readers.get(key) === reader) accept(key, page)
  } catch {
    if (readers.get(key) === reader) update(key, { status: 'error' })
  }
}
