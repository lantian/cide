/**
 * One image attachment, full size, over everything. (M39)
 *
 * Its own scrim rather than an `OverlayCard`: the card's chrome — a bordered box sized to its
 * content, 96px from the top — is right for a form and wrong for a picture, which wants the
 * whole viewport and a dark ground. Portalled to `document.body` for `OverlayCard`'s own reason
 * (a tab panel is a stacking context, and a scrim inside one paints under its siblings) and one
 * step **above** it in the z-order, because the viewer is opened *from* the task card, which is
 * itself an `OverlayCard`, and DOM order alone would put a re-rendered card back on top.
 *
 * `images` is the walk ←/→ take: `imageAttachmentsOf`'s order, body first then the log oldest
 * first, so the keys follow the eye down the card. The image itself is the `<img>` the strip
 * already vouched for — same URL, same asset-protocol grant, no second round trip — and a
 * thumbnail that was refused is not in the walk at all.
 *
 * Owned by the host, not by `TaskDetail`: the card is server-rendered by the render check and
 * must draw nothing for a viewer that is not open, and the viewer's open/closed state is the
 * kind of transient the host already keeps (`editing`, `deleteArmed`).
 */
import { useEffect, useRef, type JSX } from 'react'
import { createPortal } from 'react-dom'
import { Icon } from '@/icons/Icon'
import { formatBytes, type AttachmentPreview, type AttachmentView } from './model'
import styles from './AttachmentLightbox.module.css'

export interface AttachmentLightboxProps {
  images: readonly AttachmentView[]
  /** The attachment id on screen. Not in `images` — a detach under the viewer — closes it. */
  current: string
  previews: Readonly<Record<string, AttachmentPreview>>
  onClose: () => void
  onStep: (attachment: string) => void
  onOpen?: ((attachment: string) => void) | undefined
}

export function AttachmentLightbox({
  images,
  current,
  previews,
  onClose,
  onStep,
  onOpen,
}: AttachmentLightboxProps): JSX.Element | null {
  const index = images.findIndex((a) => a.id === current)
  const close = useRef<HTMLButtonElement>(null)

  // Focus lands on the close button when the viewer opens, and goes back to wherever it was
  // when it closes — the thumbnail, normally — so Escape works at once and the card is not left
  // with focus on `<body>`.
  useEffect(() => {
    const before = document.activeElement instanceof HTMLElement ? document.activeElement : null
    close.current?.focus()
    return () => before?.focus()
  }, [])

  useEffect(() => {
    if (index === -1) onClose()
  }, [index, onClose])

  if (index === -1) return null
  const image = images[index]
  if (image === undefined) return null
  const preview = previews[image.id]
  const prev = images[index - 1]
  const next = images[index + 1]

  return createPortal(
    <div
      className={styles.scrim}
      data-audit="attachmentLightbox"
      role="dialog"
      aria-modal="true"
      aria-label={image.name}
      onMouseDown={(event) => {
        // The scrim closes; the figure inside it does not — `OverlayCard`'s split, so a click
        // that lands on the picture (to look closer, or to drag-select the caption) stays.
        if (event.target === event.currentTarget) onClose()
      }}
      onKeyDown={(event) => {
        if (event.key === 'Escape') {
          event.stopPropagation()
          onClose()
        } else if (event.key === 'ArrowLeft' && prev !== undefined) {
          event.preventDefault()
          onStep(prev.id)
        } else if (event.key === 'ArrowRight' && next !== undefined) {
          event.preventDefault()
          onStep(next.id)
        }
      }}
    >
      <div className={styles.bar}>
        <span className={styles.caption} title={image.name}>
          {image.name}
        </span>
        <span className={styles.meta}>
          {formatBytes(image.bytes)}
          {images.length > 1 ? ` · ${index + 1} / ${images.length}` : ''}
        </span>
        {onOpen !== undefined && (
          <button
            type="button"
            className={styles.action}
            data-audit="attachmentLightboxOpen"
            onClick={() => onOpen(image.id)}
          >
            Open externally
          </button>
        )}
        <button
          ref={close}
          type="button"
          className={styles.iconButton}
          data-audit="attachmentLightboxClose"
          title="Close (Esc)"
          aria-label="Close the viewer"
          onClick={onClose}
        >
          <Icon name="x" size={2} />
        </button>
      </div>
      <div className={styles.stage}>
        <button
          type="button"
          className={styles.step}
          data-audit="attachmentLightboxPrev"
          aria-label="Previous image"
          disabled={prev === undefined}
          onClick={() => prev !== undefined && onStep(prev.id)}
        >
          <Icon name="chevron-left" size={3} />
        </button>
        {preview?.kind === 'ready' ? (
          <img className={styles.image} src={preview.url} alt={image.name} />
        ) : (
          <span className={styles.missing}>
            {preview?.kind === 'refused' ? preview.reason : 'Loading…'}
          </span>
        )}
        <button
          type="button"
          className={styles.step}
          data-audit="attachmentLightboxNext"
          aria-label="Next image"
          disabled={next === undefined}
          onClick={() => next !== undefined && onStep(next.id)}
        >
          <Icon name="chevron-right" size={3} />
        </button>
      </div>
    </div>,
    document.body,
  )
}
