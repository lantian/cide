/**
 * What the `git` scene adds to the base repository state: the diff behind the file the commit
 * window has open, and a shelf with something on it.
 */
import type { FileDiff, ShelfEntry } from '../../ipc/generated'
import { wire } from '../world'
import { hunks, SESSION_BASE, SESSION_OURS } from './git-session'

/** The file the scene's diff tab is open on — the heart of the change. */
export const DIFF_PATH = 'crates/cide-pty/src/session.rs'

/** `git_diff_file` for `session.rs`, HEAD → working tree, as the changelist row compares it. */
export function sessionDiff(side: FileDiff['side']): FileDiff {
  return {
    path: DIFF_PATH,
    oldPath: null,
    side,
    status: 'modified',
    binary: false,
    oldMode: 0o100644,
    newMode: 0o100644,
    hunks: hunks(SESSION_BASE, SESSION_OURS),
    rev: 'b7e41c09d2a35f68',
    partialOk: true,
    oldText: SESSION_BASE,
    newText: SESSION_OURS,
    textsOmitted: false,
  }
}

/** A plain one-hunk diff for any other row the scene might open: `lib.rs`'s new re-export. */
export function otherDiff(path: string, side: FileDiff['side']): FileDiff {
  const before = 'pub mod session;\npub mod vt;\n\npub use session::Session;\n'
  const after = 'pub mod coalesce;\npub mod session;\npub mod vt;\n\npub use coalesce::{Coalescer, Pressure};\npub use session::Session;\n'
  return { ...sessionDiff(side), path, hunks: hunks(before, after), oldText: before, newText: after, rev: 'c01d2e3f4a5b6c7d' }
}

const NOW = 1_790_000_000

/** The shelf: two things put aside while the coalescer took priority. */
export const SHELF: ShelfEntry[] = [
  {
    id: 'shelf-0002',
    name: 'Scrollback ring: grow by pages',
    created: wire(NOW - 2 * 3600),
    files: ['crates/cide-pty/src/ring.rs', 'crates/cide-pty/src/vt.rs'],
  },
  {
    id: 'shelf-0001',
    name: 'WIP: tool window remembers its height per project',
    created: wire(NOW - 26 * 3600),
    files: ['ui/src/toolwindow/toolWindowHeight.ts', 'crates/cide-core/src/workspace.rs', 'ui/src/toolwindow/ToolWindow.tsx'],
  },
]

/** What the commit box holds: a message written the house way, subject then the why. */
export const COMMIT_MESSAGE =
  'Hold PTY frames past the ack window\n\n' +
  'push() answers Pressure::Hold once the\n' +
  'webview is a window behind, so a runaway\n' +
  'child stalls in its own write(), not in\n' +
  'our heap.'
