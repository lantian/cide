import { useEffect, useId, useRef, useState } from 'react'
import { Button } from '@/kit/components/Button'
import { VisualChoices } from '@/kit/components/Choice'
import { Textarea } from '@/kit/components/Field'
import { Dialog } from '@/kit/components/Overlay'
import { Modal } from '@/overlays/ModalShell'
import { ImageViewer, type ImageGallery, type ViewerImage } from '@/overlays/ImageViewer'
import { questionText, type TaskQuestion, type QuestionAnswer as Answer, type AttachmentPreview } from './model'
import { TaskReferences, type OpenTaskReference } from './TaskReferences'
import styles from './QuestionAnswer.module.css'

export type AnswerHandler = (task: string, answer: Answer) => void | Promise<void>

export function questionImages(question: TaskQuestion): readonly ViewerImage[] {
  return typeof question === 'string' ? [] : question.options.flatMap((option) => option.image === undefined ? [] : [{ id: option.id, title: option.title, previewId: option.image }])
}

/** A pure view: image access and responses are supplied by the panel/card host. */
export function QuestionAnswer({ task, title, question, busy = false, previews, onRequestPreview, onAnswer, onOpenTask, compact = false, embedded = false, formId, onValidityChange, onViewImage }: {
  task: string
  title?: string | undefined
  question: TaskQuestion
  busy?: boolean | undefined
  previews?: Readonly<Record<string, AttachmentPreview>> | undefined
  onViewImage?: ((gallery: ImageGallery) => void) | undefined
  onRequestPreview?: ((task: string, image: string) => void) | undefined
  onAnswer?: AnswerHandler | undefined
  onOpenTask?: OpenTaskReference | undefined
  embedded?: boolean | undefined
  formId?: string | undefined
  onValidityChange?: ((valid: boolean) => void) | undefined
  compact?: boolean | undefined
}) {
  const [text, setText] = useState('')
  const [selected, setSelected] = useState<string[]>([])
  const [sending, setSending] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [expanded, setExpanded] = useState(false)
  const [viewing, setViewing] = useState<ImageGallery | null>(null)
  const inFlight = useRef(false)
  const groupName = useId()
  const locked = busy || sending
  const options = typeof question === 'string' ? [] : question.options
  const images = questionImages(question)
  const valid = selected.length > 0 || text.trim() !== ''
  useEffect(() => { onValidityChange?.(valid && !locked) }, [valid, locked, onValidityChange])
  const send = async () => {
    if (!valid || locked || inFlight.current || onAnswer === undefined) return
    inFlight.current = true
    setSending(true)
    setError(null)
    try {
      await onAnswer(task, { kind: 'answer', text: text.trim(), selectedIds: selected, expectedQuestion: question })
      setText('')
      setSelected([])
      setExpanded(false)
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason))
    } finally {
      inFlight.current = false
      setSending(false)
    }
  }
  const form = (
    <form id={formId} className={styles.form} data-audit="questionAnswerForm" onSubmit={(event) => { event.preventDefault(); void send() }}>
      <p className={compact && !expanded ? styles.summary : styles.text} data-audit="waitingQuestion"><TaskReferences text={questionText(question)} onOpenTask={onOpenTask} /></p>
      {options.length > 0 && (
        <VisualChoices label={typeof question !== 'string' && question.selection === 'multiple' ? 'Choose one or more' : 'Choose one'}
          name={`question-${task}-${groupName}`} multiple={typeof question !== 'string' && question.selection === 'multiple'}
          values={selected} disabled={locked || onAnswer === undefined} onChange={setSelected}
          options={options.map((option) => ({
            id: option.id, title: option.title,
            ...(option.description === undefined ? {} : { description: option.description }),
            ...(option.image === undefined ? {} : { art: <QuestionImage
              task={task} image={option.image} title={option.title} preview={previews?.[option.image]}
              onRequest={onRequestPreview} onView={() => (onViewImage ?? setViewing)({ images, current: option.id,
                returnFocus: document.activeElement instanceof HTMLElement ? document.activeElement : null })} /> }),
          }))} />
      )}
      {selected.length > 0 && onAnswer !== undefined && <Button size="sm" variant="link" disabled={locked}
        onClick={() => setSelected([])}>Clear selection</Button>}
      {onAnswer !== undefined && (
        <>
          <Textarea aria-label={`Your answer to ${task}`} placeholder={options.length > 0 ? 'Custom answer or additional details' : 'Your answer — Ctrl+Enter sends'}
            rows={3} value={text} disabled={locked} autoFocus={expanded || embedded} onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); void send() }
            }} />
          {error !== null && <p className={styles.error} role="alert">{error}</p>}
          {!embedded && <Button size="sm" variant="primary" busy={locked} disabled={!valid || locked}
            data-audit="waitingAnswer" data-write="true" onClick={() => void send()}>Answer</Button>}
        </>
      )}
    </form>
  )
  return (
    <div className={styles.response}>
      {expanded ? (
        <Modal suspended={viewing !== null} onDismiss={() => viewing === null && setExpanded(false)}>
          <Dialog title={title || `Question on ${task}`} width={options.length > 0 ? 'wide' : 'picker'}
            titleAside={onOpenTask === undefined ? undefined : <Button variant="link" size="sm"
              data-audit="waitingDialogOpenTask" onClick={(event) => { setExpanded(false); onOpenTask(task, event.currentTarget) }}>{task}</Button>}
            data-audit="waitingQuestionDialog" onClose={() => setExpanded(false)}
            onKeyDown={(e) => { if (e.key === 'Escape') { e.stopPropagation(); setExpanded(false) } }}>{form}</Dialog>
        </Modal>
      ) : form}
      {!embedded && (compact || options.length > 0) && <Button size="sm" variant="link" data-audit="waitingReadAll" onClick={() => setExpanded(true)}>
        {options.length > 0 ? 'Compare choices' : 'Read in full'}
      </Button>}
      {viewing !== null && (
        <ImageViewer {...viewing} images={images} previews={previews ?? {}} closeLabel={embedded ? 'Back' : 'Close'}
          onClose={() => setViewing(null)} onStep={(current) => setViewing((gallery) => gallery && { ...gallery, current })}
          onRequestPreview={(image) => onRequestPreview?.(task, image)} />
      )}
    </div>
  )
}

export function QuestionImage({ task, image, title, preview, onRequest, onView }: {
  task: string; image: string; title: string
  preview: AttachmentPreview | undefined
  onRequest: ((task: string, image: string) => void) | undefined
  onView: (url: string) => void
}) {
  const element = useRef<HTMLDivElement>(null)
  const [failed, setFailed] = useState(false)
  const url = preview?.kind === 'ready' ? preview.url : undefined
  useEffect(() => { setFailed(false) }, [image, url])
  useEffect(() => {
    if (preview !== undefined || onRequest === undefined || element.current === null) return
    if (typeof IntersectionObserver === 'undefined') { onRequest(task, image); return }
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) { onRequest(task, image); observer.disconnect() }
    })
    observer.observe(element.current)
    return () => observer.disconnect()
  }, [task, image, preview, onRequest])
  return (
    <div ref={element} className={styles.image}>
      {preview?.kind === 'ready' && !failed ? (
        <>
          <img src={preview.url} alt={title} loading="lazy" onError={() => setFailed(true)} />
          <Button size="sm" variant="link" onClick={() => onView(preview.url)} aria-label={`View ${title} full size`}>View image</Button>
        </>
      ) : <span>{failed ? 'Image unavailable' : preview?.kind === 'refused' ? preview.reason : 'Loading image…'}</span>}
    </div>
  )
}
