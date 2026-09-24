/**
 * The UI kit page: every component in `components/`, live, in both themes and at any UI size.
 *
 * The chapter list below is the page's table of contents and the nav is drawn from it, so a new
 * chapter is one entry here plus its file under `page/chapters/`. `docs/ui-kit.md` is the prose
 * twin of this page and names the same chapters; `check:kit` holds the two together.
 *
 * Theme and size are written to `<html>` exactly as a cide window's are (`data-theme`,
 * `--ui-scale`), and into the URL, so a reload — or a link sent to someone — shows the same view.
 */
import { useState, type ReactElement } from 'react'

import { Icon } from '@/icons/Icon'
import { Select } from './components/Select'
import { Segmented } from './components/Choice'
import styles from './page/kit.module.css'
import { Foundations } from './page/chapters/Foundations'
import { Buttons } from './page/chapters/Buttons'
import { Fields } from './page/chapters/Fields'
import { Choices } from './page/chapters/Choices'
import { StatusMarks } from './page/chapters/StatusMarks'
import { FeedbackChapter } from './page/chapters/FeedbackChapter'
import { Structure } from './page/chapters/Structure'
import { Overlays } from './page/chapters/Overlays'
import { Patterns } from './page/chapters/Patterns'
import { ChromeChapter } from './page/chapters/ChromeChapter'
import { Inventory } from './page/chapters/Inventory'

/** The page's chapters, in order. `id` is the anchor, and the name `docs/ui-kit.md` uses. */
export const CHAPTERS = [
  { group: 'Foundations', id: 'colour', title: 'Colour' },
  { group: 'Foundations', id: 'type', title: 'Type' },
  { group: 'Foundations', id: 'space', title: 'Space, radius, elevation' },
  { group: 'Foundations', id: 'icons', title: 'Icons' },
  { group: 'Controls', id: 'buttons', title: 'Buttons' },
  { group: 'Controls', id: 'fields', title: 'Fields' },
  { group: 'Controls', id: 'choices', title: 'Choices' },
  { group: 'Display', id: 'status', title: 'Status marks' },
  { group: 'Display', id: 'feedback', title: 'Feedback' },
  { group: 'Structure', id: 'structure', title: 'Panels, cards, lists' },
  { group: 'Overlays', id: 'overlays', title: 'Dialogs, menus, pickers' },
  { group: 'Chrome', id: 'chrome', title: 'App chrome' },
  { group: 'Composed', id: 'patterns', title: 'Patterns' },
  { group: 'Audit', id: 'inventory', title: 'What the app has today' },
] as const

const SIZES = ['11', '12', '13', '14', '15', '16', '17'] as const

type Theme = 'light' | 'dark'

/* Both readers answer a default when there is no document: `check:kit` renders this page under
   node, where the first render must not throw. */
function readTheme(): Theme {
  if (typeof document === 'undefined') return 'light'
  return document.documentElement.dataset.theme === 'dark' ? 'dark' : 'light'
}

function readSize(): string {
  if (typeof document === 'undefined') return '13'
  const scale = parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--ui-scale'))
  return String(Math.round((Number.isFinite(scale) ? scale : 1) * 13))
}

/** Mirror a setting into the URL so a reload keeps it; `theme-boot.js` reads it back. */
function writeParam(key: string, value: string): void {
  const url = new URL(window.location.href)
  url.searchParams.set(key, value)
  window.history.replaceState(null, '', url)
}

export function Kit(): ReactElement {
  const [theme, setTheme] = useState<Theme>(readTheme)
  const [size, setSize] = useState<string>(readSize)

  const chooseTheme = (next: Theme): void => {
    document.documentElement.dataset.theme = next
    writeParam('theme', next)
    setTheme(next)
  }

  const chooseSize = (next: string): void => {
    document.documentElement.style.setProperty('--ui-scale', String(Number(next) / 13))
    writeParam('ui', next)
    setSize(next)
  }

  let lastGroup = ''
  return (
    <div className={styles.page}>
      <nav className={styles.nav} aria-label="Kit chapters">
        <div className={styles.brand}>
          <span className={styles.brandMark}>
            <Icon name="blocks" size={2} />
          </span>
          <span className={styles.brandText}>
            cide UI kit
            <span className={styles.brandSub}>reference, not the app</span>
          </span>
        </div>
        <ul className={styles.navList}>
          {CHAPTERS.map((c) => {
            const head = c.group !== lastGroup
            lastGroup = c.group
            return (
              <li key={c.id}>
                {head && <div className={styles.navGroup}>{c.group}</div>}
                <a className={styles.navLink} href={`#${c.id}`}>
                  {c.title}
                </a>
              </li>
            )
          })}
        </ul>
      </nav>
      <div className={styles.main}>
        <div className={styles.topbar}>
          <span className={styles.topbarTitle}>
            Etalon: the New Project wizard and the GitLab panel. Palette:{' '}
            <code>styles/tokens.css</code>, the same the app paints with.
          </span>
          <Segmented
            label="Theme"
            size="sm"
            value={theme}
            onChange={chooseTheme}
            options={[
              { value: 'light', label: 'Light' },
              { value: 'dark', label: 'Dark' },
            ]}
          />
          <div className={styles.sizePicker}>
            <Select
              size="sm"
              aria-label="UI font size"
              value={size}
              onChange={chooseSize}
              options={SIZES.map((s) => ({ value: s, label: `UI ${s}px` }))}
            />
          </div>
        </div>
        <div className={styles.scroll}>
          <div className={styles.content}>
            <div className={styles.intro}>
              <h1>How cide should look</h1>
              <p>
                Every control in cide, drawn from the two surfaces that got it right — the New
                Project wizard and the GitLab panel — and rebuilt on the design tokens. This page
                is the reference: a new feature takes its parts from here, and a part that is not
                here is added here first.
              </p>
              <p>
                Switch the theme and the UI size above: every specimen must stay right in both
                themes and at every size. The written rules are in <code>docs/ui-kit.md</code>.
              </p>
            </div>
            <Foundations />
            <Buttons />
            <Fields />
            <Choices />
            <StatusMarks />
            <FeedbackChapter />
            <Structure />
            <Overlays />
            <ChromeChapter />
            <Patterns />
            <Inventory />
          </div>
        </div>
      </div>
    </div>
  )
}
