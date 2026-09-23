import { renderToStaticMarkup } from 'react-dom/server'
import { ReviewPanel } from './ReviewPanel'
import { Discussions } from './Discussions'
import { Drafts } from './Drafts'
import { data } from './store'
import type { Change, MR, Version } from './types'
const refs = {
  base_sha: 'a'.repeat(40),
  start_sha: 'b'.repeat(40),
  head_sha: 'c'.repeat(40),
}
const change: Change = {
  old_path: 'src/main.go',
  new_path: 'src/main.go',
  diff: '@@ -1 +1,2 @@\n-old\n+new\n+extra',
  new_file: false,
  deleted_file: false,
  renamed_file: false,
}
const mr: MR = {
  id: 1,
  iid: 42,
  project_id: 7,
  title: 'Review the service',
  description: 'Useful context',
  web_url: 'https://git.example/g/p/-/merge_requests/42',
  state: 'opened',
  draft: false,
  source_branch: 'feature',
  target_branch: 'main',
  source_project_id: 7,
  target_project_id: 7,
  author: { id: 1, username: 'author', name: 'Author' },
  reviewers: [],
  assignees: [],
  diff_refs: refs,
  sha: refs.head_sha,
  updated_at: '2026-01-01',
  user_notes_count: 1,
}
const version: Version = {
  ...refs,
  id: 2,
  base_commit_sha: refs.base_sha,
  start_commit_sha: refs.start_sha,
  head_commit_sha: refs.head_sha,
  real_size: '2',
  diffs: [
    change,
    {
      ...change,
      old_path: 'api/generated.pb.go',
      new_path: 'api/generated.pb.go',
    },
  ],
}
data.set('review', {
  mr,
  version,
  versions: [version],
  discussions: [
    {
      id: 'thread',
      individual_note: false,
      notes: [
        {
          id: 3,
          body: 'Please handle the failure.',
          author: { id: 2, name: 'Reviewer', username: 'reviewer' },
          created_at: '2026-01-01',
          system: false,
          resolvable: true,
          resolved: false,
          position: {
            ...refs,
            position_type: 'text',
            old_path: change.old_path,
            new_path: change.new_path,
            new_line: 1,
          },
        },
      ],
    },
  ],
  activity: [],
  approval: { approved: false, approved_by: [], approvals_left: 1 },
  approvalError: null,
  drafts: [
    {
      id: 'draft1',
      review: 'review',
      severity: 'critical',
      body: 'This drops the error.',
      path: change.new_path,
      oldPath: change.old_path,
      side: 'new',
      line: 2,
      position: {
        ...refs,
        position_type: 'text',
        old_path: change.old_path,
        new_path: change.new_path,
        new_line: 2,
      },
      headSha: refs.head_sha,
      author: { label: 'Review !42', harness: 'opencode', run: 'run' },
      createdUnixMs: 0,
    },
  ],
})
console.log(
  JSON.stringify({
    panel: renderToStaticMarkup(<ReviewPanel review="review" />),
    discussions: renderToStaticMarkup(<Discussions review="review" />),
    drafts: renderToStaticMarkup(<Drafts review="review" />),
  }),
)
