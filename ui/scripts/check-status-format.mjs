/**
 * Checks `src/store/statusFormat.ts` against a statusline payload captured verbatim from a
 * real Claude Code 2.1.226 session.
 *
 * This project has no JS test runner, and adding one for a handful of pure functions would
 * be a larger commitment than the code it tests. But this particular formatter is worth
 * pinning: its input shape is undocumented and unversioned, and its output is a line in the
 * design mock that can be checked exactly. The first version of `compact` rendered the
 * mock's own example, 128400, as `128k` instead of `128.4k`.
 *
 * Run: `pnpm --dir ui run check:format`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-fmt-'))
try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/store/statusFormat.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
    ],
    { stdio: 'inherit' },
  )

  const { formatClaude, compact, usedTokens } = await import(
    `file://${join(out, 'statusFormat.js')}`
  )

  // Captured from a real session, not written by hand.
  const fresh = {
    context_window: {
      context_window_size: 1000000,
      current_usage: null,
      total_input_tokens: 0,
      total_output_tokens: 0,
      used_percentage: null,
    },
    cost: { total_cost_usd: 0 },
    model: { display_name: 'Opus 5 (1M context)', id: 'claude-opus-5[1m]' },
    session_id: '9c8fb189',
    version: '2.1.226',
  }
  const midturn = {
    ...fresh,
    context_window: { ...fresh.context_window, current_usage: 128400 },
    cost: { total_cost_usd: 1.2345 },
  }

  let failed = 0
  const eq = (actual, expected, what) => {
    if (actual !== expected) {
      console.error(`FAIL ${what}\n  actual:   ${JSON.stringify(actual)}\n  expected: ${JSON.stringify(expected)}`)
      failed++
    }
  }

  eq(usedTokens(fresh), 0, 'a fresh session has used zero tokens, which is not the same as not reporting')
  eq(usedTokens({}), undefined, 'no context window means not reported')
  eq(compact(128400), '128.4k', "the design mock's own figure")
  eq(compact(1000000), '1M', 'a whole magnitude drops its .0')
  eq(compact(840), '840', 'below a thousand is written out')
  eq(compact(1500000), '1.5M', 'millions keep their tenth')
  eq(
    formatClaude(midturn),
    'claude · Opus 5 (1M context) · 128.4k / 1M · $1.23',
    'the mid-turn readout matches the mock',
  )
  eq(formatClaude(fresh).includes('$'), false, 'a zero cost is not shown as $0.00')
  eq(formatClaude({}), undefined, 'nothing usable yields no readout rather than a bare "claude"')
  eq(
    formatClaude({ model: { display_name: 'Opus 5' } }),
    'claude · Opus 5',
    'a renamed field costs one part, not the whole readout',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('status format: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
