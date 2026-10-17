/** One versioned review snapshot shared by pane notices and every attention surface. */
import { useSyncExternalStore } from 'react'
import { peerChat, type PeerChatReviews } from '@/ipc/client'

let snapshot: PeerChatReviews = { revision: -1, reviews: [] }
let installed = false
const subscribers = new Set<() => void>()

export const reviewSnapshot = (): PeerChatReviews => snapshot

export function subscribeReviews(notify: () => void): () => void {
  subscribers.add(notify)
  if (!installed) {
    installed = true
    const receive = (next: PeerChatReviews) => {
      if (!next || next.revision <= snapshot.revision) return
      snapshot = next
      for (const subscriber of subscribers) subscriber()
    }
    // Subscribe before pulling. A delayed initial response cannot replace a newer event.
    void peerChat.onReviews(receive).then(() => peerChat.reviews()).then(receive).catch(() => {})
  }
  return () => { subscribers.delete(notify) }
}

export function usePeerReview(tab: string, writer: string | null | undefined) {
  return useSyncExternalStore(subscribeReviews,
    () => writer ? snapshot.reviews.find((r) => r.tab === tab && r.writer === writer) : undefined,
    () => undefined)
}
