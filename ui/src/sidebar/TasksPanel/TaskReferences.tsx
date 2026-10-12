import type { ReactNode } from 'react'
import { Button } from '@/kit/components/Button'

export type OpenTaskReference = (task: string, source: HTMLButtonElement) => void

/** Task IDs in a question navigate through its host; the rest stays plain, selectable text. */
export function TaskReferences({ text, onOpenTask }: {
  text: string
  onOpenTask?: OpenTaskReference | undefined
}) {
  if (onOpenTask === undefined) return <>{text}</>
  const parts: ReactNode[] = []
  let start = 0
  for (const match of text.matchAll(/(?<![\p{L}\p{N}_])t-[0-9]+(?![\p{L}\p{N}_])/gu)) {
    const index = match.index
    const id = match[0]
    parts.push(text.slice(start, index), <Button key={index} variant="link" data-audit="questionTaskReference"
      aria-label={`Open task ${id}`} onClick={(event) => onOpenTask(id, event.currentTarget)}>{id}</Button>)
    start = index + id.length
  }
  parts.push(text.slice(start))
  return <>{parts}</>
}
