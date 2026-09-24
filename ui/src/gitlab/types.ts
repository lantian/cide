/** GitLab REST payloads consumed by the review UI. Durable Cide identities are generated. */
export interface User {
  id: number
  web_url?: string
  avatar_url?: string
  username: string
  name: string
}
export interface Refs {
  base_sha: string
  start_sha: string
  head_sha: string
}
export interface MR {
  id: number
  iid: number
  project_id: number
  title: string
  description: string | null
  web_url: string
  state: string
  draft: boolean
  source_branch: string
  target_branch: string
  source_project_id: number | null
  target_project_id: number
  author: User
  reviewers: User[]
  assignees: User[]
  diff_refs: Refs | null
  sha: string
  updated_at: string
  references?: { full: string }
  user_notes_count: number
  source_project?: { web_url: string }
}
export interface Change {
  old_path: string
  new_path: string
  diff: string
  new_file: boolean
  deleted_file: boolean
  renamed_file: boolean
  too_large?: boolean
  collapsed?: boolean
}
export interface Version extends Refs {
  id: number
  base_commit_sha: string
  start_commit_sha: string
  head_commit_sha: string
  diffs?: Change[]
  state?: string
  real_size?: string
  overflow?: boolean
}
export interface Position extends Refs {
  position_type: 'text'
  old_path: string
  new_path: string
  old_line?: number | null
  new_line?: number | null
}
export interface Note {
  id: number
  body: string
  author: User
  created_at: string
  system: boolean
  resolvable?: boolean
  resolved?: boolean
  position?: Position
}
export interface Discussion {
  id: string
  individual_note: boolean
  notes: Note[]
}
export interface Approval {
  approved?: boolean
  approved_by: { user: User }[]
  approvals_required?: number
  approvals_left?: number
  user_has_approved?: boolean
  user_can_approve?: boolean
}
/** `GET /merge_requests/:iid/commits`, the fields the Commits section draws. */
export interface Commit {
  id: string
  short_id: string
  title: string
  author_name: string
  authored_date: string
  web_url: string
}
export interface Pipeline {
  id: number
  project_id: number
  status: string
  ref: string
  sha: string
  web_url: string
}
export interface Job {
  id: number
  name: string
  stage: string
  status: string
  web_url: string
  duration: number | null
  archived?: boolean
  erased_at?: string | null
  downstream_pipeline?: Pipeline | null
}
