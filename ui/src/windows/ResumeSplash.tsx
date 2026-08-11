/**
 * What a secondary Claude pane shows after a restart, instead of spawning.
 *
 * A restored workspace knows which session each pane held, but a session that outlived the
 * app is a conversation the user may or may not want back — and starting one per pane on
 * launch would bill for a screenful of conversations nobody asked to resume. So a secondary
 * pane comes back as this splash and waits for a click.
 *
 * The design mock's idle Claude pane is the reference for everything above the control: the
 * accent tile, the name with its dim version, the model line and the cwd, all in the mono
 * stack at the mock's 12/19. What replaces the mock's prompt box is the one thing this
 * surface does.
 *
 * Styling is inline rather than a CSS module because the hover lift is the only state and a
 * second stylesheet for four boxes buys nothing. If this grows, it wants a module.
 */
import { useEffect, useState, type CSSProperties, type ReactNode } from 'react'

const MINUTE = 60_000
const HOUR = 60 * MINUTE
const DAY = 24 * HOUR

/**
 * "just now", "5m ago", "2h ago", "3d ago" — nothing finer, and no dependency.
 *
 * `Intl.RelativeTimeFormat` would produce "5 minutes ago", which is twice the width in a
 * control that also has to carry a session title.
 */
export function relativeTime(at: number, now: number = Date.now()): string {
  const elapsed = now - at
  // Also the branch a timestamp from the future takes: a clock that moved backwards between
  // runs should read as "just now", never as a negative age.
  if (elapsed < MINUTE) return 'just now'
  if (elapsed < HOUR) return `${Math.floor(elapsed / MINUTE)}m ago`
  if (elapsed < DAY) return `${Math.floor(elapsed / HOUR)}h ago`
  return `${Math.floor(elapsed / DAY)}d ago`
}

/**
 * Re-render once a minute so the age stays true.
 *
 * A splash sits untouched for as long as the user ignores it, and "just now" is only true
 * for the first of those minutes. One interval per idle pane is a cost worth paying to stop
 * the surface lying about how old the session is.
 */
function useMinuteTick(active: boolean): void {
  const [, bump] = useState(0)
  useEffect(() => {
    if (!active) return
    const id = window.setInterval(() => bump((n) => n + 1), MINUTE)
    return () => window.clearInterval(id)
  }, [active])
}

const rootStyle: CSSProperties = {
  height: '100%',
  display: 'flex',
  flexDirection: 'column',
  alignItems: 'flex-start',
  gap: 16,
  padding: '18px 20px',
  background: 'var(--panel-2)',
  fontFamily: 'var(--font-mono)',
  fontSize: 12,
  lineHeight: '19px',
  color: 'var(--dim)',
  // A pane can be short; the splash scrolls rather than clipping the resume control, which
  // is the only thing here the user can act on.
  overflow: 'auto',
}

const tileStyle: CSSProperties = {
  width: 38,
  height: 38,
  flex: 'none',
  borderRadius: 9,
  background: 'var(--accent)',
}

/* The lines carry their rhythm from the 19px line height, so the block adds no gaps. */
const linesStyle: CSSProperties = { display: 'flex', flexDirection: 'column', maxWidth: '100%' }

const nameStyle: CSSProperties = { color: 'var(--text-hi)' }

const versionStyle: CSSProperties = { color: 'var(--faint)', marginLeft: 8 }

const pathStyle: CSSProperties = {
  color: 'var(--faint)',
  overflow: 'hidden',
  textOverflow: 'ellipsis',
  whiteSpace: 'nowrap',
}

export interface ResumeSplashProps {
  /** The session's title, shown in the control so the user knows what they are resuming. */
  title: string
  /** Epoch milliseconds of the session's last activity; null when nothing recorded it. */
  lastActive: number | null
  onResume: () => void
  /** The `claude` version, when the caller knows it. Omitted rather than invented. */
  version?: string | undefined
  /** The model line, e.g. `model: sonnet`. Omitted rather than invented. */
  model?: string | undefined
  /** The session's working directory. */
  cwd?: string | undefined
}

export function ResumeSplash({
  title,
  lastActive,
  onResume,
  version,
  model,
  cwd,
}: ResumeSplashProps): ReactNode {
  // Two flags rather than one. Sharing a single one lets a pointer that passes over a
  // control the user reached with Tab clear the lift on its way out, taking the keyboard
  // focus indicator with it while the control is still what Enter would activate.
  const [hovered, setHovered] = useState(false)
  const [focused, setFocused] = useState(false)
  const lifted = hovered || focused
  useMinuteTick(lastActive !== null)

  const when = lastActive === null ? null : relativeTime(lastActive)
  const label = when === null ? `Resume "${title}"` : `Resume "${title}" · ${when}`

  const controlStyle: CSSProperties = {
    alignSelf: 'stretch',
    textAlign: 'left',
    padding: '9px 12px',
    borderRadius: 8,
    // Longhand rather than the `border` shorthand: the colour is the only part that moves,
    // and mixing the two in one style object leaves the result depending on key order.
    borderWidth: 1,
    borderStyle: 'solid',
    // The lift is the whole affordance — nothing else on this surface is clickable, so the
    // box has to say so on hover and on keyboard focus alike.
    borderColor: lifted ? 'var(--accent)' : 'var(--border)',
    background: 'transparent',
    color: lifted ? 'var(--text-hi)' : 'var(--text)',
    font: 'inherit',
    cursor: 'pointer',
  }

  return (
    <div style={rootStyle} data-audit="resumeSplash">
      <div style={tileStyle} aria-hidden="true" />

      <div style={linesStyle}>
        <div style={nameStyle}>
          Claude Code
          {version !== undefined && <span style={versionStyle}>v{version}</span>}
        </div>
        {model !== undefined && <div>{model}</div>}
        {cwd !== undefined && (
          <div style={pathStyle} title={cwd}>
            {cwd}
          </div>
        )}
      </div>

      <button
        type="button"
        style={controlStyle}
        onClick={onResume}
        onPointerEnter={() => setHovered(true)}
        onPointerLeave={() => setHovered(false)}
        onFocus={() => setFocused(true)}
        onBlur={() => setFocused(false)}
      >
        {label}
      </button>
    </div>
  )
}

/*
 * `RestoredShellBanner` was here, and it is deliberately gone rather than merely unused.
 *
 * It rendered `— session restored (previous output not retained) —` as a `<p>` above a
 * restored shell's terminal, and it broke the pane it was explaining: `PaneTitleBar`'s
 * `.body` has a definite height and `PaneSlot` claims `height: 100%` of it, so a ~31px
 * sibling pushed the terminal that far past the bottom of the frame. The terminal's own
 * background — white, on the default theme — then painted over the title bar of the row
 * below, which is exactly what was reported.
 *
 * Its replacement is `cide_app::lifecycle::restore_notice`: the same sentence, written into
 * the terminal as a dim line at the top of the transcript. It cannot overflow anything, it
 * scrolls away with the text it is about, it survives a re-dock like every other byte in the
 * pane, and it now says which of two things happened — because the visible screen *is*
 * retained across a restart.
 */
