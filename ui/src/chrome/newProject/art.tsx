/**
 * The wizard's illustrations: small drawn mock-ups of the surfaces each road leads to. (M97)
 *
 * Inline SVG rather than images in `public/`, for two reasons. A picture is drawn in *this*
 * theme's colours — every fill is a class in `art.module.css` that reads a token, so a light
 * theme, a dark one and an imported scheme each get their own illustration with no second asset
 * to keep in step. And nothing here has to be fetched: `BASE_URL` paths are right in the packaged
 * app and in dev, but an image that failed to load would be an empty box in the one dialog whose
 * job is to make a first impression.
 *
 * They are decorative (`aria-hidden`): every fact a picture shows is also in the step's text.
 * No glyphs — `check:ui-icons` refuses symbol characters in source — so anything that reads as
 * a mark is drawn as a path.
 */
import type { ReactNode } from 'react'

import styles from './art.module.css'

function Svg({ children }: { children: ReactNode }) {
  return (
    <svg className={styles.art} viewBox="0 0 240 132" aria-hidden="true" focusable="false">
      {children}
    </svg>
  )
}

/** A tick drawn inside a circle centred at `(x, y)`. */
function Tick({ x, y, r = 6 }: { x: number; y: number; r?: number }) {
  const s = r / 6
  return (
    <g>
      <circle className={styles.green} cx={x} cy={y} r={r} />
      <path
        className={styles.tick}
        d={`M${x - 2.6 * s} ${y + 0.2 * s} l${1.9 * s} ${1.9 * s} l${3.4 * s} ${-3.8 * s}`}
      />
    </g>
  )
}

/** A folder, then the console it opens onto. */
export function EmptyArt() {
  return (
    <Svg>
      <g>
        <rect className={styles.accentFill} x="22" y="30" width="30" height="12" rx="3" />
        <rect className={styles.folder} x="22" y="36" width="78" height="60" rx="7" />
        <rect className={styles.folderLip} x="22" y="46" width="78" height="50" rx="7" />
      </g>
      <path className={styles.link} d="M104 66 H120" />
      <g>
        <rect className={styles.frame} x="120" y="20" width="102" height="92" rx="8" />
        <path className={styles.chrome} d="M120 28 a8 8 0 0 1 8 -8 h86 a8 8 0 0 1 8 8 v6 h-102 z" />
        <circle className={styles.red} cx="130" cy="27" r="2.4" />
        <circle className={styles.yellow} cx="138" cy="27" r="2.4" />
        <circle className={styles.green} cx="146" cy="27" r="2.4" />
        <path className={styles.prompt} d="M131 48 l5 4 l-5 4" />
        <rect className={styles.lineHi} x="141" y="50" width="54" height="4" rx="2" />
        <rect className={styles.line} x="131" y="64" width="72" height="4" rx="2" />
        <rect className={styles.line} x="131" y="74" width="48" height="4" rx="2" />
        <path className={styles.prompt} d="M131 88 l5 4 l-5 4" />
        <rect className={styles.accentFill} x="141" y="88" width="6" height="9" rx="1" />
      </g>
    </Svg>
  )
}

/** A proposal, the requirement delta it carries, and the approval. */
export function SpecArt() {
  return (
    <Svg>
      <g>
        <rect className={styles.sheetBack} x="30" y="14" width="92" height="104" rx="8" />
        <rect className={styles.frame} x="20" y="22" width="92" height="100" rx="8" />
        <rect className={styles.accentFill} x="32" y="34" width="38" height="6" rx="3" />
        <rect className={styles.lineHi} x="32" y="50" width="66" height="4" rx="2" />
        <rect className={styles.line} x="32" y="60" width="58" height="4" rx="2" />
        <rect className={styles.line} x="32" y="70" width="64" height="4" rx="2" />
        <rect className={styles.lineHi} x="32" y="86" width="44" height="4" rx="2" />
        <rect className={styles.line} x="32" y="96" width="60" height="4" rx="2" />
        <rect className={styles.line} x="32" y="106" width="36" height="4" rx="2" />
      </g>
      <path className={styles.link} d="M114 70 H130" />
      <g>
        <rect className={styles.frame} x="130" y="34" width="92" height="78" rx="8" />
        <rect className={styles.addBg} x="138" y="46" width="76" height="14" rx="3" />
        <path className={styles.plus} d="M144 53 h6 M147 50 v6" />
        <rect className={styles.lineHi} x="155" y="51" width="50" height="4" rx="2" />
        <rect className={styles.addBg} x="138" y="64" width="76" height="14" rx="3" />
        <path className={styles.plus} d="M144 71 h6 M147 68 v6" />
        <rect className={styles.lineHi} x="155" y="69" width="40" height="4" rx="2" />
        <rect className={styles.delBg} x="138" y="82" width="76" height="14" rx="3" />
        <path className={styles.minus} d="M144 89 h6" />
        <rect className={styles.line} x="155" y="87" width="46" height="4" rx="2" />
      </g>
      <Tick x={214} y={36} r={11} />
    </Svg>
  )
}

/** A board: to do, doing, done — each card wearing its role's colour. */
export function TasksArt() {
  const card = (x: number, y: number, dot: string, w: number, done = false) => (
    <g key={`${x}-${y}`}>
      <rect className={styles.card} x={x} y={y} width="60" height="20" rx="4" />
      {done ? (
        <Tick x={x + 9} y={y + 10} r={4} />
      ) : (
        <circle className={dot} cx={x + 9} cy={y + 10} r="3.5" />
      )}
      <rect className={styles.lineHi} x={x + 17} y={y + 8} width={w} height="4" rx="2" />
    </g>
  )
  return (
    <Svg>
      <rect className={styles.frame} x="10" y="12" width="220" height="110" rx="9" />
      {[18, 90, 162].map((x) => (
        <g key={x}>
          <rect className={styles.column} x={x} y="20" width="60" height="94" rx="6" />
          <rect className={styles.line} x={x + 6} y="27" width="26" height="4" rx="2" />
        </g>
      ))}
      {card(18, 38, styles.agentBlue ?? '', 34)}
      {card(18, 62, styles.agentPurple ?? '', 26)}
      {card(18, 86, styles.agentGreen ?? '', 30)}
      {card(90, 38, styles.agentGreen ?? '', 30)}
      {card(90, 62, styles.agentBlue ?? '', 22)}
      {card(162, 38, '', 32, true)}
      {card(162, 62, '', 24, true)}
    </Svg>
  )
}

/** The console, handing work to three roles, each on a branch of its own. */
export function AgentsArt() {
  const worker = (y: number, tone: string | undefined) => (
    <g key={y}>
      <rect className={styles.frame} x="158" y={y} width="66" height="28" rx="7" />
      <circle className={tone} cx="172" cy={y + 14} r="5" />
      <rect className={styles.lineHi} x="182" y={y + 9} width="30" height="4" rx="2" />
      <rect className={styles.line} x="182" y={y + 16} width="20" height="3" rx="1.5" />
    </g>
  )
  return (
    <Svg>
      <path className={styles.branchBlue} d="M86 66 C 120 66, 124 30, 158 30" />
      <path className={styles.branchGreen} d="M86 66 H158" />
      <path className={styles.branchPurple} d="M86 66 C 120 66, 124 102, 158 102" />
      <g>
        <rect className={styles.consoleGlow} x="12" y="42" width="76" height="48" rx="10" />
        <rect className={styles.console} x="16" y="46" width="68" height="40" rx="8" />
        <path className={styles.promptOn} d="M26 60 l5 4 l-5 4" />
        <rect className={styles.onAccent} x="36" y="62" width="36" height="4" rx="2" />
        <rect className={styles.onAccentDim} x="26" y="74" width="46" height="3" rx="1.5" />
      </g>
      {worker(16, styles.agentBlue)}
      {worker(52, styles.agentGreen)}
      {worker(88, styles.agentPurple)}
    </Svg>
  )
}

/** Milestones on a track: two met, one being worked on, one ahead. */
export function MilestonesArt() {
  const xs = [36, 92, 148, 204]
  return (
    <Svg>
      <rect className={styles.track} x="36" y="64" width="168" height="4" rx="2" />
      <rect className={styles.trackDone} x="36" y="64" width="112" height="4" rx="2" />
      <Tick x={xs[0] ?? 0} y={66} r={9} />
      <Tick x={xs[1] ?? 0} y={66} r={9} />
      <circle className={styles.halo} cx={xs[2]} cy="66" r="16" />
      <circle className={styles.current} cx={xs[2]} cy="66" r="9" />
      <circle className={styles.accentFill} cx={xs[2]} cy="66" r="4" />
      <circle className={styles.ahead} cx={xs[3]} cy="66" r="9" />
      <path className={styles.flagPole} d="M148 44 V26" />
      <path className={styles.accentFill} d="M148 26 h18 l-5 5 l5 5 h-18 z" />
      {xs.map((x, i) => (
        <g key={x}>
          <rect className={i < 3 ? styles.lineHi : styles.line} x={x - 20} y="88" width="40" height="4" rx="2" />
          <rect className={styles.line} x={x - 14} y="98" width="28" height="3" rx="1.5" />
        </g>
      ))}
      <g>
        <rect className={styles.gate} x="14" y="18" width="62" height="18" rx="9" />
        <path className={styles.gateMark} d="M24 27 l3 3 l5 -6" />
        <rect className={styles.lineHi} x="38" y="25" width="30" height="4" rx="2" />
      </g>
    </Svg>
  )
}
