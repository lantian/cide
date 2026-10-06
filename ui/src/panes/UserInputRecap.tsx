import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { Recap } from '@/kit/components/Recap'
import { peekHost } from '@/layout/paneHosts'
import { highlightUserInputs } from '@/terminal/userInputHighlight'
import { followUserInputs, type InputNavigator } from '@/terminal/userInputNavigation'
import { notify } from '@/chrome/notices'
import { useSettings } from '@/settings/useSettings'
import type { Pane } from '@/ipc/client'
import { navigateInput, selectedOrdinal } from './recapModel'
import { chooseInput, EMPTY_RECAP, expandInput, loadPreviousInput, recapKey, retainRecap, useRecaps } from './recapStore'

/** Called after TerminalPane's open effect. The slot's host, not React, continues to own the
 * terminal, while this conversation-scoped mirror owns only the feature's controls. */
export function useUserInputs(pane: Pane, project: string | undefined) {
  const settings = useSettings()?.terminal
  const harness = pane.harnessConversation?.harness ?? pane.harness ?? pane.continues?.harness ?? 'claude'
  const supported = pane.kind === 'claude' && (harness === 'claude' || harness === 'codex')
  const id = pane.codexCleared ? null : pane.conversation ?? pane.harnessConversation?.id ?? pane.continues?.id ?? pane.session
  const key = supported && project && id ? recapKey(project, id) : null
  const state = useRecaps((s) => key === null ? EMPTY_RECAP : s.conversations[key] ?? EMPTY_RECAP)
  const show = supported && settings?.showRecap === true
  const highlight = supported && settings?.highlightUserInput === true
  const navigator = useRef<InputNavigator | null>(null)
  useEffect(() => {
    if (!key || !project || !id || (!show && !highlight)) return
    return retainRecap(project, id, show || highlight)
  }, [key, project, id, show, highlight])
  useEffect(() => {
    if (!highlight || !key || (harness !== 'claude' && harness !== 'codex')) return
    const handle = peekHost(pane.id)?.terminal
    if (!handle) return
    const painter = highlightUserInputs(handle, pane.id, harness, useRecaps.getState().conversations[key]?.inputs ?? [])
    const stop = useRecaps.subscribe((s, previous) => {
      if (s.conversations[key]?.inputs !== previous.conversations[key]?.inputs) painter.update(s.conversations[key]?.inputs ?? [])
    })
    return () => { stop(); painter.dispose() }
  }, [pane.id, harness, key, highlight])
  useEffect(() => {
    if (!show || !key || (harness !== 'claude' && harness !== 'codex')) return
    const handle = peekHost(pane.id)?.terminal
    if (!handle) return
    let choosing = false
    const follower = followUserInputs(handle, harness, useRecaps.getState().conversations[key]?.inputs ?? [], selected => {
      if (useRecaps.getState().conversations[key]?.selected === selected) return
      choosing = true
      try { chooseInput(key, selected) } finally { choosing = false }
    })
    if (useRecaps.getState().conversations[key]?.selected != null) follower.pause()
    navigator.current = follower
    const stop = useRecaps.subscribe((s, previous) => {
      const next = s.conversations[key], before = previous.conversations[key]
      if (next?.inputs !== before?.inputs) follower.update(next?.inputs ?? [])
      // A mirror's manual choice must survive output in another pane of the conversation.
      if ((!choosing && next?.selected !== before?.selected) || (before && next?.generation !== before.generation)) follower.pause()
    })
    return () => { stop(); follower.dispose(); if (navigator.current === follower) navigator.current = null }
  }, [pane.id, harness, key, show])
  return { show, key, project, id, state, navigator }
}

export function UserInputRecap({ feature }: { feature: ReturnType<typeof useUserInputs> }) {
  const { key, project, id, state } = feature
  const root = useRef<HTMLDivElement>(null)
  const [busy, setBusy] = useState(false)
  // The cluster is a sibling of the pane body, so inherit a measured top-strip from their
  // shared frame. This keeps both the collapsed and expanded recap clear of pane controls.
  useLayoutEffect(() => {
    const element = root.current?.firstElementChild
    const frame = element?.closest<HTMLElement>('[data-kind]')
    if (!element || !frame) return
    const measure = (): void => frame.style.setProperty('--pane-top-strip', `${element.getBoundingClientRect().height}px`)
    const observer = new ResizeObserver(measure)
    observer.observe(element)
    measure()
    return () => { observer.disconnect(); frame.style.removeProperty('--pane-top-strip') }
  }, [])
  const ordinal = selectedOrdinal(state.selected, state.total)
  const input = state.inputs.find((entry) => entry.ordinal === ordinal)
  const status = !id ? 'No submitted inputs yet.'
    : state.status === 'error' ? 'Could not read this conversation’s history. Retrying…'
    : state.status === 'unavailable' ? 'This conversation’s transcript is not available yet.'
    : state.status === 'loading' ? 'Loading inputs…'
    : state.total === 0 ? 'No submitted inputs yet.' : undefined
  const select = async (next: number | null): Promise<void> => {
    if (!key || !project || !id) return
    feature.navigator?.current?.pause()
    const target = selectedOrdinal(next, state.total)
    if (!state.inputs.some((entry) => entry.ordinal === target)) {
      setBusy(true)
      try { await loadPreviousInput(project, id, target + 1) }
      finally { setBusy(false) }
    }
    const current = useRecaps.getState().conversations[key]
    if (current?.generation !== state.generation || !current.inputs.some((entry) => entry.ordinal === target)) return
    chooseInput(key, next)
  }
  const reveal = async (): Promise<void> => {
    const navigator = feature.navigator?.current
    if (!navigator || !input) return
    setBusy(true)
    try {
      const result = await navigator.reveal(input.ordinal)
      if (result === 'missing') notify('Could not locate this input in the console history.', { kind: 'warn', project })
      else if (result === 'unsupported') notify('This console does not support scrolling to older inputs.', { kind: 'warn', project })
    } finally { setBusy(false) }
  }
  return (
    <div ref={root} data-pane-recap="true" style={{ display: 'contents' }}>
      <Recap text={input?.text ?? ''} ordinal={ordinal} total={state.total} status={status}
        expanded={state.expanded} busy={busy}
        onReveal={feature.navigator ? () => void reveal() : undefined}
        onPrevious={() => void select(navigateInput(state.selected, state.total, -1))}
        onNext={() => void select(navigateInput(state.selected, state.total, 1))} onFirst={() => void select(1)}
        onLatest={() => void select(null)} onExpand={() => { if (key) expandInput(key, !state.expanded) }} />
    </div>
  )
}
