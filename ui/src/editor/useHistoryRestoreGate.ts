import { useCallback, useEffect, useRef } from 'react'
import { events, localHistory } from '@/ipc/client'
import type { HistoryEditorRegistration } from '@/ipc/generated'

/** Freeze synchronously before acknowledging; keep a successful restore frozen until reload. */
export function useHistoryRestoreGate(editor: string | undefined, path: string, dirty: () => boolean, refresh: () => void): (handle: ((active: boolean) => void) | null) => void {
  const handle = useRef<((active: boolean) => void) | null>(null)
  const active = useRef(editor !== undefined)
  const refreshing = useRef(false)
  const dirtyRef = useRef(dirty); dirtyRef.current = dirty
  const refreshRef = useRef(refresh); refreshRef.current = refresh
  const receiveHandle = useCallback((next: ((active: boolean) => void) | null) => {
    handle.current = next
    if (next !== null && refreshing.current) { refreshing.current = false; active.current = false }
    next?.(active.current)
  }, [])
  useEffect(() => {
    if (editor === undefined) return
    let dropped = false
    let registration: HistoryEditorRegistration | null = null
    let request: string | null = null
    let unlisten: (() => void) | undefined
    active.current = true; handle.current?.(true)
    void events.onLocalHistoryEditorGate((event) => {
      if (registration === null || !event.editors.some((e) => e.mount === registration?.mount)) return
      if (event.phase === 'prepare') {
        request = event.request; active.current = true; handle.current?.(true)
        void localHistory.acknowledge(editor, registration.mount, request, dirtyRef.current() || handle.current === null).catch(() => {})
      } else if (event.request === request) {
        request = null
        if (event.restored) { refreshing.current = true; refreshRef.current() }
        else { active.current = false; handle.current?.(false) }
      }
    }).then(async (stop) => {
      if (dropped) { stop(); return }
      unlisten = stop
      const mounted = await localHistory.registerEditor(editor, path)
      if (dropped) { void localHistory.unregisterEditor(editor, mounted.mount).catch(() => {}); return }
      registration = mounted
      active.current = false; handle.current?.(false)
    }).catch(() => { if (!dropped) { active.current = false; handle.current?.(false) } })
    return () => {
      dropped = true; unlisten?.()
      if (registration !== null) void localHistory.unregisterEditor(editor, registration.mount).catch(() => {})
      active.current = false; handle.current?.(false)
    }
  }, [editor, path])
  return receiveHandle
}
