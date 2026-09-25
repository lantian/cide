import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { resolve } from 'node:path'
import { JSDOM } from 'jsdom'
import React, { act } from 'react'

mkdirSync('node_modules/.cache', { recursive: true })
const out = mkdtempSync('node_modules/.cache/cide-gitlab-dom-')
const dom = new JSDOM('<!doctype html><div id="root"></div>', {
  url: 'https://cide.test',
  pretendToBeVisual: true,
})
for (const name of [
  'window',
  'document',
  'location',
  'HTMLElement',
  'Element',
  'Node',
  'MutationObserver',
  'DOMParser',
])
  globalThis[name] = dom.window[name]
globalThis.self = dom.window
const { createRoot } = await import('react-dom/client')
globalThis.IS_REACT_ACT_ENVIRONMENT = true
globalThis.requestAnimationFrame = (callback) => setTimeout(callback, 0)
globalThis.cancelAnimationFrame = clearTimeout
globalThis.ResizeObserver = class {
  observe() {}
  disconnect() {}
  unobserve() {}
}
globalThis.IntersectionObserver = class {
  observe() {}
  disconnect() {}
  unobserve() {}
}
const scrolls = []
dom.window.HTMLElement.prototype.scrollIntoView = function (options) {
  scrolls.push({ element: this, options })
}
const calls = []
let inboxRows = []
let approvalReply = {
  approvals_required: 2,
  approvals_left: 2,
  approved_by: [],
  user_can_approve: true,
}
let comparison
const jobs = [
  {
    id: 1,
    name: 'build',
    stage: 'compile',
    status: 'success',
    duration: 12,
    web_url: 'https://git.example/jobs/1',
    archived: true,
  },
  {
    id: 2,
    name: 'test',
    stage: 'verify',
    status: 'failed',
    duration: 20,
    web_url: 'https://git.example/jobs/2',
  },
]
// One local draft (M85): critical, on the changed line of api/main.go. Its severity is
// metadata — the assertions below hold it out of the body and out of what is published.
let drafts = [
  {
    id: 'd1',
    review: 'review',
    severity: 'critical',
    body: 'This drops the error.',
    path: 'api/main.go',
    oldPath: 'api/main.go',
    side: 'new',
    line: 1,
    position: {
      position_type: 'text',
      base_sha: 'a'.repeat(40),
      start_sha: 'b'.repeat(40),
      head_sha: 'c'.repeat(40),
      old_path: 'api/main.go',
      new_path: 'api/main.go',
      new_line: 1,
    },
    headSha: 'c'.repeat(40),
    author: { label: 'Review !42', harness: 'claude', run: 'run', conversation: 'conv' },
    createdUnixMs: 0,
    replies: [
      {
        id: 'r1',
        author: { label: 'You', harness: null, run: null, conversation: null },
        body: 'Why critical?',
        createdUnixMs: 1,
      },
      {
        id: 'r2',
        author: { label: 'Review !42', harness: 'claude', run: 'run', conversation: 'conv' },
        body: 'It drops the error the caller retries on.',
        createdUnixMs: 2,
      },
    ],
  },
]
window.__TAURI_INTERNALS__ = {
  transformCallback: () => 1,
  invoke: async (command, args) => {
    calls.push({ command, args })
    if (command === 'gitlab_open_url') return
    // The agent review's harness list (M85): one runnable, one not installed.
    if (command === 'gitlab_review_harnesses')
      return [
        { harness: 'claude', unavailable: null },
        { harness: 'opencode', unavailable: '`opencode` is not on PATH' },
      ]
    if (command === 'gitlab_request') {
      const q = args.request
      const review = {
        id: 'review',
        account: 'account',
        project: 7,
        iid: 42,
        title: 'Review title',
        url: 'https://git.example/p/-/merge_requests/42',
      }
      if (q.kind === 'board')
        return {
          data: {
            revision: 1,
            accounts: [
              {
                id: 'account',
                host: 'https://git.example',
                username: 'me',
                userId: 1,
              },
            ],
            reviews: [review],
            preferences: {
              excludeEnabled: true,
              excludedFiles: ['*.pb.go'],
              reviewPrompt: 'look everywhere',
              reviewPrompts: [
                { repository: 'other/repo', prompt: 'not this one' },
                { repository: 'p', prompt: 'check the cache' },
              ],
            },
          },
          nextPage: null,
        }
      if (q.kind === 'list') return { data: inboxRows, nextPage: null }
      if (q.kind === 'approvals') {
        if (!approvalReply) throw new Error('Approval API unavailable')
        return { data: approvalReply, nextPage: null }
      }
      if (q.kind === 'approve') {
        approvalReply = {
          ...approvalReply,
          approvals_left: q.undo ? 2 : 1,
          approved_by: q.undo
            ? []
            : [{ user: { id: 1, username: 'me', name: 'Me' } }],
        }
        return { data: {}, nextPage: null }
      }
      if (q.kind === 'comparison') return { data: comparison, nextPage: null }
      if (q.kind === 'drafts') return { data: drafts, nextPage: null }
      if (q.kind === 'draftPublish') {
        drafts = drafts.filter((d) => !q.drafts.includes(d.id))
        return {
          data: { published: q.drafts, failed: [] },
          nextPage: null,
        }
      }
      if (q.kind === 'open') return { data: review, nextPage: null }
      const data =
        q.kind === 'pipelines'
          ? [
              {
                id: 8,
                project_id: 7,
                status: 'failed',
                ref: 'feature',
                sha: 'a'.repeat(40),
                web_url: 'https://git.example/pipelines/8',
              },
            ]
          : q.kind === 'jobs'
            ? q.bridges
              ? []
              : jobs
            : q.kind === 'trace'
              ? q.job === 1
                ? {
                    state: 'archived',
                    text: '',
                    message:
                      'This job is archived. GitLab no longer provides its log through the API.',
                  }
                : {
                    state: 'available',
                    text: 'test failed\nstack trace',
                    message: null,
                  }
              : q.kind === 'checkout'
                ? { root: '/review/source' }
                : q.kind === 'sourceTree'
                  ? ['api/main.go', 'api/service.pb.go', 'root.pb.go']
                  : null
      return { data, nextPage: null }
    }
    return null
  },
}
let root
try {
  execFileSync(
    'node',
    [
      'node_modules/vite/bin/vite.js',
      'build',
      '--ssr',
      'src/gitlab/domEntry.tsx',
      '--outDir',
      out,
      '--logLevel',
      'error',
    ],
    { stdio: 'inherit' },
  )
  const {
    Markdown,
    Pipelines,
    FileTree,
    GitDiffView,
    UserLink,
    ReviewPanel,
    ReviewDiff,
    OverlayHost,
    showOverlay,
    GitLabDialogs,
    Thread,
    JobLog,
    refreshBoard,
    refreshApprovals,
    GitLabInbox,
    data,
  } = await import(`file://${resolve(out, 'domEntry.js')}`)
  const overlayHost = () =>
    React.createElement(OverlayHost, {
      project: 'project',
      commands: [],
      context: {},
      keymap: {
        keyFor: () => null,
        chipFor: () => null,
        all: () => [],
        resolve: () => ({ kind: 'unbound' }),
      },
      actions: {},
    })
  const container = document.getElementById('root')
  root = createRoot(container)
  const render = async (element) =>
    act(async () => {
      root.render(element)
    })
  const click = async (element) => {
    assert.ok(element, 'control exists')
    await act(async () => element.click())
  }
  await render(
    React.createElement(Markdown, {
      text: '**Formatted** <em>HTML</em>\n\n<script>window.attacked=true</script><img src=x onerror=alert(1)>[bad](javascript:alert(1))\n\n[Good](https://git.example/user)\n\n```go\nfunc main() {}\n```',
    }),
  )
  assert.equal(container.querySelector('strong')?.textContent, 'Formatted')
  assert.equal(container.querySelector('em')?.textContent, 'HTML')
  assert.match(
    container.querySelector('pre code')?.textContent ?? '',
    /func main/,
  )
  assert.equal(
    container.querySelector('script,img,[onerror],[href^="javascript:"]'),
    null,
  )
  await click(
    [...container.querySelectorAll('a')].find((a) => a.textContent === 'Good'),
  )
  assert.equal(calls.at(-1).args.url, 'https://git.example/user')

  await render(
    React.createElement(UserLink, {
      user: {
        id: 1,
        name: 'Assignee',
        username: 'reviewer',
        web_url: 'https://git.example/prefix/reviewer',
      },
    }),
  )
  await click(container.querySelector('a'))
  assert.equal(calls.at(-1).args.url, 'https://git.example/prefix/reviewer')

  let opened
  await render(
    React.createElement(FileTree, {
      files: [{ path: 'api/main.go' }],
      selected: 'api/main.go',
      label: 'Files',
      onOpen: (path) => {
        opened = path
      },
    }),
  )
  assert.equal(
    container.querySelectorAll('img').length,
    2,
    'folder and file icons',
  )
  await click(container.querySelector('[aria-selected="true"]'))
  assert.equal(opened, 'api/main.go')

  const treePaths = () =>
    [...container.querySelectorAll('[role="treeitem"]')].map((row) => row.title)
  const sortedFiles = [
    'zeta/file.go',
    'apple.go',
    'Zoo.go',
    'README.md',
    '.gitignore',
    'Alpha/zeta/file.go',
    'Alpha/apple.go',
    'Alpha/Zoo.go',
    'Alpha/README.md',
    'Alpha/.gitignore',
    'Alpha/Beta.go',
    'Alpha/beta.go',
    'beta.go',
    'Beta.go',
  ].map((path) => ({ path }))
  const renderTree = (selected) =>
    render(
      React.createElement(FileTree, {
        files: sortedFiles,
        selected,
        label: 'Files',
        onOpen: (path) => {
          opened = path
        },
      }),
    )
  await renderTree(null)
  assert.deepEqual(
    treePaths(),
    [
      'Alpha',
      'zeta',
      '.gitignore',
      'apple.go',
      'Beta.go',
      'beta.go',
      'README.md',
      'Zoo.go',
    ],
    'root folders come first, names ignore case with a deterministic tie-break, and folders start collapsed',
  )
  const alpha = () => container.querySelector('[title="Alpha"]')
  await click(alpha())
  assert.deepEqual(
    treePaths().slice(0, 8),
    [
      'Alpha',
      'Alpha/zeta',
      'Alpha/.gitignore',
      'Alpha/apple.go',
      'Alpha/Beta.go',
      'Alpha/beta.go',
      'Alpha/README.md',
      'Alpha/Zoo.go',
    ],
    'nested folders use the same sorting and stay collapsed',
  )
  await click(alpha())
  assert.equal(alpha().getAttribute('aria-expanded'), 'false')
  scrolls.length = 0
  const navigationFocus = document.activeElement
  await renderTree('Alpha/zeta/file.go')
  assert.ok(
    container.querySelector('[title="Alpha/zeta/file.go"]'),
    'navigation reveals selected ancestors',
  )
  assert.equal(
    scrolls.length,
    1,
    'navigation scrolls after collapsed ancestors mount the selected file',
  )
  assert.equal(scrolls[0].element.title, 'Alpha/zeta/file.go')
  assert.deepEqual(scrolls[0].options, { block: 'nearest', inline: 'nearest' })
  assert.equal(
    document.activeElement,
    navigationFocus,
    'revealing a file preserves keyboard focus',
  )
  assert.equal(
    container.querySelector('[title="zeta"]').getAttribute('aria-expanded'),
    'false',
    'unrelated folders stay collapsed',
  )
  await click(alpha())
  scrolls.length = 0
  await renderTree('Alpha/zeta/file.go')
  assert.equal(
    alpha().getAttribute('aria-expanded'),
    'false',
    'ordinary rerenders preserve manual collapse',
  )
  assert.equal(
    scrolls.length,
    0,
    'ordinary rerenders do not move the sidebar scroll',
  )
  // Expand all / Collapse all (M96): the tree, not each row, holds what is open.
  const expandAll = () => container.querySelector('[aria-label="Expand all"]')
  const collapseAll = () => container.querySelector('[aria-label="Collapse all"]')
  await click(expandAll())
  assert.ok(
    container.querySelector('[title="Alpha/zeta/file.go"]') &&
      container.querySelector('[title="zeta/file.go"]'),
    'Expand all opens every folder, nested ones included',
  )
  await click(collapseAll())
  assert.deepEqual(
    [...container.querySelectorAll('[role="treeitem"][aria-expanded]')].map(
      (row) => [row.title, row.getAttribute('aria-expanded')],
    ),
    [
      ['Alpha', 'false'],
      ['zeta', 'false'],
    ],
    'Collapse all folds every folder',
  )
  await renderTree('zeta/file.go')
  assert.ok(
    container.querySelector('[title="zeta/file.go"]'),
    'a new selection after Collapse all still opens its folder',
  )
  // The maximize toggle: drawn only when the host can hide the rest of the panel, and it
  // reports the flipped state rather than holding one of its own.
  assert.equal(
    container.querySelector('[aria-label="Maximize file tree"]'),
    null,
    'no maximize button without a host to act on it',
  )
  const maximizeCalls = []
  for (const maximized of [false, true]) {
    await render(
      React.createElement(FileTree, {
        files: sortedFiles,
        selected: null,
        onOpen: () => {},
        label: 'Maximize',
        maximized,
        onMaximize: (on) => maximizeCalls.push(on),
      }),
    )
    const toggle = container.querySelector(
      maximized ? '[aria-label="Restore file tree"]' : '[aria-label="Maximize file tree"]',
    )
    assert.equal(toggle?.getAttribute('aria-pressed'), String(maximized))
    assert.equal(
      toggle?.previousElementSibling?.getAttribute('aria-label'),
      'Collapse all',
      'the maximize toggle sits right after Collapse all',
    )
    await click(toggle)
  }
  assert.deepEqual(maximizeCalls, [true, false], 'the toggle reports the flipped state')
  await render(
    React.createElement(FileTree, {
      // Keyed: the prop is read once, which is why ReviewPanel keys the tree on its mode.
      key: 'expanded',
      files: sortedFiles,
      selected: null,
      label: 'Files',
      defaultExpanded: true,
      onOpen: () => {},
    }),
  )
  assert.ok(
    container.querySelector('[title="Alpha/zeta/file.go"]'),
    'defaultExpanded draws nested files on the first render',
  )
  await renderTree('Alpha/zeta/file.go')
  await renderTree('apple.go')
  assert.equal(
    scrolls.at(-1).element.title,
    'apple.go',
    'navigation also scrolls an already mounted file',
  )
  await renderTree('Alpha/zeta/file.go')
  assert.equal(
    scrolls.at(-1).element.title,
    'Alpha/zeta/file.go',
    'navigating back reveals the previous file',
  )

  await render(React.createElement(Pipelines, { review: 'review' }))
  await click(
    [...container.querySelectorAll('button')].find((b) =>
      b.textContent.includes('build'),
    ),
  )
  assert.match(
    container.querySelector('[aria-pressed="true"]')?.textContent ?? '',
    /build/,
  )
  assert.match(
    container.querySelector('[aria-label="Job output"]')?.textContent ?? '',
    /archived/,
  )
  await click(
    [...container.querySelectorAll('button')].find((b) =>
      b.textContent.includes('test'),
    ),
  )
  assert.match(
    container.querySelector('[aria-pressed="true"]')?.textContent ?? '',
    /test/,
  )
  assert.match(
    container.querySelector('[aria-label="Job output"]')?.textContent ?? '',
    /test failed/,
  )
  assert.doesNotMatch(
    container.querySelector('[aria-label="Job output"]')?.textContent ?? '',
    /archived/,
  )
  assert.ok(container.querySelector('[data-status="failed"]'))

  const refs = {
    base_sha: 'a'.repeat(40),
    start_sha: 'b'.repeat(40),
    head_sha: 'c'.repeat(40),
  }
  const user = {
    id: 4,
    username: 'assignee',
    name: 'MR Assignee',
    web_url: 'https://git.example/assignee',
  }
  const change = {
    new_path: 'api/main.go',
    old_path: 'api/main.go',
    diff: '@@ -1 +1 @@\n-old\n+new',
    new_file: false,
    deleted_file: false,
    renamed_file: false,
  }
  data.set('review', {
    mr: {
      iid: 42,
      title: 'Review title',
      description: '**Context**',
      state: 'opened',
      source_branch: 'feature',
      target_branch: 'main',
      author: { ...user, name: 'Author' },
      assignees: [user],
      reviewers: [],
      web_url: 'https://git.example/p/-/merge_requests/42',
    },
    version: {
      id: 1,
      base_commit_sha: refs.base_sha,
      start_commit_sha: refs.start_sha,
      head_commit_sha: refs.head_sha,
      diffs: [
        change,
        { ...change, new_path: 'api/service.pb.go' },
        { ...change, new_path: 'root.pb.go' },
      ],
    },
    versions: [],
    discussions: [],
    drafts,
    commits: [
      {
        id: 'c'.repeat(40),
        short_id: 'cccccccc',
        title: 'Regenerate the bindings',
        author_name: 'Reviewer',
        authored_date: '2026-01-01T00:00:00Z',
        web_url: 'https://gitlab.example/g/p/-/commit/cccccccc',
      },
    ],
    approval: approvalReply,
    approvalError: null,
    activity: [
      {
        id: 1,
        system: true,
        body: 'assigned this MR',
        author: user,
        created_at: '2026-01-01',
      },
      {
        id: 2,
        system: false,
        body: '<b>Review comment</b>',
        author: user,
        created_at: '2026-01-02',
      },
    ],
  })
  await refreshBoard()
  await render(
    React.createElement(
      React.Fragment,
      null,
      React.createElement(ReviewPanel, { review: 'review' }),
      React.createElement(GitLabDialogs),
      overlayHost(),
    ),
  )
  assert.match(
    container.querySelector('section')?.textContent ?? '',
    /Assignees: MR Assignee/,
  )
  assert.match(container.textContent, /Changes \(1\)/)
  const approvalBanner = () =>
    container.querySelector('[aria-label="Overall MR approval status"]')
  assert.equal(approvalBanner().dataset.status, 'pending')
  assert.match(approvalBanner().textContent, /2 approvals remaining/)
  await click(
    [...container.querySelectorAll('button')].find(
      (b) => b.textContent === 'Approve',
    ),
  )
  assert.match(container.textContent, /Remove approval/)
  assert.equal(
    approvalBanner().dataset.status,
    'pending',
    'own approval is separate from MR approval',
  )
  assert.match(approvalBanner().textContent, /1 approval remaining/)
  approvalReply = { ...approvalReply, approvals_left: 0 }
  await act(async () => refreshApprovals('review'))
  assert.equal(approvalBanner().dataset.status, 'approved')
  approvalReply = null
  await act(async () => refreshApprovals('review'))
  assert.equal(
    approvalBanner().dataset.status,
    'unknown',
    'API failure clears the previous green status',
  )
  approvalReply = {
    approvals_required: 1,
    approvals_left: 0,
    approved_by: [{ user: { id: 1, username: 'me', name: 'Me' } }],
  }
  await act(async () => refreshApprovals('review'))
  await click(
    [...container.querySelectorAll('button')].find(
      (b) => b.textContent === 'Activity',
    ),
  )
  assert.equal(
    document.querySelectorAll('[role="dialog"]').length,
    1,
    'MR activity opens exactly one dialog',
  )
  assert.equal(document.querySelector('[aria-label="Show all commands"]'), null)
  assert.match(
    document.querySelector('[aria-label="MR activity and comments"]')
      ?.textContent ?? '',
    /assigned this MR/,
  )
  assert.equal(
    document.querySelector('[aria-label="MR activity and comments"] b')
      ?.textContent,
    'Review comment',
  )
  await click(document.querySelector('[aria-label="Close MR information"]'))
  // The counts are a kit `Counter` after the label, so a button's text is label + count
  // ("Drafts1"). A zero draws no Counter at all, so a fixture with no unresolved thread reads
  // plain "Discussions". Commits sits beside Pipelines.
  const sectionButton = (label) =>
    [
      ...container.querySelectorAll('[aria-label="MR information"] button'),
    ].find((b) => b.firstChild?.nextSibling?.textContent === label || b.textContent === label)
  assert.equal(
    sectionButton('Discussions')?.textContent,
    'Discussions',
    'a zero unresolved count draws no Counter',
  )
  assert.equal(
    sectionButton('Commits')?.textContent,
    'Commits1',
    'the Commits section is counted from the fixture',
  )
  for (const label of ['Overview', 'Discussions', 'Pipelines', 'Commits']) {
    await click(sectionButton(label))
    assert.equal(
      document.querySelectorAll('[role="dialog"]').length,
      1,
      `${label} opens exactly one dialog`,
    )
    assert.equal(
      document.querySelector('[aria-label="Show all commands"]'),
      null,
    )
    await click(document.querySelector('[aria-label="Close MR information"]'))
  }
  assert.doesNotMatch(
    container.querySelector('[aria-label="MR changed files"]')?.textContent ??
      '',
    /pb\.go/,
  )

  // Drafts (M85): counted on the file, listed with a severity badge that is not the body, and
  // published as the body alone.
  const changedApi = container.querySelector(
    '[aria-label="MR changed files"] [title="api"]',
  )
  if (changedApi?.getAttribute('aria-expanded') === 'false')
    await click(changedApi)
  assert.ok(
    container.querySelector('[aria-label="1 unpublished drafts"]'),
    'the changed file shows its draft count',
  )
  assert.equal(sectionButton('Drafts')?.textContent, 'Drafts1')
  await click(sectionButton('Drafts'))
  const card = document.querySelector(
    '[aria-label="Critical draft comment, not published"]',
  )
  assert.ok(card, 'the draft is listed and marked unpublished')
  assert.equal(card.querySelector('[data-severity]')?.textContent, 'Critical')
  assert.doesNotMatch(
    card.querySelector('div')?.textContent ?? '',
    /Critical/,
    'the severity is not part of the comment body',
  )
  // Discuss (M101): the local thread under the draft, the user's and the reviewer's lines told
  // apart, and a composer that opens under the card. Sending needs a host project, which this
  // fixture has none of: the refusal must be said, and nothing may reach Rust.
  const draftThread = card.querySelector('[aria-label="Discussion about this draft"]')
  assert.ok(draftThread, 'the draft shows its discussion')
  assert.deepEqual(
    [...draftThread.querySelectorAll('li')].map((li) => li.dataset.from),
    ['user', 'reviewer'],
  )
  const discuss = [...card.querySelectorAll('button')].find(
    (b) => b.textContent === 'Discuss',
  )
  assert.ok(discuss, "an agent's draft offers Discuss")
  await click(discuss)
  const box = card.querySelector(
    '[aria-label="Message to the reviewer about this draft"]',
  )
  assert.ok(box, 'Discuss opens a composer under the draft')
  await act(async () => {
    const set = Object.getOwnPropertyDescriptor(
      window.HTMLTextAreaElement.prototype,
      'value',
    ).set
    set.call(box, 'Please make it a minor.')
    box.dispatchEvent(new window.Event('input', { bubbles: true }))
  })
  await click(
    [...card.querySelectorAll('button')].find(
      (b) => b.textContent === 'Send to reviewer',
    ),
  )
  await act(async () => new Promise((r) => setTimeout(r, 0)))
  assert.match(
    card.querySelector('[role="alert"]')?.textContent ?? '',
    /Open a workspace project/,
    'a Discuss with no host project says why it went nowhere',
  )
  assert.equal(
    calls.filter((c) => c.command === 'gitlab_draft_discuss').length,
    0,
  )
  await click(
    [...card.querySelectorAll('button')].find((b) => b.textContent === 'Publish'),
  )
  const published = calls.filter(
    (c) => c.args?.request?.kind === 'draftPublish',
  )
  assert.deepEqual(published.at(-1)?.args.request.drafts, ['d1'])
  assert.equal(
    calls.filter((c) => c.args?.request?.kind === 'comment').length,
    0,
    'publishing is one request to Rust, which posts the body; the UI posts nothing itself',
  )
  await click(document.querySelector('[aria-label="Close MR information"]'))

  // The launch dialog: a harness that is not installed cannot be chosen.
  await click(container.querySelector('[aria-label="Review with an agent"]'))
  const harness = document.querySelector('#gitlab-review-harness')
  assert.ok(harness, 'the agent review dialog opens')
  await act(async () => new Promise((r) => setTimeout(r, 0)))
  // Settings → Harness is preselected (M107): the harness the user has already set up, with its
  // binary and arguments — not whichever runnable harness happens to come first in the list.
  assert.equal(harness.value, 'default', 'Default (Settings → Harness) is chosen')
  assert.match(
    harness.querySelector('option[value="default"]')?.textContent ?? '',
    /Default .*Settings/,
    'the default entry says where it comes from',
  )
  assert.equal(
    harness.querySelector('option[value="opencode"]')?.disabled,
    true,
    'an unavailable harness is disabled',
  )
  assert.equal(
    document.querySelector('#gitlab-review-prompt')?.value,
    'check the cache',
    "the instructions start as the MR repository's saved default, not the global one",
  )
  assert.match(
    document.querySelector('[role="dialog"]')?.textContent ?? '',
    /Nothing is posted to GitLab until you publish/,
  )
  await act(async () => showOverlay('commands'))

  await click(
    [...container.querySelectorAll('button')].find(
      (b) => b.textContent === 'Browse MR source',
    ),
  )
  const sourceTree = container.querySelector('[aria-label="MR source files"]')
  assert.ok(sourceTree, 'source browsing opens')
  assert.equal(
    sourceTree.querySelector('[title="api"]').getAttribute('aria-expanded'),
    'false',
  )
  await click(sourceTree.querySelector('[title="api"]'))
  assert.match(sourceTree.textContent, /main.go/)
  assert.doesNotMatch(sourceTree.textContent, /pb\.go/)
  await click(container.querySelector('input[type="checkbox"]'))
  assert.match(
    container.querySelector('[aria-label="MR source files"]').textContent,
    /service.pb.go/,
  )
  assert.match(container.textContent, /Changes \(3\)/)

  await act(async () => showOverlay('commands'))
  assert.equal(document.querySelectorAll('[role="dialog"]').length, 1)
  assert.ok(
    document.querySelector('[aria-label="Show all commands"]'),
    'palette remains available when explicitly requested',
  )
  await act(async () => showOverlay('gitlabOpen'))
  assert.equal(
    document.querySelectorAll('[role="dialog"]').length,
    1,
    'URL dialog replaces the palette',
  )
  assert.equal(document.querySelector('[aria-label="Show all commands"]'), null)
  const urlInput = document.querySelector('#gitlab-mr-url')
  assert.equal(
    document.activeElement,
    urlInput,
    'MR dialog focuses URL for paste',
  )
  await act(async () => {
    Object.getOwnPropertyDescriptor(
      window.HTMLInputElement.prototype,
      'value',
    ).set.call(urlInput, 'https://git.example/p/-/merge_requests/42')
    urlInput.dispatchEvent(new window.Event('input', { bubbles: true }))
  })
  const submit = document.querySelector('button[type="submit"]')
  assert.equal(submit.disabled, false)
  await act(async () => urlInput.form.requestSubmit())
  assert.ok(
    calls.some(
      (c) =>
        c.command === 'gitlab_request' &&
        c.args.request.kind === 'open' &&
        c.args.request.url.endsWith('/42'),
    ),
  )

  const thread = {
    id: 't',
    individual_note: false,
    notes: [
      {
        id: 1,
        body: 'Inline review note',
        author: user,
        created_at: '2026-01-01',
        system: false,
        resolvable: true,
        resolved: false,
      },
    ],
  }
  await render(
    React.createElement(Thread, { review: 'review', thread, inline: true }),
  )
  assert.match(container.textContent, /Inline review note/)
  await render(
    React.createElement(Thread, {
      review: 'review',
      inline: true,
      thread: { ...thread, notes: [{ ...thread.notes[0], resolved: true }] },
    }),
  )
  assert.doesNotMatch(container.textContent, /Inline review note/)
  await click(container.querySelector('[aria-expanded="false"]'))
  assert.match(container.textContent, /Inline review note/)
  await render(
    React.createElement(JobLog, {
      text: '\x1b[31mfailed\x1b[0m\r\nsection_start:1:build[collapsed=true]\r\x1b[0KBuild\noutput\nsection_end:3:build\r\x1b[0K',
      search: '',
    }),
  )
  assert.equal(
    [...container.querySelectorAll('span')].find(
      (s) => s.textContent === 'failed',
    )?.style.color,
    'rgb(214, 59, 59)',
  )
  assert.doesNotMatch(container.textContent, /output/)
  await click(container.querySelector('[aria-expanded="false"]'))
  assert.match(container.textContent, /output/)

  const positions = []
  const diff = {
    path: 'main.go',
    oldPath: null,
    status: 'modified',
    binary: false,
    oldMode: 33188,
    newMode: 33188,
    new: { kind: 'commit', oid: 'new' },
    old: { kind: 'commit', oid: 'old' },
    newOid: 'new',
    oldOid: 'old',
    oldText: 'old\n',
    newText: 'new\n',
    textsOmitted: false,
    hunks: [
      {
        index: 0,
        header: '@@ -1 +1 @@',
        oldStart: 1,
        oldLines: 1,
        newStart: 1,
        newLines: 1,
        lines: [
          {
            origin: 'deletion',
            content: 'old',
            oldLineno: 1,
            newLineno: null,
            noNewline: false,
          },
          {
            origin: 'addition',
            content: 'new',
            oldLineno: null,
            newLineno: 1,
            noNewline: false,
          },
        ],
      },
    ],
  }
  for (const view of ['unified', 'split']) {
    await render(
      React.createElement(GitDiffView, {
        readOnly: true,
        diff,
        path: 'main.go',
        revisions: { from: 'old', to: 'new' },
        view,
        collapsed: new Set(),
        busy: false,
        note: null,
        reason: null,
        onCollapse: () => {},
        onReviewLine: (side, line) => positions.push({ side, line }),
        renderAfterLine: (oldLine, newLine, side) =>
          newLine === 1 && side !== 'old'
            ? React.createElement('p', null, 'Anchored inline thread')
            : null,
      }),
    )
    await click(
      container.querySelector('[data-review-side="old"][data-review-line="1"]'),
    )
    await click(
      container.querySelector('[data-review-side="new"][data-review-line="1"]'),
    )
    assert.equal(
      container.querySelectorAll('[data-audit="gitDiffAnnotation"]').length,
      1,
    )
    assert.equal(
      container.querySelector('[data-audit="gitDiffApply"]'),
      null,
      'review never offers staging',
    )
  }
  assert.deepEqual(positions, [
    { side: 'old', line: 1 },
    { side: 'new', line: 1 },
    { side: 'old', line: 1 },
    { side: 'new', line: 1 },
  ])
  comparison = diff
  data.set('review', {
    ...data.get('review'),
    discussions: [
      {
        ...thread,
        notes: [
          {
            ...thread.notes[0],
            body: 'Deleted-line discussion',
            position: {
              ...refs,
              old_path: 'main.go',
              new_path: 'main.go',
              old_line: 1,
              new_line: null,
            },
          },
        ],
      },
    ],
  })
  await render(
    React.createElement(ReviewDiff, {
      document: { review: 'review', path: 'main.go', mode: 'diff', refs },
      visible: true,
    }),
  )
  const annotation = container.querySelector('[data-audit="gitDiffAnnotation"]')
  assert.ok(annotation, 'null new_line anchors the thread on the old side')
  assert.match(annotation.textContent, /Deleted-line discussion/)
  assert.equal(
    container.querySelectorAll('[data-audit="gitDiffAnnotation"]').length,
    1,
  )
  inboxRows = [
    {
      ...data.get('review').mr,
      id: 42,
      iid: 42,
      title: 'Review title',
      updated_at: '2026-09-01',
      user_notes_count: 2,
      references: { full: 'group/service!42' },
    },
    {
      ...data.get('review').mr,
      id: 43,
      iid: 43,
      title: 'Draft: Improve retries',
      updated_at: '2026-09-02',
      user_notes_count: 0,
      draft: true,
      web_url: 'https://git.example/p/-/merge_requests/43',
    },
  ]
  await render(
    React.createElement(
      React.Fragment,
      null,
      React.createElement(GitLabInbox),
      React.createElement(GitLabDialogs),
      overlayHost(),
    ),
  )
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 350))
  })
  assert.equal(container.querySelector('[aria-label="GitLab settings"]'), null)
  assert.doesNotMatch(
    container.textContent,
    /Connections & filters|Personal access token/,
  )
  assert.equal(container.querySelectorAll('article').length, 2)
  assert.match(
    container.querySelector('article').textContent,
    /Improve retries/,
    'MR rows sort by latest update across accounts',
  )
  assert.ok(container.querySelector('[data-state="draft"]'))
  assert.match(container.textContent, /group\/service/)
  // The scope is the kit's `Segmented` since the redesign: a radio group whose segments are
  // named by their visible label, and "on" is `aria-checked`, not `aria-pressed`.
  const scopeSegment = (label) =>
    [...container.querySelectorAll('[role="radiogroup"][aria-label="MR relationship"] [role="radio"]')]
      .find((b) => b.textContent === label)
  await click(scopeSegment('Created'))
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 300))
  })
  assert.equal(
    calls.filter((c) => c.args?.request?.kind === 'list').at(-1).args.request
      .scope,
    'created',
  )
  assert.equal(
    scopeSegment('Created').getAttribute('aria-checked'),
    'true',
  )
  await click(
    container.querySelector('[aria-label="Open merge request by URL"]'),
  )
  assert.equal(document.querySelectorAll('[role="dialog"]').length, 1)
  assert.equal(document.activeElement, document.querySelector('#gitlab-mr-url'))
  console.log(
    'GitLab DOM: sanitized Markdown/HTML, profile links, tree icons, job selection/logs and shared diff comment gutters passed',
  )
} finally {
  if (root) await act(async () => root.unmount())
  dom.window.close()
  rmSync(out, { recursive: true, force: true })
}
