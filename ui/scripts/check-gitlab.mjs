import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
const out = mkdtempSync(join(tmpdir(), 'cide-gitlab-'))
try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/gitlab/model.ts',
      'src/gitlab/types.ts',
      'src/gitlab/jobLogModel.ts',
      '--outDir',
      out,
      '--module',
      'esnext',
      '--target',
      'es2023',
      '--moduleResolution',
      'bundler',
      '--strict',
      '--skipLibCheck',
    ],
    { stdio: 'inherit' },
  )
  const {
    globRegex,
    visibleChanges,
    changeTotals,
    linePosition,
    pathVisibility,
    fileDiscussions,
    threadResolved,
    approvalStatus,
  } = await import(join(out, 'model.js'))
  const approval = {
    approved_by: [{ user: { id: 1 } }],
    approvals_required: 2,
    approvals_left: 1,
  }
  assert.equal(
    approvalStatus(approval).kind,
    'pending',
    'a personal approval does not satisfy all MR requirements',
  )
  assert.equal(
    approvalStatus({ ...approval, approvals_left: 0 }).kind,
    'approved',
    'GitLab may omit the approved boolean',
  )
  assert.equal(
    approvalStatus({
      approved_by: [],
      approvals_required: 0,
      approvals_left: 0,
    }).kind,
    'optional',
  )
  assert.equal(approvalStatus(null).kind, 'unknown')
  assert.equal(
    approvalStatus({ approved_by: approval.approved_by }).kind,
    'unknown',
    'individual approvers alone do not prove overall approval',
  )
  assert.equal(
    approvalStatus({ ...approval, approvals_left: 0 }, true).kind,
    'unknown',
    'new commits must not retain a green stale approval',
  )
  const change = (path, diff) => ({
    old_path: path,
    new_path: path,
    diff,
    new_file: false,
    deleted_file: false,
    renamed_file: false,
  })
  const diff = '@@ -2,3 +2,4 @@\n context\n-old\n+new\n+extra\n end'
  const source = change('service/main.go', diff),
    generated = change('api/service.pb.go', diff),
    grpc = change('service_grpc.pb.go', diff)
  const patterns = ['**/*.pb.go', '**/*_grpc.pb.go']
  assert.deepEqual(visibleChanges([source, generated, grpc], patterns, true), [
    source,
  ])
  assert.equal(
    visibleChanges([source, generated, grpc], patterns, false).length,
    3,
  )
  assert.equal(globRegex('**/*.pb.go').test('root.pb.go'), true)
  assert.equal(globRegex('*.pb.go').test('api/nested/service.pb.go'), true)
  assert.equal(globRegex(' *.pb.go ').test('root.pb.go'), true)
  assert.deepEqual(
    ['main.go', 'api/service.pb.go', 'root.pb.go'].filter(
      pathVisibility(['*.pb.go'], true),
    ),
    ['main.go'],
  )
  assert.deepEqual(
    visibleChanges([source, generated, grpc], ['*.pb.go'], true),
    [source],
  )
  assert.equal(globRegex('api/*.go').test('api/sub/a.go'), false)
  assert.deepEqual(changeTotals([source]), {
    files: 1,
    additions: 2,
    deletions: 1,
    complete: true,
  })
  assert.equal(changeTotals([{ ...source, too_large: true }]).complete, false)
  assert.deepEqual(
    changeTotals([
      change('main.go', '@@ -1 +1 @@\n---deleted content\n+++added content'),
    ]),
    { files: 1, additions: 1, deletions: 1, complete: true },
  )
  const refs = { base_sha: 'base', start_sha: 'start', head_sha: 'head' }
  assert.deepEqual(linePosition(source, refs, 'old', 3), {
    ...refs,
    position_type: 'text',
    old_path: source.old_path,
    new_path: source.new_path,
    old_line: 3,
  })
  assert.deepEqual(linePosition(source, refs, 'new', 4), {
    ...refs,
    position_type: 'text',
    old_path: source.old_path,
    new_path: source.new_path,
    new_line: 4,
  })
  assert.equal(linePosition(source, refs, 'new', 1), null)
  assert.equal(linePosition(source, refs, 'new', 5).old_line, 4)
  const renamed = {
    ...source,
    old_path: 'old/name.go',
    new_path: 'new/name.go',
  }
  assert.equal(linePosition(renamed, refs, 'old', 3).old_path, 'old/name.go')
  const deleted = {
    ...source,
    diff: '@@ -1,2 +0,0 @@\n-first\n-second',
    deleted_file: true,
  }
  assert.equal(linePosition(deleted, refs, 'old', 2).new_line, undefined)
  const thread = (resolved, path, replies = 0) => ({
    id: path,
    individual_note: false,
    notes: Array.from({ length: replies + 1 }, () => ({
      resolvable: true,
      resolved,
      position: { ...refs, old_path: 'old/name.go', new_path: path },
    })),
  })
  const threads = [
    thread(false, 'new/name.go', 3),
    thread(true, 'new/name.go', 2),
    thread(false, 'other.go'),
  ]
  assert.deepEqual(
    fileDiscussions(threads.slice(0, 2), 'new/name.go', 'old/name.go'),
    { resolved: 1, unresolved: 1 },
  )
  assert.deepEqual(fileDiscussions(threads, 'missing.go'), {
    resolved: 0,
    unresolved: 0,
  })
  assert.equal(threadResolved({ notes: [] }), false)
  assert.equal(
    threadResolved({
      notes: [
        { resolvable: true, resolved: true },
        { resolvable: true, resolved: false },
      ],
    }),
    false,
  )
  const { parseJobLog } = await import(join(out, 'jobLogModel.js'))
  const lines = parseJobLog('\x1b[31mred\r\ncontinued\x1b[0m plain\n10%\r20%')
  assert.equal(lines[0].text, 'red')
  assert.equal(lines[1].spans[0].style.color, lines[0].spans[0].style.color)
  assert.deepEqual(lines[1].spans[1].style, {})
  assert.equal(lines[2].text, '20%')
  assert.equal(
    parseJobLog('\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\ after')[0]
      .text,
    'link after',
  )
  const sections = parseJobLog(
    'section_start:100:build[collapsed=true]\r\x1b[0KBuild\nbuilding\nsection_end:103:build\r\nafter',
  )
  assert.equal(sections[0].kind, 'section')
  assert.equal(sections[0].collapsed, true)
  assert.equal(sections[0].duration, 3)
  assert.equal(sections[0].children[0].text, 'building')
  assert.equal(sections[1].text, 'after')
  console.log(
    'GitLab: filters, totals, immutable positions, thread counters, and ANSI log parsing passed',
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}
