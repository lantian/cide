/**
 * Thumbnails for image attachments, and the one call that makes them legal. (M39)
 *
 * `editor/markdown/images.ts` for the task card: an attachment record says `kind: 'image'`,
 * and turning that into pixels on screen is `attachments.image` — the same road `image.read`
 * is for a file in a tab. Rust re-sniffs the bytes on disk, refuses anything that is not an
 * image *now* whatever the record claims, and widens the asset-protocol scope by exactly that
 * one file. The `<img>` then loads over the asset protocol; no pixels cross the IPC.
 *
 * Cached per attachment id, per window, **including the refusal**: an attachment's bytes never
 * change under their id (there is no replace, only detach and attach again), so the answer is
 * good for the life of the window and re-asking on every render of a card with six screenshots
 * would be six round trips per frame.
 *
 * # The snapshot is an object, and that is safe here
 *
 * `check:selectors`' rule is about a selector that *builds* a value per read. [`previewsSnapshot`]
 * returns the **same** object until an answer lands — `Object.is` holds between reads, so
 * `useSyncExternalStore` bails out. The host reads that snapshot and hands it to the card as a
 * prop; the card never subscribes itself, which is what keeps it server-renderable.
 */
import { attachments as attachmentsApi, diag, image as imageApi } from '@/ipc/client'
import type { AttachmentPreview } from './model'

let snapshot: Readonly<Record<string, AttachmentPreview>> = {}
const inflight = new Set<string>()
const listeners = new Set<() => void>()

function emit(): void {
  for (const listener of listeners) listener()
}

export function subscribePreviews(listener: () => void): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

/**
 * Every answer so far, by attachment id. A new object only when an answer lands, so a host
 * holding it through `useSyncExternalStore` re-renders exactly then. An id with no entry is
 * pending — or never asked about, which the strip draws the same way.
 */
export function previewsSnapshot(): Readonly<Record<string, AttachmentPreview>> {
  return snapshot
}

/**
 * Ask Rust to vouch for an image attachment, at most once per window.
 *
 * `project` and `task` are the address the command needs; the cache key is the attachment id
 * alone, because an id is minted once and lives under one task for ever.
 */
export function requestPreview(project: string, task: string, id: string): void {
  if (id in snapshot || inflight.has(id)) return
  inflight.add(id)
  void attachmentsApi
    .image(project as never, task as never, id)
    .then((doc) => {
      snapshot = { ...snapshot, [id]: { kind: 'ready', url: imageApi.srcUrl(doc) } }
    })
    .catch((error: unknown) => {
      const reason = error instanceof Error ? error.message : String(error)
      snapshot = { ...snapshot, [id]: { kind: 'refused', reason } }
      diag.log(`task attachment ${id}: ${reason}`)
    })
    .finally(() => {
      inflight.delete(id)
      emit()
    })
}

/**
 * Forget one attachment's answer — after a detach, so an id that somehow comes back is asked
 * about afresh rather than drawn from a URL whose file is gone.
 */
export function forgetPreview(id: string): void {
  if (!(id in snapshot)) return
  const { [id]: _gone, ...rest } = snapshot
  snapshot = rest
  emit()
}
