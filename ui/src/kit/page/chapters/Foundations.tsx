/**
 * Foundations: the tokens every component is built from. Values are read live from `tokens.css`
 * — nothing on this page restates a hex, so the page cannot drift from the stylesheet.
 */
import type { CSSProperties, ReactElement } from 'react'

import { Icon } from '@/icons/Icon'
import { ICON_PATHS, type IconName } from '@/icons/iconPaths'
import { Chapter, Specimen } from '../Specimen'
import { rootValue, useRootTick } from '../useRootTick'
import styles from '../kit.module.css'

type Swatch = { token: string; use: string }

const SURFACES: Swatch[] = [
  { token: '--bg', use: 'The window; the inside of a text field.' },
  { token: '--chrome', use: 'Header, tab strip, dialogs, pickers.' },
  { token: '--panel', use: 'Sidebars, cards, menus.' },
  { token: '--panel-2', use: 'Recessed strips in a card; row hover.' },
  { token: '--chrome-hi', use: 'Pressed controls, tracks, counters.' },
  { token: '--sel', use: 'Selected row, focused menu item.' },
]

const LINES: Swatch[] = [
  { token: '--border', use: 'Edges of cards, fields, panels.' },
  { token: '--border-soft', use: 'Dividers between rows inside a card.' },
]

const TEXT: Swatch[] = [
  { token: '--text-hi', use: 'Titles, typed values, the current thing.' },
  { token: '--text', use: 'Body text, labels.' },
  { token: '--dim', use: 'Secondary text, icons at rest.' },
  { token: '--faint', use: 'Placeholders, hints, captions, disabled.' },
]

const ACCENT: Swatch[] = [
  { token: '--accent', use: 'Links, focus, the current step, "yours".' },
  { token: '--accent-hi', use: 'Accent text on a raised segment.' },
  { token: '--accent-dim', use: 'Hover borders, done-step threads.' },
  { token: '--on-accent', use: 'Text and icons on an accent fill.' },
  { token: '--on-fill', use: 'Text and icons on any --grad-<tone> fill.' },
]

const GRADIENTS: Swatch[] = [
  { token: '--grad-accent-ink', use: 'Anything carrying text: primary button, current step dot, switch on.' },
  { token: '--grad-accent', use: 'Decorative only — never under text (2.4:1).' },
  { token: '--grad-accent-h', use: 'Horizontal bars: progress, the current tab underline.' },
  { token: '--grad-accent-soft', use: 'Art grounds: the wizard rail, an empty state mark.' },
  { token: '--grad-green', use: 'Filled marks: passed, done, open.' },
  { token: '--grad-yellow', use: 'Filled marks: pending, waiting.' },
  { token: '--grad-red', use: 'Filled marks: failed, critical.' },
  { token: '--grad-purple', use: 'Filled marks: merged.' },
  { token: '--grad-blue', use: 'Filled marks: informational.' },
  { token: '--grad-neutral', use: 'Filled marks: no state worth a colour; counters.' },
]

const STATUS: Swatch[] = [
  { token: '--green', use: 'Passed, done, open MR, additions.' },
  { token: '--yellow', use: 'Waiting on someone: pending, needs review.' },
  { token: '--red', use: 'Failed, blocked, deletions, destructive.' },
  { token: '--purple', use: 'Merged.' },
  { token: '--blue', use: 'Informational references.' },
  { token: '--cyan', use: 'Reserved: terminal and syntax only.' },
]

function Swatches({ items, gradient = false }: { items: Swatch[]; gradient?: boolean }): ReactElement {
  useRootTick()
  return (
    <div className={styles.swatches}>
      {items.map((s) => (
        <div key={s.token} className={styles.swatch}>
          <div
            className={styles.swatchChip}
            style={{ background: `var(${s.token})` } as CSSProperties}
          />
          <div className={styles.swatchText}>
            <span className={styles.swatchName}>{s.token}</span>
            {!gradient && <span className={styles.swatchValue}>{rootValue(s.token)}</span>}
            <span className={styles.swatchUse}>{s.use}</span>
          </div>
        </div>
      ))}
    </div>
  )
}

const TYPE = [
  { token: '--fs-ui-20', weight: 600, use: 'Page and kit chapter titles only.' },
  { token: '--fs-ui-16', weight: 600, use: 'Dialog and step titles.' },
  { token: '--fs-ui-14', weight: 600, use: 'Card titles, the MR title in its own view, brand.' },
  { token: '--fs-ui-13', weight: 400, use: 'Body text, a dialog lead, list titles, md field values.' },
  { token: '--fs-ui-12', weight: 400, use: 'Labels, buttons, rows, menus — the workhorse.' },
  { token: '--fs-ui-11', weight: 400, use: 'Hints, captions, badges, meta lines, sm buttons.' },
] as const

const SPACE = ['--sp-1', '--sp-2', '--sp-3', '--sp-4', '--sp-5', '--sp-6', '--sp-7'] as const

const RADII = [
  { token: '--r-1', use: 'Tag, key, inline code, icon button' },
  { token: '--r-2', use: 'Buttons, fields, notes' },
  { token: '--r-3', use: 'Cards, menus, summaries' },
  { token: '--r-4', use: 'Dialogs, pickers' },
  { token: '--r-full', use: 'Badges, dots, switch, avatar' },
] as const

const SHADOWS = [
  { token: '--shadow-1', use: 'Resting primary button, raised segment' },
  { token: '--shadow-2', use: 'Hover lift, menus, tooltips' },
  { token: '--shadow-3', use: 'Dialogs, pickers, toasts' },
] as const

export function Foundations(): ReactElement {
  useRootTick()
  const icons = Object.keys(ICON_PATHS) as IconName[]
  return (
    <>
      <Chapter
        id="colour"
        title="Colour"
        lead={
          <>
            White and near-white grounds, graphite text, one bright red accent — the same
            palette the app paints with. Every colour is a
            token from <code>tokens.css</code> and means the same thing on every surface; a colour
            a component needs that is not here is a token to add, never a literal.
          </>
        }
      >
        <Specimen
          name="Surfaces"
          source="tokens.css"
          use="Step down one surface at a time; a card on a panel is told apart by its border, not a second fill."
        >
          <Swatches items={SURFACES} />
        </Specimen>
        <Specimen name="Lines and text" source="tokens.css" use="Four text levels, and never a fifth made from opacity.">
          <Swatches items={[...LINES, ...TEXT]} />
        </Specimen>
        <Specimen
          name="Accent"
          source="tokens.css"
          use={
            <>
              <strong>One accent, used sparingly:</strong> it marks the current thing and the one
              act a surface exists for. A screen where half the controls are orange has no current
              thing.
            </>
          }
        >
          <Swatches items={ACCENT} />
          <Swatches items={GRADIENTS} gradient />
        </Specimen>
        <Specimen
          name="Status"
          source="tokens.css"
          use="Each hue means one state everywhere (see Status marks). Used as text, an icon, a wash, or a small filled mark's gradient (--grad-*) — never as a large fill."
        >
          <Swatches items={STATUS} />
        </Specimen>
      </Chapter>

      <Chapter
        id="type"
        title="Type"
        lead={
          <>
            Inter for the interface, JetBrains Mono for anything a user might paste into a
            terminal (paths, branches, ids, shortcuts). Six sizes on the <code>--fs-ui-*</code>{' '}
            ladder, all of which follow the UI font-size setting. Weight is 400 or 600 — 500 only
            for button labels and badges.
          </>
        }
      >
        <Specimen name="The ladder" source="tokens.css" use="A size that is not a rung is a size that does not scale.">
          <div className={styles.stack} style={{ width: '100%' }}>
            {TYPE.map((t) => (
              <div key={t.token} className={styles.typeRow}>
                <span className={styles.cellLabel}>
                  {t.token} · {rootValue(t.token).replace(/^calc\((.*) \* var\(--ui-scale\)\)$/, '$1')}
                </span>
                <span style={{ fontSize: `var(${t.token})`, fontWeight: t.weight, color: 'var(--text-hi)' }}>
                  {t.use}
                </span>
              </div>
            ))}
            <div className={styles.typeRow}>
              <span className={styles.cellLabel}>--font-mono</span>
              <span style={{ fontFamily: 'var(--font-mono)', fontSize: 'var(--fs-ui-12)' }}>
                ~/work/cide · feature/ui-kit · a1b2c3d · Ctrl+Shift+P
              </span>
            </div>
          </div>
        </Specimen>
      </Chapter>

      <Chapter
        id="space"
        title="Space, radius, elevation"
        lead="A 2 · 4 · 6 · 8 · 12 · 16 · 24 spacing step, five radii by object size, three shadows by height above the page, two durations."
      >
        <Specimen
          name="Spacing"
          source="tokens.css"
          use="Gaps between siblings, padding inside boxes. 12 (--sp-5) inside cards and rows, 16 (--sp-6) around sections, 24 (--sp-7) between groups."
        >
          <div className={styles.stack} style={{ width: '100%' }}>
            {SPACE.map((t) => (
              <div key={t} className={styles.typeRow}>
                <span className={styles.cellLabel}>
                  {t} · {rootValue(t)}
                </span>
                <span className={styles.scaleBar} style={{ width: `calc(var(${t}) * 4)` }} />
              </div>
            ))}
          </div>
        </Specimen>
        <Specimen name="Radius" source="tokens.css" use="Bigger objects, rounder corners — a dialog is never squarer than the button in it.">
          {RADII.map((r) => (
            <div key={r.token} className={styles.cell}>
              <span className={styles.radiusBox} style={{ borderRadius: `var(${r.token})` }} />
              <span className={styles.cellLabel}>{r.token}</span>
              <span className={styles.swatchUse}>{r.use}</span>
            </div>
          ))}
        </Specimen>
        <Specimen
          name="Elevation and motion"
          source="tokens.css"
          use={
            <>
              Shadows say how far above the page a thing floats. Motion is 90ms (
              <code>--dur-1</code>, hover and press) or 140ms (<code>--dur-2</code>, a step
              arriving), on <code>--ease-out</code>, and only ever on paint properties — see{' '}
              <code>check:motion</code>.
            </>
          }
        >
          {SHADOWS.map((s) => (
            <div key={s.token} className={styles.cell}>
              <span className={styles.shadowBox} style={{ boxShadow: `var(${s.token})` }} />
              <span className={styles.cellLabel}>{s.token}</span>
              <span className={styles.swatchUse}>{s.use}</span>
            </div>
          ))}
        </Specimen>
      </Chapter>

      <Chapter
        id="icons"
        title="Icons"
        lead={
          <>
            One set of 24×24 stroke marks (<code>icons/Icon.tsx</code>), coloured by{' '}
            <code>currentColor</code>, in four sizes: 12 · 14 · 16 · 20. 14 in rows and small
            buttons, 16 in toolbars and md buttons, 20 in empty states. Never a Unicode glyph for
            an icon.
          </>
        }
      >
        <Specimen
          name={`The set (${icons.length})`}
          source="icons/iconPaths.ts"
          use="A mark that is not here is vendored with scripts/vendor-ui-icons.mjs, not drawn inline."
        >
          <div className={styles.iconGrid}>
            {icons.map((name) => (
              <div key={name} className={styles.iconCell} title={name}>
                <Icon name={name} size={3} />
                <span className={styles.iconName}>{name}</span>
              </div>
            ))}
          </div>
        </Specimen>
      </Chapter>
    </>
  )
}
