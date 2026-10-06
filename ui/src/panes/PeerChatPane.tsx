import { useEffect, useState, type ReactNode } from 'react'
import { agentDefs, peerChat, type PaneRestore, type Tab } from '@/ipc/client'
import type { ConsoleHarness, PeerChatReceipt } from '@/ipc/generated'
import { Button } from '@/kit/components/Button'
import { Field, TextInput } from '@/kit/components/Field'
import { Select } from '@/kit/components/Select'
import { Note } from '@/kit/components/Feedback'
import { useWorkspace } from '@/store/workspace'
import { explain } from '@/chrome/branchModel'
import { peekHost } from '@/layout/paneHosts'
import { focusPaneDom } from './paneFocus'
import { TerminalPane } from './TerminalPane'
import type { PaneBodyProps } from './PaneBody'
import styles from './PeerChatPane.module.css'

const HARNESSES = [
  { value: 'claude', label: 'Claude Code' },
  { value: 'codex', label: 'Codex' },
  { value: 'opencode', label: 'OpenCode' },
] as const
const label = (h: ConsoleHarness) => HARNESSES.find((v) => v.value === h)?.label ?? h

/** Each panel keeps its existing terminal host. Pair metadata is Rust-owned. */
export function PeerChatPane(props: PaneBodyProps & { tab: Tab }): ReactNode {
  const { pane, tab, project } = props
  const chat = tab.peerChat!
  const main = pane.id === chat.main
  const [harness, setHarness] = useState<ConsoleHarness>('claude')
  const [models, setModels] = useState<string[]>([])
  const [provider, setProvider] = useState('')
  const [model, setModel] = useState('')
  const [catalogProblem, setCatalogProblem] = useState<string | null>(null)
  const [problem, setProblem] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [receipts, setReceipts] = useState<PeerChatReceipt[]>([])
  const [plan, setPlan] = useState<PaneRestore | undefined>()
  const [planned, setPlanned] = useState(false)
  const [fresh, setFresh] = useState(false)

  useEffect(() => {
    if (main || chat.selection || !project) return
    let alive = true
    setModels([])
    setCatalogProblem(null)
    void agentDefs.models(project, harness).then((result) => {
      if (!alive) return
      setModels(result?.models ?? [])
      setCatalogProblem(result?.problem ?? null)
    }).catch((error: unknown) => { if (alive) setCatalogProblem(explain(error)) })
    return () => { alive = false }
  }, [main, chat.selection?.harness, project, harness])

  useEffect(() => {
    if (!chat.selection || chat.starting || !project) { setPlan(undefined); setPlanned(false); return }
    let alive = true
    void peerChat.plan(project, tab.id).then((entries) => {
      if (!alive) return
      setPlan(entries.find((p) => p.pane === pane.id && pane.session !== null))
      setPlanned(true)
    }).catch((error: unknown) => { if (alive) setProblem(explain(error)) })
    return () => { alive = false }
    // A new binding must not rebuild the launch decision or the live terminal.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [chat.selection?.harness, chat.selection?.model, chat.starting, project, tab.id, pane.id])

  useEffect(() => {
    if (!chat.selection || !project) return
    let alive = true
    let timer: ReturnType<typeof setTimeout>
    const poll = async () => {
      try { const result = await peerChat.receipts(project, tab.id); if (alive) setReceipts(result) }
      catch { /* A close cancels this view's polling; the terminal already shows process errors. */ }
      if (alive) timer = setTimeout(() => { void poll() }, 500)
    }
    void poll()
    return () => { alive = false; clearTimeout(timer) }
  }, [chat.selection?.harness, project, tab.id])

  const writing = receipts.some((r) => r.sender !== pane.id && r.status === 'writing')
  useEffect(() => {
    const term = peekHost(pane.id)?.terminal?.term
    if (term) term.options.disableStdin = writing
    return () => { if (term) term.options.disableStdin = false }
  }, [writing, pane.id])

  const start = async () => {
    if (!project) return
    setBusy(true); setProblem(null)
    try {
      const nativeModel = model.trim()
      if (harness === 'opencode' && provider && nativeModel.includes('/') && nativeModel.split('/')[0] !== provider) {
        throw new Error('Choose a model from the selected provider, or change the provider.')
      }
      const chosen = harness === 'opencode' && provider && nativeModel && !nativeModel.includes('/') ? `${provider}/${nativeModel}` : nativeModel
      await peerChat.configure(project, tab.id, { harness, model: chosen || null })
      await useWorkspace.getState().synced()
      requestAnimationFrame(() => { focusPaneDom(chat.main) })
    } catch (error) { setProblem(explain(error)) }
    finally { setBusy(false) }
  }

  const togglePause = async () => {
    if (!project) return
    setBusy(true)
    try { await peerChat.pause(project, tab.id, !chat.paused) }
    catch (error) { setProblem(explain(error)) }
    finally { setBusy(false) }
  }

  if (!chat.selection) {
    if (main) return <div className={styles.setup}><h2>Main · Writer</h2><p>{label(chat.mainHarness)} · Project settings</p><Note>Choose the peer in the right panel. Your task on the left will start collaboration automatically.</Note></div>
    const providers = [...new Set(models.filter((m) => m.includes('/')).map((m) => m.split('/')[0]!))]
    const choices = harness === 'opencode' && provider ? models.filter((m) => m.startsWith(`${provider}/`)) : models
    return <div className={styles.setup}>
      <h2>Choose your peer</h2><p>Experimental · The peer inspects and reviews; the main agent writes.</p>
      <Field label="Harness">{({ id }) => <Select id={id} value={harness} options={HARNESSES} onChange={(value) => { setHarness(value as ConsoleHarness); setModel(''); setProvider(''); setProblem(null) }} />}</Field>
      {harness === 'opencode' ? <Field label="Provider">{({ id }) => <Select id={id} value={provider} options={[{ value: '', label: 'Use configured default' }, ...providers.map((p) => ({ value: p, label: p }))]} onChange={(p) => { setProvider(p); setModel('') }} />}</Field> : <Note>Uses the provider and credentials configured for {label(harness)}.</Note>}
      <Field label="Model" hint="Use provider/model for OpenCode, or leave both fields blank for the configured default.">{({ id, describedBy }) => <TextInput id={id} aria-describedby={describedBy} value={model} list={`peer-models-${pane.id}`} placeholder={harness === 'opencode' ? 'provider/model' : 'Use configured default'} onChange={(e) => setModel(e.target.value)} />}</Field>
      <datalist id={`peer-models-${pane.id}`}>{choices.map((m) => <option key={m} value={m} />)}</datalist>
      {catalogProblem && <Note tone="warn">{catalogProblem}</Note>}
      {problem && <Note tone="bad">{problem}</Note>}
      <Button variant="primary" busy={busy} disabled={harness === 'opencode' && provider !== '' && model.trim() === ''} onClick={() => { void start() }}>Start two-agent chat</Button>
    </div>
  }

  if (chat.starting) return <div className={styles.setup}>Starting both agents…</div>

  const missing = !fresh && plan !== undefined && plan.restore.kind !== 'resumable'
  const currentHarness = main ? chat.mainHarness : chat.selection.harness
  // Both panels see the same receipt list. Show each direction separately so
  // delivery to the peer cannot look like confirmation that its reply arrived.
  const toPeer = receipts.findLast((receipt) => receipt.sender === chat.main)
  const toMain = receipts.findLast((receipt) => receipt.sender === chat.peer)
  return <div className={styles.pane}>
    <div className={styles.header}>
      <strong>{main ? 'Main · Writer' : 'Peer · Reviewer'}</strong>
      <span>{label(currentHarness)}{!main && chat.selection.model ? ` · ${chat.selection.model}` : ''}</span>
      {main && <Button size="sm" busy={busy} onClick={() => { void togglePause() }}>{chat.paused ? 'Resume conversation' : 'Pause conversation'}</Button>}
    </div>
    {chat.paused && <div className={styles.notice}>Conversation paused. Resume or submit a task on the left.</div>}
    <div className={styles.notice} role="status">
      <div>Main → Peer: {toPeer ? `${toPeer.status}: ${toPeer.detail}` : 'No messages sent yet'}</div>
      <div>Peer → Main: {toMain ? `${toMain.status}: ${toMain.detail}` : 'No reply sent yet'}</div>
    </div>
    {problem && <Note tone="bad">{problem}</Note>}
    {!planned ? <div className={styles.setup}>Preparing the conversation…</div> : missing ? <div className={styles.setup}><Note tone="warn">The saved conversation could not be restored. Starting fresh requires your choice.</Note><Button onClick={() => setFresh(true)}>Start a fresh {main ? 'main' : 'peer'} conversation</Button></div> :
      <div className={styles.terminal}><TerminalPane {...props} restore={fresh ? undefined : plan} forceSpawn={plan !== undefined} recovery={fresh ? 'fresh' : undefined} /></div>}
  </div>
}
