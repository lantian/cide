/**
 * When an unsaved buffer is kept on disk, and whether a kept one is offered back. (M117)
 *
 * # Why this exists
 *
 * A buffer lives only in its `EditorView`. Autosave writes it to the *file* after a minute of
 * idleness or five minutes of typing — and until then a power cut, an OOM kill or a crash took
 * the work with it. Every other thing cide keeps across a restart was on disk within a second.
 * So a dirty buffer is also snapshotted to the state directory (`cide_core::buffers`), a moment
 * after the typing pauses, and a file opened with such a snapshot behind it comes back dirty with
 * the snapshot's text over the disk's.
 *
 * # The cost, and what bounds it
 *
 * A snapshot is `doc.toString()`, one IPC frame and one fsynced write. Per keystroke that would
 * be the per-gesture disk cost the position store's whole ladder exists to avoid, so it is not:
 *
 * * **quiet** — `BACKUP_QUIET_MS` after the *last* change, so a burst of typing is one snapshot;
 * * **ceiling** — at most `BACKUP_CEILING_MS` after the *first* unsnapshotted change, so a
 *   person who never pauses still loses at most that much;
 * * **large buffers** skip the quiet snapshot and take only the ceiling: stringifying and
 *   shipping several megabytes every second and a half of typing is the cost this module refuses
 *   to pay, and autosave still writes those files to disk in the ordinary way.
 *
 * Pure and import-free, so `check:editor` compiles it standalone and drives the table.
 */

/** A snapshot this long after the last change. */
export const BACKUP_QUIET_MS = 1_500

/** …and never later than this after the first change it has not yet covered. */
export const BACKUP_CEILING_MS = 10_000

/** Above this many characters a buffer is snapshotted on the ceiling alone. */
export const BACKUP_LARGE_CHARS = 2 * 1024 * 1024

/**
 * The two delays for a buffer of `chars` characters: the quiet one, or `null` when the buffer is
 * too large for it, and the ceiling.
 */
export function backupDelays(chars: number): { quiet: number | null; ceiling: number } {
  return {
    quiet: chars > BACKUP_LARGE_CHARS ? null : BACKUP_QUIET_MS,
    ceiling: BACKUP_CEILING_MS,
  }
}

/**
 * What to do with a kept buffer found when a file opens.
 *
 * * `none` — nothing was kept;
 * * `drop` — something was, and it says what the disk already says (the buffer was saved and
 *   the clear never landed, or it was typed back to the saved text): forget it, restore nothing;
 * * `restore` — it holds work the disk does not: open the file dirty, with this text.
 *
 * The comparison ignores line endings. The kept text is the editor's, which is always `\n`; the
 * disk's is verbatim, and a CRLF file would otherwise never compare equal to its own snapshot.
 */
export function restoreDecision(kept: string | null, disk: string): 'none' | 'drop' | 'restore' {
  if (kept === null) return 'none'
  return kept === disk.replace(/\r\n?/g, '\n') ? 'drop' : 'restore'
}
