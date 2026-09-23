import { showOverlay } from '@/overlays/store'
import { useSyncExternalStore } from 'react'
import {
  gitlab,
  type GitLabBoard,
  type GitLabReview,
  type GitLabRequest,
  type GitLabDocument,
} from '@/ipc/client'
import type {
  MR,
  Version,
  Discussion,
  Approval,
  Change,
  Refs,
  Note,
} from './types'
import { message, versionRefs, visibleChanges } from './model'
export async function api<T>(request: GitLabRequest): Promise<T> {
  return (await gitlab.request(request)).data as T
}
export async function pages<T>(
  make: (page: number) => GitLabRequest,
): Promise<T[]> {
  const result: T[] = []
  let page: number | null = 1
  const seen = new Set<number>()
  while (page !== null && !seen.has(page)) {
    seen.add(page)
    const response = await gitlab.request(make(page))
    result.push(...(response.data as T[]))
    page = response.nextPage
  }
  return result
}
export interface ReviewData {
  mr: MR
  version: Version
  versions: Version[]
  discussions: Discussion[]
  activity: Note[]
  approval: Approval | null
  approvalError: string | null
}
export interface Document {
  review: string
  path: string
  mode: 'diff' | 'source' | 'base'
  change?: Change
  refs: Refs
  oldPath?: string
  at?: { line: number; column: number; side?: 'old' | 'new' }
}
interface Snapshot {
  board: GitLabBoard
  editor: Document | null
  info: {
    review: string
    section: 'description' | 'discussions' | 'activity' | 'pipelines'
  } | null
  error: string | null
  revision: number
}
let snapshot: Snapshot = {
  board: {
    revision: 0,
    accounts: [],
    reviews: [],
    preferences: {
      excludeEnabled: true,
      excludedFiles: ['**/*.pb.go', '**/*_grpc.pb.go'],
    },
  },
  editor: null,
  info: null,
  error: null,
  revision: 0,
}
const listeners = new Set<() => void>()
export const data = new Map<string, ReviewData>()
const pending = new Map<string, Promise<ReviewData>>()
export const drafts = new Map<string, string>()
export const revealed = new Set<string>()
export const roots = new Map<string, string>()
const sourcePending = new Map<string, Promise<string>>()
let started = false
function set(patch: Partial<Snapshot>) {
  snapshot = { ...snapshot, ...patch, revision: snapshot.revision + 1 }
  for (const l of listeners) l()
}
export function touch() {
  set({})
}
export function useGitLab() {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l)
      return () => listeners.delete(l)
    },
    () => snapshot,
    () => snapshot,
  )
}
function boardChanged(board: GitLabBoard) {
  if (board.revision < snapshot.board.revision) return
  const alive = new Set(board.reviews.map((r) => r.id))
  for (const id of data.keys())
    if (!alive.has(id)) {
      data.delete(id)
      revealed.delete(id)
      for (const key of roots.keys())
        if (key.startsWith(id + ':')) roots.delete(key)
    }
  set({
    board,
    error: null,
    editor:
      snapshot.editor && alive.has(snapshot.editor.review)
        ? snapshot.editor
        : null,
  })
}
export function startGitLab() {
  if (started) return
  started = true
  void gitlab.onChanged(boardChanged).catch((e) => set({ error: message(e) }))
  void refreshBoard()
}
export async function refreshBoard() {
  try {
    boardChanged(await api<GitLabBoard>({ kind: 'board' }))
  } catch (e) {
    set({ error: message(e) })
  }
}
export function selectReview(id: string) {
  window.dispatchEvent(new CustomEvent('cide:gitlab-review', { detail: id }))
  void loadReview(id).catch((e) => set({ error: message(e) }))
}
export function showInbox() {
  window.dispatchEvent(new CustomEvent('cide:gitlab-inbox'))
}
export function documentKey(doc: Document): GitLabDocument {
  return {
    review: doc.review,
    path: doc.path,
    oldPath: doc.change?.old_path ?? doc.oldPath ?? doc.path,
    baseSha: doc.refs.base_sha,
    startSha: doc.refs.start_sha,
    headSha: doc.refs.head_sha,
    mode: doc.mode,
    newFile: doc.change?.new_file ?? false,
    deletedFile: doc.change?.deleted_file ?? false,
  }
}
export function documentFromKey(key: GitLabDocument): Document {
  const change = data
    .get(key.review)
    ?.version.diffs?.find(
      (c) =>
        c.new_path === key.path &&
        data.get(key.review)?.version.head_commit_sha === key.headSha &&
        data.get(key.review)?.version.base_commit_sha === key.baseSha,
    )
  return {
    review: key.review,
    path: key.path,
    oldPath: key.oldPath,
    mode: key.mode,
    refs: {
      base_sha: key.baseSha,
      start_sha: key.startSha,
      head_sha: key.headSha,
    },
    change: change ?? {
      old_path: key.oldPath,
      new_path: key.path,
      diff: '',
      new_file: key.newFile,
      deleted_file: key.deletedFile,
      renamed_file: key.path !== key.oldPath,
    },
  }
}
export function openDocument(doc: Document) {
  void (async () => {
    const [{ useWorkspace }, { activeProjectIdOf }] = await Promise.all([
      import('@/store/workspace'),
      import('@/keys/target'),
    ])
    const review = await loadReview(doc.review)
    if (!doc.change) {
      const version = review.versions.find(
        (v) =>
          v.head_commit_sha === doc.refs.head_sha &&
          v.base_commit_sha === doc.refs.base_sha,
      )
      const detail =
        doc.refs.head_sha === review.version.head_commit_sha &&
        doc.refs.base_sha === review.version.base_commit_sha
          ? review.version
          : version
            ? await api<Version>({
                kind: 'diffs',
                review: doc.review,
                version: version.id,
                page: 1,
              })
            : null
      const change = detail?.diffs?.find(
        (c) => c.new_path === doc.path || c.old_path === doc.path,
      )
      if (change) doc = { ...doc, change }
    }
    const project = activeProjectIdOf(useWorkspace.getState().boot)
    if (!project)
      throw new Error(
        'Open a workspace project to host review tabs. The MR may belong to any repository.',
      )
    await gitlab.openDocument(project, documentKey(doc))
    set({ editor: doc, error: null })
  })().catch((e) => set({ error: message(e) }))
}
export function closeDocument() {
  set({ editor: null })
}
export async function openReview(account: string, url: string) {
  const review = await api<GitLabReview>({ kind: 'open', account, url })
  await refreshBoard()
  selectReview(review.id)
  return review
}
export async function closeReview(id: string) {
  // Stop displaying source buffers before their language server and checkout are removed.
  if (snapshot.editor?.review === id) closeDocument()
  boardChanged(await api<GitLabBoard>({ kind: 'close', review: id }))
  for (const key of drafts.keys())
    if (key.startsWith(id + ':')) drafts.delete(key)
}
export async function loadReview(
  id: string,
  force = false,
): Promise<ReviewData> {
  const old = data.get(id)
  if (old && !force) return old
  const running = pending.get(id)
  if (running) return running
  const promise = (async () => {
    const [mr, versions, discussions, activity, approval] = await Promise.all([
      api<MR>({ kind: 'detail', review: id }),
      api<Version[]>({ kind: 'versions', review: id }),
      pages<Discussion>((page) => ({ kind: 'discussions', review: id, page })),
      pages<Note>((page) => ({ kind: 'notes', review: id, page })),
      api<Approval>({ kind: 'approvals', review: id }).then(
        (value) => ({ value, error: null }),
        (error) => ({ value: null, error: message(error) }),
      ),
    ])
    const latest = versions[0]
    if (!latest)
      throw new Error('GitLab is preparing the MR diff. Refresh shortly.')
    const version = await api<Version>({
      kind: 'diffs',
      review: id,
      version: latest.id,
      page: 1,
    })
    if (
      ![
        version.base_commit_sha,
        version.start_commit_sha,
        version.head_commit_sha,
      ].every((sha) => typeof sha === 'string' && /^[0-9a-f]{40}$/i.test(sha))
    ) {
      throw new Error(
        'GitLab is still preparing the MR revisions. Refresh shortly.',
      )
    }
    const accountId = snapshot.board.reviews.find((r) => r.id === id)?.account
    const userId = snapshot.board.accounts.find(
      (a) => a.id === accountId,
    )?.userId
    if (approval.value)
      approval.value.user_has_approved = approval.value.approved_by.some(
        (a) => a.user.id === userId,
      )
    const result = {
      mr,
      versions,
      version,
      discussions,
      activity,
      approval: approval.value,
      approvalError: approval.error,
    }
    if (snapshot.board.reviews.some((r) => r.id === id)) {
      data.set(id, result)
      // Existing tabs remain pinned to the immutable revision they were opened on.
      touch()
    }
    return result
  })()
  pending.set(id, promise)
  try {
    return await promise
  } finally {
    pending.delete(id)
  }
}
export async function refreshApprovals(id: string) {
  let approval: Approval | null = null
  let approvalError: string | null = null
  try {
    approval = await api<Approval>({ kind: 'approvals', review: id })
    const account = snapshot.board.reviews.find((r) => r.id === id)?.account
    const userId = snapshot.board.accounts.find((a) => a.id === account)?.userId
    approval.user_has_approved = approval.approved_by.some(
      (a) => a.user.id === userId,
    )
  } catch (error) {
    approvalError = message(error)
  }
  const old = data.get(id)
  if (old) {
    data.set(id, { ...old, approval, approvalError })
    touch()
  }
}
export async function refreshDiscussions(id: string) {
  const [discussions, activity] = await Promise.all([
    pages<Discussion>((page) => ({ kind: 'discussions', review: id, page })),
    pages<Note>((page) => ({ kind: 'notes', review: id, page })),
  ])
  const old = data.get(id)
  if (old) {
    data.set(id, { ...old, discussions, activity })
    touch()
  }
}
export async function prepareSource(
  id: string,
  revision?: string,
): Promise<string> {
  const sha = revision ?? versionRefs((await loadReview(id)).version).head_sha
  const key = `${id}:${sha}`
  const old = roots.get(key)
  if (old) return old
  const running = sourcePending.get(key)
  if (running) return running
  const promise = (async () => {
    const result = await api<{ root: string }>({
      kind: 'checkout',
      review: id,
      sha,
    })
    if (snapshot.board.reviews.some((r) => r.id === id))
      roots.set(key, result.root)
    touch()
    return result.root
  })()
  sourcePending.set(key, promise)
  try {
    return await promise
  } finally {
    sourcePending.delete(key)
  }
}

export function showReviewInfo(
  review: string,
  section: NonNullable<Snapshot['info']>['section'],
) {
  set({ info: { review, section } })
  showOverlay('gitlabInfo')
}

export async function navigateReviewFile(key: GitLabDocument, delta: number) {
  const review = await loadReview(key.review)
  const visible = visibleChanges(
    review.version.diffs ?? [],
    snapshot.board.preferences.excludedFiles,
    snapshot.board.preferences.excludeEnabled && !revealed.has(key.review),
  )
  const index = visible.findIndex((c) => c.new_path === key.path)
  const change = visible[index + delta]
  if (change)
    openDocument({
      review: key.review,
      path: change.new_path,
      mode: 'diff',
      change,
      refs: versionRefs(review.version),
    })
}
