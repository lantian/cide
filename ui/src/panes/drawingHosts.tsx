/**
 * A drawing's React root outlives the slot that displays it. Excalidraw exposes no way to
 * restore its undo history, so a scene snapshot cannot preserve a full editing session.
 *
 * Keep the whole pane, including its dirty baseline, saver and autosave timer. Parking is
 * hidden, connected and measured at the last real size: a zero-sized canvas must not adjust
 * its viewport while its project is away. This registry is window-local and never evicts an
 * open drawing. The workspace owner supplies the live membership; a slot unmount is only a
 * move, never evidence that the drawing has closed.
 *
 * React and the DOM are the only dependencies so the lifecycle can be exercised with a real
 * React root under jsdom, without pretending to implement Tauri or the drawing engine.
 */
import type { ReactNode } from 'react'
import { createRoot, type Root } from 'react-dom/client'

interface ScreenProps {
  readonly onScreen?: boolean | undefined
}

interface DrawingHost<Props> {
  readonly element: HTMLDivElement
  readonly root: Root
  props: Props
  slot: HTMLElement | null
  lease: object
}

export function createDrawingHosts<Props extends ScreenProps>(render: (props: Props, attachment: object) => ReactNode) {
  const hosts = new Map<string, DrawingHost<Props>>()
  let parking: HTMLDivElement | null = null

  function updateHost(host: DrawingHost<Props>, props: Props, moved = false): void {
    const next = { ...props, onScreen: host.slot !== null && props.onScreen !== false }
    const keys = Object.keys(next) as (keyof Props)[]
    if (!moved && keys.length === Object.keys(host.props).length && keys.every((key) => host.props[key] === next[key])) return
    host.props = next
    host.root.render(render(next, host.lease))
  }

  function parked(): HTMLDivElement {
    if (parking === null) {
      parking = document.createElement('div')
      parking.dataset['drawingParking'] = ''
      parking.setAttribute('aria-hidden', 'true')
      parking.inert = true
      Object.assign(parking.style, {
        position: 'fixed', left: '-100000px', top: '0', visibility: 'hidden', pointerEvents: 'none',
      })
      document.body.appendChild(parking)
    }
    return parking
  }

  function dispose(pane: string, host: DrawingHost<Props>): void {
    hosts.delete(pane)
    host.root.unmount()
    host.element.remove()
    if (hosts.size === 0) {
      parking?.remove()
      parking = null
    }
  }

  return {
    mount(pane: string, slot: HTMLElement, props: Props): () => void {
      let host = hosts.get(pane)
      const fresh = host === undefined
      if (host === undefined) {
        const element = document.createElement('div')
        element.dataset['drawingHost'] = pane
        Object.assign(element.style, { position: 'relative', width: '100%', height: '100%', minWidth: '0', minHeight: '0' })
        host = { element, root: createRoot(element), props, slot: null, lease: {} }
        hosts.set(pane, host)
      }
      const lease = {}
      host.lease = lease
      host.slot = slot
      Object.assign(host.element.style, { width: '100%', height: '100%' })
      slot.appendChild(host.element)
      if (fresh) host.root.render(render(props, lease))
      else updateHost(host, props, true)

      return () => {
        // A split move may attach the same pane before the old slot cleans up. Only the
        // current attachment can park it; StrictMode's attach/park/attach follows this too.
        if (hosts.get(pane) !== host || host.lease !== lease) return
        const box = host.element.getBoundingClientRect()
        if (box.width > 0) host.element.style.width = `${box.width}px`
        if (box.height > 0) host.element.style.height = `${box.height}px`
        host.slot = null
        parked().appendChild(host.element)
        updateHost(host, host.props)
      }
    },

    update(pane: string, slot: HTMLElement, props: Props): void {
      const host = hosts.get(pane)
      if (host?.slot === slot) updateHost(host, props)
    },

    /** Update retained panes, including renames in inactive projects. Never opens a new one. */
    reconcile(live: ReadonlyMap<string, Props>): void {
      for (const [pane, host] of hosts) {
        const props = live.get(pane)
        if (props === undefined) dispose(pane, host)
        else updateHost(host, props)
      }
    },
  }
}
