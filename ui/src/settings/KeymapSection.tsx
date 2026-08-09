/**
 * Settings → Keymap: the resolved bindings, and the conflict detector.
 *
 * The detection is `cide_core::keymap::conflicts`, a pure function over resolved bindings,
 * and it is deliberately not reimplemented here. Doing it in the frontend would need the key
 * normaliser — `Ctrl+Shift+P`, `shift+ctrl+p` and `ctrl+shift+p` are one keystroke — and a
 * second implementation of that is a second set of bugs, in the module whose stated failure
 * mode is "my keybinding does nothing, with no visible cause".
 *
 * Conflicts are *reported*, never resolved. Which of two commands should own a key is a
 * judgement only the user can make; picking one silently is how the mistake stays hidden. The
 * list shows them in resolution order and says which one currently wins.
 */
import { useEffect, useMemo, useState } from 'react'
import { settings as settingsApi, type KeymapReport } from '@/ipc/client'
import { Note } from './controls'
import styles from './panels.module.css'

export function KeymapSection() {
  const [report, setReport] = useState<KeymapReport | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [filter, setFilter] = useState('')

  useEffect(() => {
    let cancelled = false
    void settingsApi
      .keymap()
      .then((next) => {
        if (!cancelled) setReport(next)
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(String(e))
      })
    return () => {
      cancelled = true
    }
  }, [])

  const rows = useMemo(() => {
    if (report === null) return []
    const needle = filter.trim().toLowerCase()
    if (needle === '') return report.bindings
    return report.bindings.filter(
      (b) => b.key.includes(needle) || b.command.toLowerCase().includes(needle),
    )
  }, [report, filter])

  if (error !== null) {
    return <Note title="The keymap could not be read">{error}</Note>
  }
  if (report === null) {
    return <div className={styles.clean}>Reading the keymap…</div>
  }

  return (
    <>
      {report.conflicts.length === 0 && report.problems.length === 0 && (
        <div className={styles.clean}>No conflicts. Every keystroke reaches one command.</div>
      )}

      {report.conflicts.map((conflict) => (
        <div key={`${conflict.key}${conflict.when ?? ''}`} className={styles.conflict}>
          <span className={styles.keyChip}>{conflict.key}</span> is bound to{' '}
          {conflict.commands.length} commands
          {conflict.when !== null && <> when <code>{conflict.when}</code></>}:{' '}
          {conflict.commands.join(', ')}.{' '}
          {/* Resolution order is the contract: later layers win, and within a layer the last
              entry does. Naming the winner is what makes this actionable rather than alarming. */}
          <strong>{conflict.commands[conflict.commands.length - 1]}</strong> wins.
        </div>
      ))}

      {report.problems.map((problem, i) => (
        <div key={`${problem.key}-${problem.command}-${i}`} className={`${styles.conflict} ${styles.problem}`}>
          {problem.key !== '' && <span className={styles.keyChip}>{problem.key}</span>}{' '}
          {problem.command !== '' && <code className={styles.command}>{problem.command}</code>}{' '}
          {problem.message}
        </div>
      ))}

      <input
        className={styles.filter}
        type="search"
        placeholder="Filter by key or command"
        aria-label="Filter keybindings"
        value={filter}
        onChange={(e) => setFilter(e.target.value)}
      />

      <table className={styles.table}>
        <thead>
          <tr>
            <th>Key</th>
            <th>Command</th>
            <th>When</th>
            <th>Source</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((binding, i) => (
            <tr key={`${binding.key}-${binding.command}-${i}`}>
              <td>
                <span className={styles.keyChip}>{binding.key}</span>
              </td>
              <td className={styles.command}>{binding.command}</td>
              <td className={styles.layer}>{binding.when ?? ''}</td>
              <td
                className={
                  binding.layer === 'user' ? `${styles.layer} ${styles.layerUser}` : styles.layer
                }
              >
                {binding.layer}
              </td>
            </tr>
          ))}
        </tbody>
      </table>

      <div className={styles.path}>
        Overrides: {report.path}
        {/* Said explicitly because an absent file is the normal state of a fresh install, and
            a path shown with no qualifier reads as "this exists and you have edited it". */}
        {' — user overrides only; the defaults above are compiled in.'}
      </div>
    </>
  )
}
