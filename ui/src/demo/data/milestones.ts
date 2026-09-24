/**
 * The demo project's milestone plan, as `milestones_get` answers it: the backpressure epic from
 * `data/tasks.ts` cut into goals, each with a gate — a command whose exit 0 means "met".
 *
 * The state picked is the busiest honest one: the first milestone accepted, the active one with
 * its gate failing on the last run and re-running now, a verify in flight on the coder's branch,
 * and two proposals from agents waiting in the inbox for the user to rule on.
 */
import type { CheckResult, MilestonePlan, MilestonesView, MilestoneTask, TaskStatus } from '../../ipc/generated'
import { NOW, TASKS } from './tasks'

const MIN = 60_000

export const PLAN: MilestonePlan = {
  items: [
    {
      id: 'm1',
      title: 'Coalescer lives in its own module',
      gate: 'cargo test --locked -p cide-pty coalesce',
      timeoutSecs: 600,
    },
    {
      id: 'm2',
      title: 'Bounded memory under a stalled webview',
      task: 't-40',
      gate: 'cargo test --locked -p cide-pty --test backpressure && pnpm --dir ui run check:render-stall',
      timeoutSecs: 1800,
    },
    {
      id: 'm3',
      title: 'Docs and journal say what shipped',
      gate: 'pnpm --dir ui run check:casing && ./scripts/check-docs.sh pty',
    },
    {
      id: 'm4',
      title: 'Release 0.14 with the new coalescer',
      gate: 'cargo --locked xtask package --check',
      timeoutSecs: 3600,
    },
  ],
  active: 'm2',
  verify: 'cargo test --locked -p cide-pty && pnpm --dir ui run check:attach',
  maxOpen: 8,
  guardPaths: ['crates/cide-pty/src/vt.rs', 'contract/'],
}

const M1_PASS: CheckResult = {
  command: PLAN.items[0]?.gate ?? '',
  passed: true,
  exitCode: 0,
  timedOut: false,
  tail: 'running 14 tests\n..............\ntest result: ok. 14 passed; 0 failed; 0 ignored; finished in 0.41s',
  startedUnixMs: NOW - 6 * 60 * MIN,
  durationMs: 48_000,
  head: 'd85b1a0',
}

/** The last run of m2's gate: the Rust half passes, the render-stall replay does not — yet. */
const M2_FAILED = [
  '$ cargo test --locked -p cide-pty --test backpressure',
  '    Finished `test` profile [unoptimized + debuginfo] target(s) in 21.84s',
  '     Running tests/backpressure.rs (target/debug/deps/backpressure-5f1c2a9e0d3b7c41)',
  '',
  'running 4 tests',
  'test a_stalled_sink_keeps_the_ring_bounded ... ok',
  'test hold_is_released_on_ack ... ok',
  'test spill_goes_to_scrollback_not_memory ... ok',
  'test exit_flushes_before_the_last_frame ... ok',
  '',
  'test result: ok. 4 passed; 0 failed; 0 ignored; finished in 1.92s',
  '',
  '$ pnpm --dir ui run check:render-stall',
  '> check:render-stall: node scripts/check-render-stall.mjs',
  '',
  '  ✓ a hidden tab paints on reveal',
  '  ✓ a 1 MB burst lands in one frame',
  '  ✗ a 10 MB burst under Pressure::Hold paints within 250 ms  (took 412 ms)',
  '  ✗ scrollback after a spill is contiguous  (gap at row 18 204)',
  '',
  '12 passed, 2 failed',
  'ELIFECYCLE  Command failed with exit code 1.',
].join('\n')

/** The run in flight now, after the coder's fix: the Rust half again, the replay under way. */
export const M2_LOG = [
  ...M2_FAILED.split('\n').slice(0, 15),
  '  ✓ a hidden tab paints on reveal',
  '  ✓ a 1 MB burst lands in one frame',
  '  … a 10 MB burst under Pressure::Hold',
].join('\n')

const M2_FAIL: CheckResult = {
  command: PLAN.items[1]?.gate ?? '',
  passed: false,
  exitCode: 1,
  timedOut: false,
  tail: M2_FAILED,
  startedUnixMs: NOW - 26 * MIN,
  durationMs: 94_000,
  head: 'f921463',
}

function tasksOf(ids: Array<[string, number]>): MilestoneTask[] {
  return ids.flatMap(([id, depth]) => {
    const t = TASKS.find((row) => row.id === id)
    if (!t) return []
    const out: MilestoneTask = { id, title: t.title, status: t.status as TaskStatus, depth }
    if (t.agent) out.agent = t.agent
    return [out]
  })
}

export function milestonesView(project: string): MilestonesView {
  return {
    project,
    plan: PLAN,
    gates: [
      { milestone: 'm1', last: M1_PASS, running: false },
      { milestone: 'm2', last: M2_FAIL, running: true, log: M2_LOG },
    ],
    accepted: ['m1'],
    tasks: [
      { milestone: 'm1', tasks: tasksOf([['t-36', 0], ['t-35', 0], ['t-34', 0]]) },
      {
        milestone: 'm2',
        tasks: tasksOf([['t-40', 0], ['t-41', 1], ['t-42', 1], ['t-43', 1], ['t-39', 1], ['t-31', 1], ['t-45', 0]]),
      },
      { milestone: 'm3', tasks: tasksOf([['t-44', 0], ['t-47', 0]]) },
      { milestone: 'm4', tasks: [] },
    ],
    verifies: [
      { task: 't-41', agent: 'coder', running: true },
      {
        task: 't-37',
        agent: 'coder',
        running: false,
        last: {
          command: PLAN.verify,
          passed: true,
          exitCode: 0,
          timedOut: false,
          tail: 'test result: ok. 41 passed; 0 failed\ncheck:attach ✓ 9 scenarios',
          startedUnixMs: NOW - 20 * MIN,
          durationMs: 71_000,
        },
      },
    ],
    proposals: [
      {
        id: 'p-3',
        title: 'Split m2: land the bounded channel before the render-stall replay',
        rationale:
          'The Rust half of the gate passes today. Gating the channel on a UI check that needs t-45 first holds t-43 hostage; a milestone per half lets the coder merge now.',
        by: { kind: 'orchestrator' },
        createdUnixMs: NOW - 11 * MIN,
        task: 't-40',
        change: {
          kind: 'plan',
          before: PLAN,
          plan: {
            ...PLAN,
            items: [
              ...PLAN.items.slice(0, 1),
              { id: 'm2', title: 'Bounded frame channel', task: 't-40', gate: 'cargo test --locked -p cide-pty --test backpressure' },
              { id: 'm2b', title: 'A stalled webview repaints in 250 ms', gate: 'pnpm --dir ui run check:render-stall' },
              ...PLAN.items.slice(2),
            ],
          },
        },
      },
      {
        id: 'p-4',
        title: 'Raise the render-stall budget for the 10 MB scenario',
        rationale:
          'A 10 MB burst is 40x the largest real frame seen in the journal; 250 ms is the budget for ordinary output. Proposing 500 ms for this one scenario only.',
        by: { kind: 'agent', agent: 'tester', label: 'Tester' },
        createdUnixMs: NOW - 4 * MIN,
        task: 't-45',
        change: {
          kind: 'files',
          files: [
            {
              path: 'ui/scripts/check-render-stall.mjs',
              diff: [
                '@@ -88,7 +88,8 @@ const SCENARIOS = [',
                "   { name: 'a 1 MB burst lands in one frame', bytes: 1 << 20, budgetMs: 250 },",
                "-  { name: 'a 10 MB burst under Pressure::Hold', bytes: 10 << 20, budgetMs: 250 },",
                '+  // 40x the largest frame the journal records; ordinary output keeps 250 ms.',
                "+  { name: 'a 10 MB burst under Pressure::Hold', bytes: 10 << 20, budgetMs: 500 },",
              ].join('\n'),
            },
          ],
        },
      },
    ],
  }
}
