import { useEffect, useRef, useState } from 'react'
import { Button } from '@/kit/components/Button'
import { Field, TextInput } from '@/kit/components/Field'
import { Dialog } from '@/kit/components/Overlay'
import { Modal } from '@/overlays/ModalShell'
import { localHistory, type ProjectId } from '@/ipc/client'
import { usePutLabel } from './actions'

function Body({ project, onClose }: { project: ProjectId; onClose: () => void }) {
  const [text, setText] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const input = useRef<HTMLInputElement>(null)
  useEffect(() => input.current?.focus(), [])

  const put = () => {
    if (text.trim() === '' || busy) return
    setBusy(true)
    localHistory.label(project, text).then(onClose, (reason) => {
      setBusy(false)
      setError(String(reason))
    })
  }

  return (
    <Modal onDismiss={onClose}>
      <Dialog
        title="Put label"
        lead="Names this moment in Local History, so it is easy to find before a risky change."
        onClose={onClose}
        data-audit="localHistoryLabel"
        onKeyDown={(event) => {
          if (event.key === 'Escape') {
            event.stopPropagation()
            onClose()
          }
          if (event.key === 'Enter') put()
        }}
        actions={
          <>
            <Button onClick={onClose}>Cancel</Button>
            <Button variant="primary" disabled={text.trim() === '' || busy} onClick={put}>
              Put label
            </Button>
          </>
        }
      >
        <Field label="Label" error={error ?? undefined}>
          {({ id }) => (
            <TextInput
              id={id}
              ref={input}
              value={text}
              onChange={(event) => setText(event.target.value)}
            />
          )}
        </Field>
      </Dialog>
    </Modal>
  )
}

/** Mounted once per window, because the palette can raise it from any of them. */
export function PutLabelDialog() {
  const project = usePutLabel((s) => s.project)
  const close = usePutLabel((s) => s.close)
  return project === null ? null : <Body key={project} project={project} onClose={close} />
}
