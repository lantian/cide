/**
 * The OpenSpec scene's wire data: cide's own `openspec/` as it would read mid-way through the
 * PTY coalescer work `data/git.ts` tells the story of.
 *
 * Wire shapes (`SpecBoard`, `SpecChange`), not the panel's view props — the scene answers
 * `spec_board` / `spec_change`, and `adapt.ts` does the rest, so a field renamed in Rust fails
 * `tsc` here rather than drawing an empty panel on the site. The shapes follow
 * `sidebar/OpenSpecPanel/fixture.ts`; the content is cide's instead of that fixture's `thing`.
 */
import type { SpecArtifactText, SpecBoard, SpecChange } from '../../ipc/generated'
import { HOME } from '../world'

const CHANGES = `${HOME}/openspec/changes`

export const SPEC_BOARD: SpecBoard = {
  kind: 'ready',
  root: HOME,
  commands: [
    { name: 'explore', line: '/openspec-explore' },
    { name: 'propose', line: '/openspec-propose' },
  ],
  changes: [
    { name: 'split-pty-coalescer', completedTasks: 5, totalTasks: 9, status: 'in-progress' },
    { name: 'add-output-backpressure', completedTasks: 2, totalTasks: 7, status: 'in-progress' },
    { name: 'detach-panes-to-windows', completedTasks: 11, totalTasks: 11, status: 'complete' },
    { name: 'remote-session-handoff', completedTasks: 0, totalTasks: 0, status: 'no-tasks' },
  ],
  specs: [
    { id: 'terminal-output', requirementCount: 9 },
    { id: 'session-lifecycle', requirementCount: 12 },
    { id: 'pane-layout', requirementCount: 14 },
    { id: 'git-integration', requirementCount: 21 },
    { id: 'task-tracker', requirementCount: 17 },
    { id: 'language-servers', requirementCount: 8 },
    { id: 'claude-hosting', requirementCount: 11 },
  ],
}

const dir = `${CHANGES}/split-pty-coalescer`

export const SPEC_CHANGE: SpecChange = {
  name: 'split-pty-coalescer',
  title: 'Split the PTY coalescer out of the session reader',
  origin: { kind: 'active' },
  progress: {
    completed: 5,
    total: 9,
    tasks: [
      { done: true, description: '1.1 Move the flush timer and byte budget into coalesce.rs' },
      { done: true, description: '1.2 Keep Session::read_loop a plain reader that hands chunks over' },
      { done: true, description: '1.3 Flush on a frame boundary, never mid escape sequence' },
      { done: true, description: '2.1 Route payloads under 4 KiB through webview.eval on the main loop' },
      { done: true, description: '2.2 Unit-test the split at a CSI, an OSC and a UTF-8 boundary' },
      { done: false, description: '3.1 Pause the reader when the channel is 8 chunks behind' },
      { done: false, description: '3.2 Surface a stalled pane as awaiting instead of a frozen one' },
      { done: false, description: '4.1 Measure yes | head -c 200M against the old path' },
      { done: false, description: '4.2 Record the numbers in the journal' },
    ],
  },
  artifacts: [
    { id: 'proposal', generates: 'proposal.md', state: 'done', existing: [`${dir}/proposal.md`] },
    { id: 'design', generates: 'design.md', state: 'done', existing: [`${dir}/design.md`] },
    {
      id: 'specs',
      generates: 'specs/**/*.md',
      state: 'done',
      existing: [`${dir}/specs/terminal-output/spec.md`, `${dir}/specs/session-lifecycle/spec.md`],
    },
    { id: 'tasks', generates: 'tasks.md', state: 'ready', existing: [`${dir}/tasks.md`] },
  ],
  deltas: [
    {
      spec: 'terminal-output',
      operation: 'added',
      description: 'Coalesce output on frame boundaries',
      requirements: [
        {
          name: 'Frame-boundary flush',
          text: 'The coalescer SHALL flush a pane’s pending output only at the end of a complete escape sequence or UTF-8 code point.',
          scenarios: [
            {
              title: 'A CSI straddles two reads',
              body: '- **WHEN** a read ends inside ESC [ 3 8 ; 5\n- **THEN** the partial sequence is held until the next read completes it',
            },
            {
              title: 'The budget fills mid code point',
              body: '- **WHEN** the byte budget is reached inside a multi-byte character\n- **THEN** the flush moves back to the previous code point boundary',
            },
          ],
          block: '### Requirement: Frame-boundary flush',
        },
        {
          name: 'Reader backpressure',
          text: 'The session reader SHALL stop reading from the PTY while its pane’s channel holds more than eight unacknowledged chunks.',
          scenarios: [
            {
              title: 'A flood of output',
              body: '- **WHEN** a child writes faster than the webview paints\n- **THEN** the reader pauses and the child blocks on its own write',
            },
          ],
          block: '### Requirement: Reader backpressure',
        },
      ],
    },
    {
      spec: 'terminal-output',
      operation: 'modified',
      description: 'Small payloads take the main-loop path',
      requirements: [
        {
          name: 'Payload routing',
          text: 'Output payloads under 4 KiB SHALL be delivered through webview.eval on the GTK main loop; larger ones SHALL use the session Channel.',
          scenarios: [
            {
              title: 'An interactive keystroke echo',
              body: '- **WHEN** a shell echoes one character\n- **THEN** it arrives by webview.eval within one frame',
            },
          ],
          block: '### Requirement: Payload routing',
        },
      ],
    },
    {
      spec: 'session-lifecycle',
      operation: 'renamed',
      description: 'Name the stalled state for what it is',
      rename: { from: 'Frozen session', to: 'Stalled session' },
      requirements: [
        {
          name: 'Stalled session',
          text: 'A session whose reader is paused for backpressure SHALL be shown as stalled, and SHALL resume without losing output once its pane catches up.',
          scenarios: [
            {
              title: 'The pane catches up',
              body: '- **WHEN** the channel drains below two chunks\n- **THEN** the reader resumes and the pane repaints from where it stopped',
            },
          ],
          block: '### Requirement: Stalled session',
        },
      ],
    },
    {
      spec: 'session-lifecycle',
      operation: 'removed',
      description: 'The fixed 16 ms flush timer is gone',
      requirements: [
        {
          name: 'Fixed flush interval',
          text: 'The reader SHALL flush pending output every 16 ms.',
          scenarios: [],
          block: '### Requirement: Fixed flush interval',
        },
      ],
    },
  ],
  validation: { valid: true, issues: [] },
}

/** What `spec_artifact` answers for the proposal, should the page ask for its text. */
export const PROPOSAL: SpecArtifactText = {
  truncated: false,
  text: `## Why

\`Session::read_loop\` both reads the PTY and decides when to flush, so a flood of output
(\`cargo build\`, \`yes\`) starves the GTK main loop and every pane in the window stops painting.

## What Changes

- Move coalescing into \`crates/cide-pty/src/coalesce.rs\`, flushing only on frame boundaries.
- Pause the reader when a pane's channel falls eight chunks behind.
- **BREAKING** for nothing outside \`cide-pty\`; the wire is unchanged.

## Impact

- Affected specs: \`terminal-output\`, \`session-lifecycle\`
- Affected code: \`crates/cide-pty/src/{coalesce,session,lib}.rs\`
`,
}
