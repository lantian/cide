/**
 * Settings → Appearance: the Linux graphics ladder.
 *
 * Three environment variables that work around WebKitGTK rendering failures, cheapest first:
 * `__NV_DISABLE_EXPLICIT_SYNC` → `WEBKIT_DISABLE_DMABUF_RENDERER` →
 * `WEBKIT_DISABLE_COMPOSITING_MODE`.
 *
 * This screen exists because no probe can pick the right rung. WebGL context creation
 * succeeds on a software rasteriser, `WEBGL_debug_renderer_info` is masked on Linux — it
 * reports "Apple GPU" on every box — and a DMABUF failure presents as a window that never
 * paints rather than as an error. So the decision is the user's, and what this screen owes
 * them is the cost of each rung and an honest statement of what is actually in effect.
 *
 * Two things it must not pretend:
 *
 * * **A change takes effect on the next launch, never on this one.** These variables are read
 *   while the webview is created. The `restartRequired` chip is not a nicety; without it the
 *   switch looks broken.
 * * **Setting any rung stands the automatic ladder down.** The launcher applies its defaults
 *   with set-if-unset semantics, so there is no value this side could write that means "and
 *   do not apply the default either". Overriding one rung therefore makes the stored settings
 *   the whole ladder, and the banner says so.
 */
import { Badge } from '@/kit/components/Status'
import { useCallback, useEffect, useState } from 'react'
import {
  settings as settingsApi,
  type GraphicsSettings,
  type GraphicsStatus,
} from '@/ipc/client'
import { Note, Segmented } from './controls'
import styles from './panels.module.css'

/** Which stored field each rung's variable drives. */
const FIELD: Record<string, keyof GraphicsSettings> = {
  __NV_DISABLE_EXPLICIT_SYNC: 'disableNvidiaExplicitSync',
  WEBKIT_DISABLE_DMABUF_RENDERER: 'disableDmabufRenderer',
  WEBKIT_DISABLE_COMPOSITING_MODE: 'disableCompositingMode',
}

type Choice = 'auto' | 'on' | 'off'

const CHOICES: readonly { value: Choice; label: string }[] = [
  { value: 'auto', label: 'Auto' },
  { value: 'on', label: 'On' },
  { value: 'off', label: 'Off' },
]

function choiceOf(setting: boolean | null): Choice {
  if (setting === null) return 'auto'
  return setting ? 'on' : 'off'
}

function settingOf(choice: Choice): boolean | null {
  if (choice === 'auto') return null
  return choice === 'on'
}

export interface GraphicsLadderProps {
  graphics: GraphicsSettings
  onChange: (next: GraphicsSettings) => void
}

export function GraphicsLadder({ graphics, onChange }: GraphicsLadderProps) {
  const [status, setStatus] = useState<GraphicsStatus | null>(null)

  const refresh = useCallback(() => {
    void settingsApi
      .graphics()
      .then(setStatus)
      .catch(() => setStatus(null))
  }, [])

  // Re-read after every stored change: `active` comes from this process's environment and
  // `restartRequired` is the comparison between the two, so a stale status would show a
  // switch that has moved and a chip that has not.
  useEffect(refresh, [refresh, graphics])

  if (status === null) {
    return <div className={styles.clean}>Reading the graphics configuration…</div>
  }

  return (
    <>
      {status.suppressedByEnv && (
        <Note title="Workarounds are disabled for this run">
          <code>CIDE_NO_GRAPHICS_WORKAROUNDS</code> is set in the environment cide was launched
          from, which overrides everything below.
        </Note>
      )}
      {!status.automatic && !status.suppressedByEnv && (
        <Note title="You are driving the ladder">
          One or more rungs is set explicitly, so the automatic ladder is off and only what you
          have chosen here is applied. Set every rung back to <em>Auto</em> to hand it back.
        </Note>
      )}

      {status.rungs.map((rung) => {
        const field = FIELD[rung.variable]
        const restart = rung.setting !== null && rung.setting !== rung.active
        return (
          <div key={rung.variable} className={styles.rung}>
            <div className={styles.rungText}>
              <div className={styles.rungLabel}>{rung.label}</div>
              <div className={styles.rungVar}>
                {rung.variable}
                {rung.active ? ' — set in this process' : ' — not set in this process'}
              </div>
              <div className={styles.rungCost}>{rung.cost}</div>
              {restart && (
                <span className={styles.restart}>
                  <Badge tone="yellow" soft>
                    takes effect on the next launch
                  </Badge>
                </span>
              )}
            </div>
            <Segmented
              label={rung.label}
              value={choiceOf(rung.setting)}
              options={CHOICES}
              onChange={(choice) => {
                // A rung whose variable this build does not know about cannot be stored, and
                // silently doing nothing would be a switch that springs back.
                if (field === undefined) return
                onChange({ ...graphics, [field]: settingOf(choice) })
              }}
            />
          </div>
        )
      })}
    </>
  )
}
