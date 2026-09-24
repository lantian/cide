import { useSyncExternalStore } from 'react'
import { IconButton } from '@/kit/components/Button'
import {
  commentCursor,
  commentNavFor,
  subscribeCommentNav,
  type CommentNavKey,
} from './commentNav'
import type { Document } from './store'
import chrome from './ReviewChrome.module.css'

/**
 * The MR diff header's comment walk: previous, "2 / 5", next. The same walker the
 * `gitlab.nextComment`/`gitlab.previousComment` chords drive (`commentNav.ts`), so the counter
 * and the buttons can never disagree with the keys. It replaced the header's "Discussions (N)"
 * button, which only opened the dialog the panel's own Discussions button opens.
 */
export function CommentWalk({ document: doc }: { document: Document }) {
  const key: CommentNavKey = {
    review: doc.review,
    path: doc.path,
    baseSha: doc.refs.base_sha,
    headSha: doc.refs.head_sha,
  }
  const cursor = useSyncExternalStore(
    subscribeCommentNav,
    () => commentCursor(key),
    () => commentCursor(key),
  )
  const [index = -1, count = 0] = cursor.split('/').map(Number)
  const step = (delta: 1 | -1) => commentNavFor(key)?.step(delta)
  return (
    <span className={chrome.commentWalk} aria-label="Comments in this file">
      <IconButton
        icon="chevron-up"
        label="Previous comment (Alt+Shift+Up)"
        disabled={count === 0 || index <= 0}
        onClick={() => step(-1)}
      />
      <span className={chrome.commentWalkCount} aria-live="polite">
        {count === 0
          ? 'No comments'
          : index < 0
            ? `${count} comment${count === 1 ? '' : 's'}`
            : `${index + 1} / ${count}`}
      </span>
      <IconButton
        icon="chevron-down"
        label="Next comment (Alt+Shift+Down)"
        disabled={count === 0 || index >= count - 1}
        onClick={() => step(1)}
      />
    </span>
  )
}
