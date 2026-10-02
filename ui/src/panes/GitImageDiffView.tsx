import { useState, type ReactNode } from 'react'
import { image, type ImageDoc } from '@/ipc/client'
import type { DiffSide, ImageDiff, ImageDiffSide } from '@/ipc/generated'
import { imageDetail, undecodableMessage, type ImageSize } from './imageKinds'
import imageStyles from './ImagePane.module.css'
import styles from './GitImageDiffView.module.css'

export function imageSideLabels(side: DiffSide): { old: string; new: string } {
  switch (side) {
    case 'unstaged': return { old: 'Index', new: 'Working tree' }
    case 'staged': return { old: 'HEAD', new: 'Index' }
    case 'combined': return { old: 'HEAD', new: 'Working tree' }
  }
}

function ImagePreview({ doc, name }: { doc: ImageDoc; name: string }): ReactNode {
  const [actual, setActual] = useState(false)
  const [size, setSize] = useState<ImageSize | null>(null)
  const [failed, setFailed] = useState(false)
  const [attempt, setAttempt] = useState(0)
  const src = image.srcUrl(doc)
  return <>
    <div className={styles.toolbar}>
      <span className={styles.facts}>{imageDetail(doc.format, doc.bytes, size)}</span>
      <button type="button" className={imageStyles.button} aria-pressed={!actual}
        onClick={() => setActual(false)} title="Scale the image down to fit">Fit</button>
      <button type="button" className={imageStyles.button} aria-pressed={actual}
        onClick={() => setActual(true)} title="Show the image at its original size">1:1</button>
    </div>
    {failed ? <div className={styles.notice} role="status">
      <span>{undecodableMessage(name, doc.format)}</span>
      <button type="button" className={imageStyles.button} onClick={() => {
        setFailed(false)
        setSize(null)
        setAttempt((n) => n + 1)
      }}>Try again</button>
    </div> : <div className={`${imageStyles.canvas} ${styles.canvas}${actual ? ` ${imageStyles.actual}` : ''}`}>
      {size === null && <span className={styles.loading} role="status">Loading image…</span>}
      <img className={imageStyles.image} alt={name} decoding="async"
        src={attempt === 0 ? src : `${src}${src.includes('?') ? '&' : '?'}retry=${attempt}`}
        onLoad={(event) => {
          const el = event.currentTarget
          setSize({ width: el.naturalWidth, height: el.naturalHeight })
        }}
        onError={() => setFailed(true)} />
    </div>}
  </>
}

function ImageColumn({ side, label, path, position }: {
  side: ImageDiffSide; label: string; path: string; position: 'old' | 'new'
}): ReactNode {
  return <section className={styles.column} aria-label={`${position === 'old' ? 'Before' : 'After'}: ${label}`}
    data-audit="gitImageSide" data-side={position}>
    <header className={styles.header}>
      <span>{position === 'old' ? 'Before' : 'After'} · {label}</span>
      <span className={styles.name} title={path}>{path}</span>
    </header>
    {side.kind === 'ready' ? <ImagePreview key={side.doc.path} doc={side.doc} name={path} />
      : <div className={styles.notice} role="status">
        {side.kind === 'unavailable' ? side.reason : 'No image on this side.'}
      </div>}
  </section>
}

export function GitImageDiffView({ images, path, labels }: {
  images: ImageDiff; path: string; labels: { old: string; new: string }
}): ReactNode {
  const single = images.old.kind === 'absent' || images.new.kind === 'absent'
  return <div className={styles.comparison} data-audit="gitImageDiff" data-single={single}>
    {images.old.kind !== 'absent' && <ImageColumn position="old" side={images.old}
      label={labels.old} path={images.oldPath ?? path} />}
    {images.new.kind !== 'absent' && <ImageColumn position="new" side={images.new}
      label={labels.new} path={path} />}
  </div>
}
