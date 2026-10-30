import { useEffect, useRef, useState } from 'react'
import { ConfirmDestructive } from '@/chrome/ConfirmDestructive'
import { notifyFailure } from '@/chrome/notices'
import {
  localHistory,
  settings as api,
  type LocalHistorySettings,
  type LocalHistoryStatus,
  type ProjectId,
} from '@/ipc/client'
import { InfoPara } from '@/kit/components/InfoTip'
import { ActionButton, Group, NumberField, Row, Segmented, ToggleRow, resetTo } from './controls'

/** The three states of a project's switch. Absence on disk is `inherit`, not a third boolean. */
type ProjectChoice = 'inherit' | 'on' | 'off'

const PROJECT_CHOICES = [
  { value: 'inherit', label: 'Use global' },
  { value: 'on', label: 'On' },
  { value: 'off', label: 'Off' },
] as const

function choiceOf(value: boolean | null): ProjectChoice {
  if (value === null) return 'inherit'
  return value ? 'on' : 'off'
}

function megabytes(bytes: number): string {
  return `${(bytes / 1048576).toFixed(1)} MiB`
}

function valueOf(choice: ProjectChoice): boolean | null {
  return choice === 'inherit' ? null : choice === 'on'
}

interface Props {
  project: ProjectId | null
  value: LocalHistorySettings
  defaults: LocalHistorySettings | undefined
  onChange: (next: LocalHistorySettings) => void
}

/**
 * Local History's switches and limits.
 *
 * The project's override is read here rather than through `SectionProps`, because it lives in
 * `local-history-settings.json` and not in `Settings`: it is one machine's decision about one
 * project, which is `IndexingExclusions`' arrangement too. `epoch` drops an answer that arrives
 * after the viewer has moved to another project.
 */
export function LocalHistoryGroup({ project, value, defaults, onChange }: Props) {
  const [choice, setChoice] = useState<ProjectChoice>('inherit')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [status, setStatus] = useState<LocalHistoryStatus | null>(null)
  const [clearing, setClearing] = useState(false)
  const epoch = useRef(0)
  const set = (next: Partial<LocalHistorySettings>) => onChange({ ...value, ...next })

  const refresh = () => {
    if (project === null) return
    void localHistory.status(project).then(setStatus, () => setStatus(null))
  }

  useEffect(() => {
    const attempt = ++epoch.current
    setError(null)
    if (project === null) return
    refresh()
    void api.projectLocalHistory(project).then(
      (stored) => {
        if (attempt === epoch.current) setChoice(choiceOf(stored))
      },
      (reason) => {
        if (attempt === epoch.current) setError(String(reason))
      },
    )
  }, [project])

  const choose = (next: ProjectChoice) => {
    if (project === null) return
    const attempt = ++epoch.current
    setSaving(true)
    setError(null)
    void api
      .setProjectLocalHistory(project, valueOf(next))
      .then(
        (stored) => {
          if (attempt === epoch.current) setChoice(choiceOf(stored))
        },
        (reason) => {
          if (attempt === epoch.current) setError(String(reason))
        },
      )
      .finally(() => {
        if (attempt === epoch.current) setSaving(false)
      })
  }

  return (
    <Group title="Local History">
      <ToggleRow
        label="Keep Local History"
        hint="Saves versions of project files as they change, with or without git."
        info={
          <>
            <InfoPara>
              Records saves and outside changes, including an agent's, so any version or deleted
              file can be restored. The first open of a project copies its files, in the background
              and compressed, so a file can be recovered before cide has ever opened it.
            </InfoPara>
            <InfoPara>
              Gitignored files, folders excluded from indexing and <code>.cide</code> are not kept.
              Turning this off stops recording and keeps the versions already saved.
            </InfoPara>
          </>
        }
        {...resetTo(value.enabled, defaults?.enabled, (v) => set({ enabled: v }))}
        checked={value.enabled}
        onChange={(v) => set({ enabled: v })}
      />
      {project !== null && (
        <Row
          label="This project"
          anchor="Local History for this project"
          hint="Saved on this machine. Overrides the switch above for this project only."
          status={error === null ? undefined : { tone: 'bad', text: error }}
          control={
            <Segmented
              label="Local History for this project"
              value={choice}
              options={PROJECT_CHOICES}
              onChange={(next) => {
                if (!saving) choose(next)
              }}
            />
          }
        />
      )}
      <Row
        label="Days to keep"
        hint="Working days per file, counting only days that file changed. Default: 5."
        {...resetTo(value.daysToKeep, defaults?.daysToKeep, (v) => set({ daysToKeep: v }))}
        control={
          <NumberField
            label="Days to keep"
            value={value.daysToKeep}
            min={1}
            max={3650}
            step={1}
            onChange={(v) => set({ daysToKeep: v })}
          />
        }
      />
      <Row
        label="Largest file to keep"
        hint="MiB. Larger files are listed as changed, with no copy kept. Default: 5."
        {...resetTo(value.maxFileSizeMiB, defaults?.maxFileSizeMiB, (v) =>
          set({ maxFileSizeMiB: v }),
        )}
        control={
          <NumberField
            label="Largest file to keep (MiB)"
            value={value.maxFileSizeMiB}
            min={1}
            max={1024}
            step={1}
            onChange={(v) => set({ maxFileSizeMiB: v })}
          />
        }
      />
      <Row
        label="Storage per project"
        hint="MiB of compressed versions. Default: 1024."
        info={
          <InfoPara>
            Oldest versions go first, then deleted files. The newest version of a file that still
            exists is never removed, so a project larger than this can exceed it.
          </InfoPara>
        }
        {...resetTo(value.maxStorageMiB, defaults?.maxStorageMiB, (v) => set({ maxStorageMiB: v }))}
        control={
          <NumberField
            label="Storage per project (MiB)"
            value={value.maxStorageMiB}
            min={64}
            max={1048576}
            step={64}
            onChange={(v) => set({ maxStorageMiB: v })}
          />
        }
      />
      {project !== null && status !== null && status.rows > 0 && (
        <Row
          label="Clear this project's history"
          hint={`${status.rows.toLocaleString('en-US')} versions, ${megabytes(status.diskBytes)} on disk.`}
          control={
            <ActionButton label="Clear History…" onClick={() => setClearing(true)} />
          }
        />
      )}
      {clearing && project !== null && (
        <ConfirmDestructive
          state={{
            title: 'Clear Local History?',
            body: 'Every saved version of this project is deleted. This cannot be undone. The current files are not touched.',
            files: [],
            confirmLabel: 'Clear History',
            danger: true,
            run: () => {},
          }}
          onCancel={() => setClearing(false)}
          onConfirm={() => {
            setClearing(false)
            localHistory.clear(project).then(refresh, notifyFailure)
          }}
        />
      )}
    </Group>
  )
}
