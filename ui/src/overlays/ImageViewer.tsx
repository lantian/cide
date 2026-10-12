import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { Icon } from '@/icons/Icon'
import { constrainImage, fitImage, zoomImage, type ImageCamera, type ImageSize } from './imageViewport'
import styles from './ImageViewer.module.css'

export interface ViewerImage {
  id: string
  title: string
  /** Separate from id: two choices may refer to the same preview. */
  previewId: string
  meta?: string | undefined
}
export interface ImageGallery {
  images: readonly ViewerImage[]
  current: string
  returnFocus?: HTMLElement | null | undefined
}
export type ImagePreview =
  | { readonly kind: 'pending' }
  | { readonly kind: 'ready'; readonly url: string }
  | { readonly kind: 'refused'; readonly reason: string }

export interface ImageViewerProps extends ImageGallery {
  previews: Readonly<Record<string, ImagePreview>>
  onClose: () => void
  onStep: (id: string) => void
  onRequestPreview?: ((id: string) => void) | undefined
  onOpen?: ((id: string) => void) | undefined
  closeLabel?: string | undefined
  /** Preserve the attachment viewer's audit hooks while sharing its implementation. */
  auditPrefix?: string | undefined
}

/** Full viewport gallery. Input and focus belong to this layer until it closes. */
export function ImageViewer({ images, current, previews, onClose, onStep, onRequestPreview, onOpen,
  returnFocus, closeLabel = 'Close', auditPrefix = 'imageViewer' }: ImageViewerProps) {
  const root = useRef<HTMLDivElement>(null)
  const close = useRef<HTMLButtonElement>(null)
  // Capture before the opener's modal is suspended, and retain it through StrictMode effects.
  const previousFocus = useRef(returnFocus ?? (document.activeElement instanceof HTMLElement ? document.activeElement : null))
  const index = images.findIndex((image) => image.id === current)
  const image = images[index]
  const preview = image === undefined ? undefined : previews[image.previewId]

  useLayoutEffect(() => {
    const before = previousFocus.current
    close.current?.focus()
    const keepFocus = (event: FocusEvent) => {
      if (event.target instanceof Node && !root.current?.contains(event.target)) close.current?.focus()
    }
    document.addEventListener('focusin', keepFocus)
    return () => {
      document.removeEventListener('focusin', keepFocus)
      // The underlying Modal must leave its suspended state before its opener can be focused.
      queueMicrotask(() => { if (before?.isConnected) before.focus() })
    }
  }, [])

  useEffect(() => { if (image === undefined) onClose() }, [image, onClose])
  useLayoutEffect(() => { close.current?.focus() }, [current, preview?.kind, preview?.kind === 'ready' ? preview.url : null])
  useEffect(() => {
    if (image !== undefined && preview === undefined) onRequestPreview?.(image.previewId)
  }, [image?.previewId, preview, onRequestPreview])

  if (image === undefined) return null
  const prev = images[index - 1]
  const next = images[index + 1]
  const step = (id: string) => {
    // An arrow becoming disabled must not leave keyboard focus on the document body.
    close.current?.focus()
    onStep(id)
  }
  const caption = <>
    <span className={styles.caption} title={image.title}>{image.title}</span>
    <span className={styles.meta} aria-live="polite">{image.meta}{image.meta && images.length > 1 ? ' · ' : ''}{images.length > 1 ? `${index + 1} / ${images.length}` : ''}</span>
  </>
  const actions = <>
    {onOpen && <button type="button" className={styles.action} data-audit={`${auditPrefix}Open`} onClick={() => onOpen(image.id)}>Open externally</button>}
    <button ref={close} type="button" className={styles.action} data-audit={`${auditPrefix}Close`} title={`${closeLabel} (Esc)`} aria-label="Close the viewer" onClick={onClose}>{closeLabel}</button>
  </>
  const navigation = images.length > 1 ? <>
    <button type="button" className={`${styles.step} ${styles.prev}`} data-audit={`${auditPrefix}Prev`} aria-label="Previous image" title="Previous image (←)" disabled={!prev} onClick={() => prev && step(prev.id)}><Icon name="chevron-left" size={3} /></button>
    <button type="button" className={`${styles.step} ${styles.next}`} data-audit={`${auditPrefix}Next`} aria-label="Next image" title="Next image (→)" disabled={!next} onClick={() => next && step(next.id)}><Icon name="chevron-right" size={3} /></button>
  </> : null

  return createPortal(<div ref={root} className={styles.scrim} data-audit={auditPrefix} role="dialog" aria-modal="true" aria-label={image.title}
    onKeyDown={(event) => {
      event.stopPropagation()
      if (event.key === 'Escape') { event.preventDefault(); onClose() }
      else if (event.key === 'ArrowLeft') { event.preventDefault(); if (prev) step(prev.id) }
      else if (event.key === 'ArrowRight') { event.preventDefault(); if (next) step(next.id) }
      else if (event.key === 'Tab') {
        const buttons = [...(root.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? [])]
        const first = buttons[0]
        const last = buttons[buttons.length - 1]
        if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus() }
        else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus() }
      }
    }}
    onMouseDown={(event) => { if (event.target === event.currentTarget) onClose() }}>
    {preview?.kind === 'ready' ? <ImageCanvas key={`${current}:${preview.url}`} url={preview.url} title={image.title} caption={caption} actions={actions} navigation={navigation} /> : <>
      <div className={styles.bar}>{caption}{actions}</div>
      <div className={styles.stage}>{navigation}<span className={styles.missing} role="status">{preview?.kind === 'refused' ? preview.reason : 'Loading image…'}</span></div>
    </>}
  </div>, document.body)
}

function ImageCanvas({ url, title, caption, actions, navigation }: { url: string; title: string; caption: ReactNode; actions: ReactNode; navigation: ReactNode }) {
  const viewport = useRef<HTMLDivElement>(null)
  const img = useRef<HTMLImageElement>(null)
  const [size, setSize] = useState<ImageSize>({ width: 0, height: 0 })
  const [area, setArea] = useState<ImageSize>({ width: 0, height: 0 })
  const [camera, setCamera] = useState<ImageCamera | null>(null)
  const [failed, setFailed] = useState(false)
  const [dragging, setDragging] = useState(false)
  const drag = useRef<{ id: number; x: number; y: number } | null>(null)
  const fit = fitImage(size, area)
  const view = constrainImage(camera ?? { scale: fit, x: 0, y: 0 }, size, area)
  const ready = !failed && size.width > 0 && area.width > 0
  const canPan = ready && (size.width * view.scale > area.width || size.height * view.scale > area.height)
  const zoom = (scale: number, point = { x: 0, y: 0 }) => setCamera((old) => zoomImage(old ?? { scale: fit, x: 0, y: 0 }, scale, point, size, area))
  const syncImage = () => {
    if (img.current?.naturalWidth) setSize({ width: img.current.naturalWidth, height: img.current.naturalHeight })
  }

  useLayoutEffect(() => {
    const element = viewport.current
    if (!element) return
    const measure = () => setArea({ width: element.clientWidth, height: element.clientHeight })
    measure()
    syncImage()
    const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(measure)
    observer?.observe(element)
    window.addEventListener('resize', measure)
    return () => { observer?.disconnect(); window.removeEventListener('resize', measure) }
  }, [])

  useLayoutEffect(() => {
    setCamera((old) => old === null ? null : constrainImage(old, size, area))
  }, [size, area])

  useEffect(() => {
    const element = viewport.current
    if (!element || !ready) return
    // React's delegated wheel listener is passive; a native listener can suppress page zoom/scroll.
    const wheel = (event: WheelEvent) => {
      event.preventDefault()
      event.stopPropagation()
      const rect = element.getBoundingClientRect()
      const delta = event.deltaY * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? area.height : 1)
      const factor = Math.exp(-Math.max(-500, Math.min(500, delta)) * 0.002)
      const point = { x: event.clientX - rect.left - area.width / 2, y: event.clientY - rect.top - area.height / 2 }
      setCamera((old) => {
        const from = old ?? { scale: fit, x: 0, y: 0 }
        return zoomImage(from, from.scale * factor, point, size, area)
      })
    }
    element.addEventListener('wheel', wheel, { passive: false })
    return () => element.removeEventListener('wheel', wheel)
  }, [ready, fit, size, area])

  return <>
    <div className={styles.bar}>
      {caption}
      <div className={styles.zoom} role="group" aria-label="Image zoom">
        <button type="button" className={styles.iconButton} aria-label="Zoom out" title="Zoom out" disabled={!ready || view.scale <= Math.min(0.1, fit)} onClick={() => zoom(view.scale / 1.25)}><Icon name="minus" size={2} /></button>
        <span className={styles.percent} data-audit="imageViewerZoom">{ready ? `${Math.round(view.scale * 100)}%` : '—'}</span>
        <button type="button" className={styles.iconButton} aria-label="Zoom in" title="Zoom in" disabled={!ready || view.scale >= 16} onClick={() => zoom(view.scale * 1.25)}><Icon name="plus" size={2} /></button>
        <button type="button" className={styles.action} disabled={!ready} onClick={() => setCamera(null)}>Fit</button>
        <button type="button" className={styles.action} disabled={!ready} onClick={() => zoom(1)}>100%</button>
      </div>
      {actions}
    </div>
    <div className={styles.stage}>
      <div ref={viewport} className={styles.viewport} data-audit="imageViewerViewport" data-pan={canPan} data-dragging={dragging}
        onPointerDown={(event) => {
          if (!canPan || event.button !== 0) return
          event.preventDefault()
          event.currentTarget.setPointerCapture(event.pointerId)
          drag.current = { id: event.pointerId, x: event.clientX, y: event.clientY }
          setDragging(true)
        }}
        onPointerMove={(event) => {
          const from = drag.current
          if (!from || from.id !== event.pointerId) return
          const dx = event.clientX - from.x
          const dy = event.clientY - from.y
          drag.current = { id: from.id, x: event.clientX, y: event.clientY }
          setCamera((old) => {
            const camera = old ?? { scale: fit, x: 0, y: 0 }
            return constrainImage({ ...camera, x: camera.x + dx, y: camera.y + dy }, size, area)
          })
        }}
        onPointerUp={(event) => {
          if (drag.current?.id !== event.pointerId) return
          drag.current = null
          setDragging(false)
          event.currentTarget.releasePointerCapture(event.pointerId)
        }}
        onLostPointerCapture={() => { drag.current = null; setDragging(false) }}
        onPointerCancel={() => { drag.current = null; setDragging(false) }}>
        <img ref={img} className={styles.image} src={url} alt={title} draggable={false} data-ready={ready}
          style={{ width: size.width * view.scale, height: size.height * view.scale, transform: `translate(-50%, -50%) translate(${view.x}px, ${view.y}px)` }}
          onLoad={syncImage} onError={() => setFailed(true)} />
        {!ready && <span className={styles.missing} role="status">{failed ? 'Image unavailable' : 'Loading image…'}</span>}
      </div>
      {navigation}
    </div>
  </>
}
