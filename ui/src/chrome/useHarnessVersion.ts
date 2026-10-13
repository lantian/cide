import { useEffect, useState } from 'react'
import { settings as settingsApi, type HarnessCliVersion, type ProjectId, type Settings } from '@/ipc/client'
import type { ProjectHarnessSettings } from '@/ipc/generated'
import { defaultHarnessReadout, HARNESS_LOADING, harnessVersionKey, type HarnessReadout } from './harnessReadout'

/** Rust resolves project overrides, whose keys may be canonical paths the UI cannot infer. */
export function useHarnessVersion(
  project: ProjectId | null,
  settings: Settings | null,
  overrides: Record<string, ProjectHarnessSettings> | undefined,
): HarnessReadout {
  const key = harnessVersionKey(project, settings, overrides)
  const [answer, setAnswer] = useState<{ key: string; info: HarnessCliVersion | null } | null>(null)
  useEffect(() => {
    if (key === null) return
    let live = true
    void settingsApi.harnessVersion(project).then(
      (info) => { if (live) setAnswer({ key, info }) },
      () => { if (live) setAnswer({ key, info: null }) },
    )
    return () => { live = false }
  }, [key, project])
  // Gating during render hides the old answer immediately, before the next effect runs.
  return answer !== null && answer.key === key ? defaultHarnessReadout(answer.info) : HARNESS_LOADING
}
