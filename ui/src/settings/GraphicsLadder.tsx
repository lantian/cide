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
 *   while the webview is created. The row's "takes effect on the next launch" status is not a
 *   nicety; without it the switch looks broken.
 * * **Setting any rung stands the automatic ladder down.** The launcher applies its defaults
 *   with set-if-unset semantics, so there is no value this side could write that means "and
 *   do not apply the default either". Overriding one rung therefore makes the stored settings
 *   the whole ladder, and the Workaround ladder row says so.
 */
import { useCallback, useEffect, useState } from 'react'
import {
  settings as settingsApi,
  type GraphicsSettings,
  type GraphicsStatus,
} from '@/ipc/client'
import { InfoPara } from '@/kit/components/InfoTip'
import { Readout, Row, Segmented } from './controls'
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

  // The ladder's own state is a row of its own, where the two notes above the rungs used to
  // be (M133): which of three things is driving it is a fact about this group, and a readout
  // with a status says it without a coloured block the user has to tie to a row.
  const driver = status.suppressedByEnv ? 'Disabled for this run' : status.automatic ? 'Automatic' : 'Manual'
  return (
    <>
      <Row
        label="Workaround ladder"
        hint="Set a rung below to take over; set every rung to Auto to hand it back."
        status={
          status.suppressedByEnv
            ? { tone: 'warn', text: 'CIDE_NO_GRAPHICS_WORKAROUNDS is set, which overrides every rung.' }
            : undefined
        }
        info={
          status.suppressedByEnv ? (
            <>
              <code>CIDE_NO_GRAPHICS_WORKAROUNDS</code> is set in the environment cide was
              launched from, which overrides everything below.
            </>
          ) : (
            'Automatic applies the rungs this machine is known to need. Setting any rung explicitly turns the automatic ladder off, and only what you have chosen is applied.'
          )
        }
        control={<Readout text={driver} />}
      />

      {status.rungs.map((rung) => {
        const field = FIELD[rung.variable]
        const restart = rung.setting !== null && rung.setting !== rung.active
        const set = (choice: Choice) => {
          // A rung whose variable this build does not know about cannot be stored, and
          // silently doing nothing would be a switch that springs back.
          if (field === undefined) return
          onChange({ ...graphics, [field]: settingOf(choice) })
        }
        return (
          <Row
            key={rung.variable}
            label={rung.label}
            hint={rung.active ? 'Set in this process.' : 'Not set in this process.'}
            info={
              <>
                <InfoPara>
                  <code>{rung.variable}</code>
                </InfoPara>
                <InfoPara>{rung.cost}</InfoPara>
              </>
            }
            // Read by WebKit while the webview is created, so a changed rung is a fact about
            // the next launch — the row says so rather than a chip that looks like a state.
            status={restart ? { tone: 'info', text: 'Takes effect on the next launch.' } : undefined}
            // Auto (`null`) is every rung's default.
            modified={rung.setting !== null}
            onReset={() => set('auto')}
            control={
              <Segmented
                label={rung.label}
                value={choiceOf(rung.setting)}
                options={CHOICES}
                onChange={set}
              />
            }
          />
        )
      })}
    </>
  )
}
