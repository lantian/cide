import type { GitLabBoard, GitLabRequest, GitLabResponse, GitLabReviewHarness } from '../../ipc/generated'
import type { Args } from '../fakeTauri'
import {
  ACTIVITY,
  APPROVAL,
  BLOBS,
  BOARD,
  COMMITS,
  COMPARISONS,
  DISCUSSIONS,
  DOCUMENT,
  DRAFTS,
  MERGE_REQUEST,
  NOTIFY_LINE,
  REVIEW,
  VERSION,
  VERSIONS,
} from '../data/gitlab'
import { showPanel, sleep, until } from '../drive'
import type { Scene } from '../scenes'
import { leaf, pane, tab } from '../world'

/**
 * GitLab: merge request !214 open for review — its sidebar (approval state, reviewers, the
 * changed-file tree with thread counts) beside `session.rs` as an MR diff tab, with an open
 * inline thread, a resolved one folded away, and an agent's unpublished finding drawn dashed
 * under the line it is about.
 */
export const gitlab: Scene = {
  setup: (world, handlers) => {
    // What `gitlab_open_document` would have opened: a diff tab over one `diff` pane, titled as
    // `cmd/gitlab.rs` titles it.
    const title = `session.rs · MR diff @ ${DOCUMENT.headSha.slice(0, 7)}`
    const diff = pane('diff', title)
    world.open(
      tab(
        {
          kind: 'diff',
          spec: { title, oldPath: DOCUMENT.oldPath, newPath: DOCUMENT.path, origin: { kind: 'gitLab', document: DOCUMENT } },
          preview: false,
        },
        leaf(diff.id),
        [diff],
      ),
    )

    // Every GitLab call goes through one command; `request.kind` is the REST call it stands for.
    const answer = (request: GitLabRequest): unknown => {
      switch (request.kind) {
        case 'board':
          return BOARD satisfies GitLabBoard
        case 'detail':
          return MERGE_REQUEST
        case 'versions':
          return VERSIONS
        case 'diffs':
          return VERSION
        case 'discussions':
          return DISCUSSIONS
        case 'notes':
          return ACTIVITY
        case 'approvals':
          return structuredClone(APPROVAL)
        case 'drafts':
          return DRAFTS
        case 'commits':
          return COMMITS
        case 'comparison':
          return COMPARISONS.get(request.document.path) ?? null
        case 'file':
          return BLOBS.get(`${request.sha}:${request.path}`) ?? ''
        case 'pipelines':
        case 'jobs':
          return []
        default:
          return null
      }
    }
    handlers.set('gitlab_request', (a: Args): GitLabResponse => ({
      data: answer(a['request'] as GitLabRequest),
      nextPage: null,
    }))
    handlers.set('gitlab_review_harnesses', (): GitLabReviewHarness[] => [
      { harness: 'claude', unavailable: null },
      { harness: 'opencode', unavailable: null },
    ])
  },
  drive: async () => {
    await showPanel(`mr:${REVIEW}`)
    // The diff opens at the top of the file; bring the agent's draft and the open thread below
    // it into view, as Alt+Shift+Down would, with a few lines of the code above for context.
    const row = `[data-review-side="new"][data-review-line="${NOTIFY_LINE - 3}"]`
    await until(() => document.querySelector(row) !== null)
    document.querySelector(row)?.scrollIntoView({ block: 'start' })
    await sleep(300)
  },
}
