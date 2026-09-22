import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'
mkdirSync('node_modules/.cache', { recursive: true })
const out = mkdtempSync('node_modules/.cache/cide-gitlab-render-')
try {
  execFileSync(
    'node',
    [
      'node_modules/vite/bin/vite.js',
      'build',
      '--ssr',
      'src/gitlab/smokeEntry.tsx',
      '--outDir',
      out,
      '--logLevel',
      'error',
    ],
    { stdio: 'inherit' },
  )
  globalThis.self = globalThis
  globalThis.window = globalThis
  globalThis.location = { search: '' }
  const printed = []
  const log = console.log
  try {
    console.log = (line) => printed.push(line)
    await import(`file://${resolve(out, 'smokeEntry.js')}`)
  } finally {
    console.log = log
  }
  const { panel, discussions } = JSON.parse(printed.at(-1))
  assert.match(panel, /Changes \(1\)/)
  assert.match(panel, /1 hidden/)
  assert.match(panel, /\+2/)
  assert.match(panel, /−1/)
  assert.match(panel, /role="treeitem" aria-expanded="false"[^>]*title="src"/)
  assert.doesNotMatch(panel, /main.go/, 'nested files stay hidden until their folder is opened')
  assert.doesNotMatch(panel, /generated.pb.go/)
  assert.match(panel, /Approve/)
  assert.match(panel, /Browse MR source/)
  assert.match(panel, /Pipelines/)
  assert.match(panel, /role="tree"/)
  assert.match(discussions, /Please handle the failure/)
  assert.match(discussions, /Resolve thread/)
  assert.match(discussions, /Reply/)
  for (const html of [panel, discussions])
    assert.doesNotMatch(html, /class="[^"]*undefined/)
  console.log(
    'GitLab render: filtered tree and counters, review controls and discussions passed',
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}
