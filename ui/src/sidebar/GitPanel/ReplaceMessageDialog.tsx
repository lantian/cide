import { useEffect, useRef } from 'react'
import { Modal } from '@/overlays/ModalShell'
import { Button } from '@/kit/components/Button'
import { Field, Textarea } from '@/kit/components/Field'
import { Dialog } from '@/kit/components/Overlay'

export function ReplaceMessageDialog({
  existing,
  generated,
  revision,
  onCancel,
  onReplace,
}: {
  existing: string
  generated: string
  revision: number
  onCancel: () => void
  onReplace: (revision: number) => void
}) {
  const cancel = useRef<HTMLButtonElement>(null)
  useEffect(() => { cancel.current?.focus() }, [revision])

  return (
    <Modal onDismiss={onCancel}>
      <Dialog
        title="Replace commit message?"
        lead="Your draft will be replaced with the generated message below."
        onClose={onCancel}
        data-audit="gitReplaceMessage"
        onKeyDown={(event) => {
          if (event.key === 'Escape') {
            event.stopPropagation()
            onCancel()
          }
        }}
        actions={<>
          <Button ref={cancel} onClick={onCancel}>Cancel</Button>
          <Button variant="primary" onClick={() => onReplace(revision)}>Replace</Button>
        </>}
      >
        <Field label="Current draft">
          {({ id }) => <Textarea id={id} readOnly rows={4} value={existing} />}
        </Field>
        <Field label="Generated message">
          {({ id }) => <Textarea id={id} readOnly rows={8} value={generated} />}
        </Field>
      </Dialog>
    </Modal>
  )
}
