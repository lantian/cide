/**
 * The local discussion under an agent's draft (M101): what the user asked about the finding and
 * what the reviewer that wrote it answered. Nothing here reaches GitLab — publishing a draft
 * posts its body alone — which the composer says, beside the button.
 *
 * Sending goes through `gitlab.discussDraft`: the message is saved under the draft first, then
 * delivered to the reviewer — typed into its conversation if that run is still alive (held until
 * its turn is over), or by reviving the conversation as a new run, which this component hands to
 * `agentReview` so the panel's review note follows it like a launched review.
 */
import { useState } from 'react'
import { gitlab } from '@/ipc/client'
import type { GitLabDraft } from '@/ipc/generated'
import { Button } from '@/kit/components/Button'
import { Textarea } from '@/kit/components/Field'
import { Spinner } from '@/kit/components/Feedback'
import { Person } from '@/kit/components/Status'
import { adoptAgentReview, agentReviews } from './agentReview'
import { Markdown } from './Markdown'
import { data, drafts, refreshDrafts } from './store'
import { message } from './model'
import chrome from './ReviewChrome.module.css'

/** The replies, oldest first, with a waiting line while the reviewer owes an answer. */
export function DraftReplies({
  review,
  draft,
}: {
  review: string
  draft: GitLabDraft
}) {
  if (!draft.replies.length) return null
  const last = draft.replies.at(-1)
  const agent = agentReviews.get(review)
  // Owed an answer: the user spoke last and a reviewer of this MR is alive to give it. Derived,
  // never stored — the answer's arrival (a drafts-changed event) is what clears it.
  const waiting =
    last?.author.run === null &&
    agent !== undefined &&
    agent.state !== 'finished' &&
    agent.state !== 'failed'
  return (
    <ol className={chrome.draftReplies} aria-label="Discussion about this draft">
      {draft.replies.map((reply) => (
        <li
          key={reply.id}
          className={chrome.draftReply}
          data-from={reply.author.run === null ? 'user' : 'reviewer'}
        >
          <header className={chrome.draftReplyHead}>
            <Person
              name={reply.author.run === null ? 'You' : reply.author.label}
              tone={reply.author.run === null ? 'neutral' : 'purple'}
            />
            <time dateTime={new Date(reply.createdUnixMs).toISOString()}>
              {new Date(reply.createdUnixMs).toLocaleString()}
            </time>
          </header>
          <Markdown
            review={review}
            text={reply.body}
            baseUrl={data.get(review)?.mr.web_url}
          />
        </li>
      ))}
      {waiting && (
        <li className={chrome.draftReplyWaiting}>
          <Spinner label="Waiting for the reviewer's answer…" />
        </li>
      )}
    </ol>
  )
}

/** The Discuss composer: a kit textarea and a send button, unsent text kept like `CommentBox`'s. */
export function DiscussBox({
  review,
  draft,
  onDone,
}: {
  review: string
  draft: GitLabDraft
  onDone: () => void
}) {
  const key = `${review}:draft:${draft.id}`
  const [body, setBody] = useState(() => drafts.get(key) ?? '')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  async function send() {
    setBusy(true)
    setError('')
    try {
      const [{ useWorkspace }, { activeProjectIdOf }] = await Promise.all([
        import('@/store/workspace'),
        import('@/keys/target'),
      ])
      const project = activeProjectIdOf(useWorkspace.getState().boot)
      if (!project)
        throw new Error(
          'Open a workspace project to host the reviewer: a revived review runs in its queue.',
        )
      const revived = await gitlab.discussDraft(project, review, draft.id, body)
      if (revived !== null && draft.author.harness !== null)
        adoptAgentReview(review, project, revived, draft.author.harness)
      setBody('')
      drafts.delete(key)
      onDone()
    } catch (e) {
      // The message was saved before delivery was tried; a failure here is why no answer is
      // coming, so the saved text is cleared from the composer only once it is in the thread.
      setError(message(e))
    } finally {
      await refreshDrafts(review).catch(() => undefined)
      setBusy(false)
    }
  }
  return (
    <form
      className={chrome.discussBox}
      onSubmit={(e) => {
        e.preventDefault()
        void send()
      }}
    >
      <Textarea
        aria-label="Message to the reviewer about this draft"
        placeholder="Ask the reviewer about this draft, or ask it to rewrite it…"
        value={body}
        autoFocus
        disabled={busy}
        onChange={(e) => {
          setBody(e.target.value)
          drafts.set(key, e.target.value)
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
            e.preventDefault()
            if (body.trim() && !busy) void send()
          }
        }}
      />
      <div className={chrome.discussActions}>
        <Button
          type="submit"
          size="sm"
          variant="primary"
          icon="message-square"
          busy={busy}
          disabled={!body.trim()}
        >
          Send to reviewer
        </Button>
        <Button size="sm" variant="quiet" disabled={busy} onClick={onDone}>
          Cancel
        </Button>
        <span className={chrome.discussHint}>Stays in cide · never published</span>
      </div>
      {error && (
        <div role="alert" className={chrome.discussError}>
          {error}
        </div>
      )}
    </form>
  )
}
