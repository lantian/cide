/**
 * Who runs an OpenSpec session: the main model on a harness, or — for Apply — a subagent role.
 *
 * The harness half is the MR reviewer's choice (`gitlab/Dialogs.tsx`'s `LaunchReviewDialog`),
 * the one the user pointed at: **Default** is Settings → Harness, the CLI their own consoles run,
 * resolved when the session starts and never copied into this state; the concrete entries below
 * it are for choosing another one on purpose, greyed with the sentence the Agents panel would
 * show when one is not installed.
 *
 * Composed from the kit's `Select` and `Segmented` here rather than added to the kit, because it
 * reads the app's stores and IPC — which the kit may never import (`docs/ui-kit.md` rule 6).
 */
import { useEffect, useState } from 'react'
import { Field } from '@/kit/components/Field'
import { Select } from '@/kit/components/Select'
import { Segmented } from '@/kit/components/Choice'
import { settings, specSessions } from '@/ipc/client'
import type { GitLabReviewHarness, Harness, SpecLauncher } from '@/ipc/generated'
import { useAgents } from '../agentsStore'
import { notifyFailure } from '@/chrome/notices'
import { HARNESS_NAME } from './model'

export type LauncherMode = 'main' | 'role'

export interface LauncherChoice {
  mode: LauncherMode
  harness: Harness | 'default'
  role: string | null
}

export const DEFAULT_CHOICE: LauncherChoice = { mode: 'main', harness: 'default', role: null }

/** The console harness Settings names — `Default` resolves to this. */
function useConsoleHarness(): Harness | null {
  const project = useAgents((s) => s.project)
  const [harness, setHarness] = useState<Harness | null>(null)
  useEffect(() => {
    let alive = true
    setHarness(null)
    if (project === null) return
    void settings.effective(project).then((settings) => {
      if (alive) setHarness(settings.consoleHarness)
    }).catch(notifyFailure)
    return () => {
      alive = false
    }
  }, [project])
  return harness
}

function useHarnesses(): { list: GitLabReviewHarness[] | null; error: string | null } {
  const [list, setList] = useState<GitLabReviewHarness[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    let alive = true
    specSessions.harnesses().then(
      (next) => alive && setList(next),
      (e: unknown) => alive && setError(e instanceof Error ? e.message : String(e)),
    )
    return () => {
      alive = false
    }
  }, [])
  return { list, error }
}

/**
 * The launcher the choice resolves to, or the sentence saying why it cannot start yet.
 * `consoleHarness` is `null` until Settings has been read.
 */
export function resolveLauncher(
  choice: LauncherChoice,
  consoleHarness: Harness | null,
  harnesses: readonly GitLabReviewHarness[] | null,
): { launcher: SpecLauncher } | { why: string } {
  if (choice.mode === 'role') {
    if (choice.role === null) return { why: 'Choose the role that implements it.' }
    return { launcher: { kind: 'role', agent: choice.role } }
  }
  const harness = choice.harness === 'default' ? consoleHarness : choice.harness
  if (harness === null) return { why: 'Reading Settings → Harness…' }
  const unavailable = harnesses?.find((h) => h.harness === harness)?.unavailable ?? null
  if (unavailable !== null) return { why: unavailable }
  return { launcher: { kind: 'harness', harness } }
}

export function LauncherPicker({
  choice,
  onChange,
  allowRoles,
  onResolved,
}: {
  choice: LauncherChoice
  onChange: (next: LauncherChoice) => void
  /** Apply only: a subagent role may implement a change; a proposal is the main model's. */
  allowRoles: boolean
  /** The launcher this choice resolves to, reported on every change. */
  onResolved: (resolved: { launcher: SpecLauncher } | { why: string }) => void
}) {
  const consoleHarness = useConsoleHarness()
  const { list, error } = useHarnesses()
  const roster = useAgents((state) => state.roster)
  const roles = roster.kind === 'ready' ? roster.agents : []

  const resolved = resolveLauncher(choice, consoleHarness, list)
  useEffect(() => {
    onResolved(resolved)
    // `resolved` is rebuilt every render; its content is what matters.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [JSON.stringify(resolved)])

  const harnessOptions = [
    {
      value: 'default',
      label: `Default${consoleHarness === null ? '' : ` — ${HARNESS_NAME[consoleHarness] ?? consoleHarness}`}`,
      detail: 'Settings → Harness',
    },
    ...(list ?? []).map((h) => ({
      value: h.harness,
      label: HARNESS_NAME[h.harness] ?? h.harness,
      disabled: h.unavailable !== null,
      ...(h.unavailable !== null ? { detail: 'unavailable' } : {}),
    })),
  ]
  const roleOptions = roles.map((role) => ({
    value: role.id,
    label: role.label.trim() === '' ? role.id : role.label,
    detail: role.unavailable !== null ? 'unavailable' : (HARNESS_NAME[role.harness] ?? role.harness),
    disabled: role.unavailable !== null,
  }))

  return (
    <>
      {allowRoles && (
        <Segmented
          label="Who implements it"
          value={choice.mode}
          block
          onChange={(mode) => onChange({ ...choice, mode })}
          options={[
            { value: 'main', label: 'Main model' },
            { value: 'role', label: 'Subagent' },
          ]}
        />
      )}
      {choice.mode === 'main' ? (
        <Field
          label="Harness"
          error={error}
          hint={'why' in resolved && choice.harness !== 'default' ? resolved.why : undefined}
        >
          {({ id, describedBy }) => (
            <Select
              id={id}
              aria-describedby={describedBy}
              value={choice.harness}
              disabled={list === null}
              placeholder="Checking installed harnesses…"
              options={harnessOptions}
              onChange={(value) => onChange({ ...choice, harness: value as Harness | 'default' })}
            />
          )}
        </Field>
      ) : (
        <Field
          label="Role"
          hint={
            roles.length === 0
              ? 'This project has no roles cide can run yet — define one, and turn subagents on, in the Agents panel.'
              : 'Runs with the role’s own prompt, model and tools.'
          }
        >
          {({ id, describedBy }) => (
            <Select
              id={id}
              aria-describedby={describedBy}
              value={choice.role}
              disabled={roles.length === 0}
              placeholder="Choose a role"
              options={roleOptions}
              onChange={(value) => onChange({ ...choice, role: value })}
            />
          )}
        </Field>
      )}
    </>
  )
}
