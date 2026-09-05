/**
 * The files on a task's body or on a comment, drawn as a strip of tiles. (M39)
 *
 * Pure: everything it draws arrives as props, and every gesture goes out through a callback the
 * host wires to a command. It is rendered by `TaskDetail`, which `check-agents-render.mjs`
 * server-renders, so nothing here may read a store or call IPC — the same rule
 * `TaskMarkdown.tsx` states for itself, and the reason a thumbnail's URL is a prop
 * (`previews`) rather than something this component asks for.
 *
 * # What an image tile is
 *
 * A medium thumbnail: the picture at its own aspect inside a bounded box, never cropped
 * (`object-fit: contain`), because a screenshot's edges are where the interesting thing usually
 * is. Clicking it opens the fullscreen viewer, which the host owns. Three states, all visible
 * — `pending` while Rust vouches for the file, `ready`, and `refused` with the sentence — on
 * `MarkdownPreview.tsx`'s rule that a preview that cannot load must look like something.
 *
 * # What a file tile is
 *
 * A name and a size behind a paperclip. Clicking it opens the file with the desktop's own
 * application, from Rust, which is the only road that works in a detached window.
 *
 * # Remove is the user's, and it is not armed
 *
 * Drawn only when the host passes `onDetach`, which it does for the user and never for anyone
 * else — the same gate the comment Edit/Delete pair has. Not armed like the task-level delete:
 * an attachment is one gesture to re-add, and `ConfirmDestructive`'s rule is that a dialog is
 * for what cannot be undone.
 */
import type { JSX } from 'react'
import { Icon } from '@/icons/Icon'
import {
  formatBytes,
  type AttachmentPreview,
  type AttachmentView,
  type StagedAttachment,
} from './model'
import styles from './TasksPanel.module.css'

export interface AttachmentStripProps {
  task: string
  attachments: readonly AttachmentView[]
  /** By attachment id. An id with no entry is drawn as pending. */
  previews?: Readonly<Record<string, AttachmentPreview>> | undefined
  onView?: ((task: string, attachment: string) => void) | undefined
  onOpen?: ((task: string, attachment: string) => void) | undefined
  onReveal?: ((task: string, attachment: string) => void) | undefined
  onDetach?: ((task: string, attachment: string) => void) | undefined
}

const PENDING: AttachmentPreview = { kind: 'pending' }

export function AttachmentStrip({
  task,
  attachments,
  previews,
  onView,
  onOpen,
  onReveal,
  onDetach,
}: AttachmentStripProps): JSX.Element | null {
  if (attachments.length === 0) return null
  return (
    <div className={styles.attachments} data-audit="tasksAttachments">
      {attachments.map((attachment) => {
        const preview = attachment.kind === 'image' ? (previews?.[attachment.id] ?? PENDING) : null
        return (
          <div
            className={styles.attachment}
            data-audit="tasksAttachment"
            data-kind={attachment.kind}
            data-preview={preview?.kind ?? ''}
            key={attachment.id}
          >
            {attachment.kind === 'image' ? (
              /* A button, not a bare `<img>`: the thumbnail is the road to the viewer, and a
                 road has to be reachable from the keyboard. */
              <button
                type="button"
                className={styles.thumbButton}
                data-audit="tasksThumb"
                title={`View ${attachment.name}`}
                aria-label={`View ${attachment.name} full size`}
                disabled={onView === undefined || preview?.kind !== 'ready'}
                onClick={() => onView?.(task, attachment.id)}
              >
                {preview?.kind === 'ready' ? (
                  <img className={styles.thumb} src={preview.url} alt={attachment.name} />
                ) : (
                  <span
                    className={styles.thumbPlaceholder}
                    data-audit="tasksThumbPlaceholder"
                    title={preview?.kind === 'refused' ? preview.reason : undefined}
                  >
                    <Icon name="image" size={3} />
                    <span className={styles.thumbNote}>
                      {preview?.kind === 'refused' ? 'No preview' : 'Loading…'}
                    </span>
                  </span>
                )}
              </button>
            ) : (
              <button
                type="button"
                className={styles.fileTile}
                data-audit="tasksFile"
                title={`Open ${attachment.name}`}
                aria-label={`Open ${attachment.name}`}
                disabled={onOpen === undefined}
                onClick={() => onOpen?.(task, attachment.id)}
              >
                <Icon name="paperclip" size={2} />
              </button>
            )}
            <div className={styles.attachmentMeta}>
              <span className={styles.attachmentName} title={attachment.name}>
                {attachment.name}
              </span>
              <span className={styles.attachmentSize}>{formatBytes(attachment.bytes)}</span>
            </div>
            {(onOpen !== undefined || onReveal !== undefined || onDetach !== undefined) && (
              <div className={styles.attachmentActions} data-audit="tasksAttachmentActions">
                {onOpen !== undefined && (
                  <button
                    type="button"
                    className={styles.logAction}
                    data-audit="tasksAttachmentOpen"
                    aria-label={`Open ${attachment.name}`}
                    onClick={() => onOpen(task, attachment.id)}
                  >
                    Open
                  </button>
                )}
                {onReveal !== undefined && (
                  <button
                    type="button"
                    className={styles.logAction}
                    data-audit="tasksAttachmentReveal"
                    aria-label={`Show ${attachment.name} in the file manager`}
                    onClick={() => onReveal(task, attachment.id)}
                  >
                    Reveal
                  </button>
                )}
                {onDetach !== undefined && (
                  <button
                    type="button"
                    className={styles.logAction}
                    data-audit="tasksAttachmentRemove"
                    data-write="true"
                    aria-label={`Remove ${attachment.name} from ${task}`}
                    onClick={() => onDetach(task, attachment.id)}
                  >
                    Remove
                  </button>
                )}
              </div>
            )}
          </div>
        )
      })}
    </div>
  )
}

/**
 * The files a person has picked, pasted or dropped and not yet submitted — the composer's and
 * the New task dialog's chips. Each has an ✕, because a mis-pick costs one click to undo here
 * and a whole detach once it has landed.
 */
export interface StagedChipsProps {
  staged: readonly StagedAttachment[]
  onRemove: (path: string) => void
}

export function StagedChips({ staged, onRemove }: StagedChipsProps): JSX.Element | null {
  if (staged.length === 0) return null
  return (
    <div className={styles.staged} data-audit="tasksStaged">
      {staged.map((file) => (
        <span
          className={styles.stagedChip}
          data-audit="tasksStagedChip"
          key={file.path}
          title={file.path}
        >
          <Icon name="paperclip" size={1} />
          <span className={styles.stagedName}>{file.name}</span>
          {file.bytes !== null && (
            <span className={styles.attachmentSize}>{formatBytes(file.bytes)}</span>
          )}
          <button
            type="button"
            className={styles.stagedRemove}
            data-audit="tasksStagedRemove"
            aria-label={`Do not attach ${file.name}`}
            onClick={() => onRemove(file.path)}
          >
            <Icon name="x" size={0} />
          </button>
        </span>
      ))}
    </div>
  )
}
