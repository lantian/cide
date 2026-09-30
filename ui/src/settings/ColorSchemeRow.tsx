/**
 * Settings › Appearance › Colour scheme. (M24)
 *
 * # Why it is a component of its own rather than a `Row` inside `Appearance`
 *
 * It is the only control on that screen whose options do not come from `Settings`. The list is
 * `boot.schemes` — imported files, broadcast to every window by `cide://schemes-changed` — so
 * this reads the store directly, the way `KeymapSection` does and for the same reason: threading
 * a fourth source through `SectionProps` would put a store subscription in front of every other
 * section that has no use for one.
 *
 * The selector returns `boot?.schemes`, which is the array Rust sent. Not a `.map`, not a
 * `.filter`: `check:selectors` exists because a selector that builds a fresh array re-renders
 * for ever. The filtering happens in `schemeChoices`, below the subscription.
 *
 * # One row, and the setting behind it is two fields
 *
 * `EditorSettings` stores `colorSchemeLight` and `colorSchemeDark`. This row edits whichever one
 * the current theme names, and says so — a scheme paints the buffer's background, so a dark
 * scheme under a light window is an inverted rectangle in a white app, and the pair of fields is
 * what makes that state unreachable rather than merely discouraged.
 */
import { useState } from 'react'

import { settings as settingsApi } from '@/ipc/client'
import { errorText } from '@/ipc/errorText'
import { useWorkspace, type Theme } from '@/store/workspace'
import { BUILTIN_SCHEME, schemeChoices } from '@/editor/scheme'
import type { ColorScheme, EditorSettings, SettingsPatch } from '@/ipc/generated'
import { InfoPara } from '@/kit/components/InfoTip'
import { ActionButton, Row, Select, resetTo, useSettingsDefaults } from './controls'
import styles from './panels.module.css'

/**
 * What happened to the last import, in a sentence — or `null` when there is nothing to say.
 *
 * Its own function because there are three outcomes and inlining a nested ternary into the row's
 * JSX is how one of them stops being reachable without anybody noticing. They are: everything
 * matched (say nothing — the picker already moved, which is the feedback), some matched (name
 * what went to the other theme), nothing matched (say so, or the import reads as a failure).
 *
 * A sentence for the row's `status` since M133, rather than a `Note` under the row: it is about
 * this row's picker, and a block below it read as being about the row after.
 */
function importedSentence(imported: readonly ColorScheme[], theme: Theme): string | null {
  const other = imported.filter((s) => s.polarity !== theme)
  if (other.length === 0) return null
  const otherName = theme === 'dark' ? 'Light' : 'Dark'
  const names = other.map((s) => s.name).join(', ')
  const matched = imported.length - other.length
  const what = `${names} ${other.length === 1 ? 'is a' : 'are'} ${otherName.toLowerCase()} scheme${other.length === 1 ? '' : 's'}`
  return matched > 0
    ? `Also imported: ${what}, offered here once you switch to ${otherName}.`
    : `Imported into ${otherName}: ${what} — nothing changed in this window.`
}

export interface ColorSchemeRowProps {
  theme: Theme
  editor: EditorSettings
  patch: (patch: SettingsPatch) => void
}

export function ColorSchemeRow({ theme, editor, patch }: ColorSchemeRowProps) {
  const schemes = useWorkspace((s) => s.boot?.schemes)
  /**
   * What the last import produced, so the row can say what happened to it.
   *
   * A `.vsix` normally carries a light/dark pair, and only one of them can be selected here —
   * the setting is keyed by polarity. Both land on disk, so the message has to account for the
   * one that did not appear: without it, importing *Min Dark + Min Light* while on Light selects
   * Min Light and silently drops Min Dark from the story, and switching theme later produces a
   * scheme the user does not remember importing.
   *
   * The case that most needs a line is an import where **nothing** matches: it worked, it is on
   * disk, it is in the other theme's list, and this window looks exactly as it did. Without a
   * message the honest reading is "the import failed".
   */
  const [imported, setImported] = useState<readonly ColorScheme[] | null>(null)
  const [failed, setFailed] = useState<string | null>(null)

  const selected = theme === 'dark' ? editor.colorSchemeDark : editor.colorSchemeLight
  const choices = schemeChoices(schemes ?? [], theme)
  // An id naming a scheme that is no longer on disk. The buffer already falls back to the
  // builtin (`schemeToApply`); the picker has to agree with what is on screen rather than show
  // a selection nothing is honouring.
  const value = choices.some((c) => c.value === selected) ? selected : BUILTIN_SCHEME

  const select = (next: string) => {
    setImported(null)
    setFailed(null)
    patch({
      editor: {
        ...editor,
        ...(theme === 'dark' ? { colorSchemeDark: next } : { colorSchemeLight: next }),
      },
    })
  }

  const doImport = () => {
    setImported(null)
    setFailed(null)
    void settingsApi
      .importScheme()
      .then((schemes) => {
        // An empty list is a cancelled picker, which is not an outcome to report.
        if (schemes.length === 0) return
        const usable = schemes.filter((s) => s.polarity === theme)
        // `select` clears the note, so it runs first and the note is set after.
        if (usable[0]) select(usable[0].id)
        setImported(schemes)
      })
      .catch((e: unknown) => setFailed(errorText(e)))
  }

  const remove = () => {
    if (value === BUILTIN_SCHEME) return
    setImported(null)
    setFailed(null)
    // Selected back to the builtin *before* the removal, so there is no frame in which the
    // setting names a file that is gone. `scheme_remove` deliberately leaves the setting alone
    // — see its own note — which makes this the caller's job.
    select(BUILTIN_SCHEME)
    void settingsApi.removeScheme(value).catch((e: unknown) => setFailed(errorText(e)))
  }

  const sentence = imported === null ? null : importedSentence(imported, theme)
  const def = useSettingsDefaults()?.editor
  const defaultId = theme === 'dark' ? def?.colorSchemeDark : def?.colorSchemeLight

  return (
    <Row
      label="Colour scheme"
      hint={`How the editor paints code, for ${theme === 'dark' ? 'Dark' : 'Light'}; the other theme keeps its own.`}
      info={
        <>
          <InfoPara>
            What the editor paints code — and the buffer’s own background. Per theme: the other
            theme keeps its own scheme.
          </InfoPara>
          <InfoPara>
            Import a VS Code theme’s <code>.vsix</code> straight from the marketplace, or a bare{' '}
            <code>-color-theme.json</code>; cide converts its TextMate scopes to its own token
            roles. A <code>.vsix</code> usually holds a light and a dark variant and both are
            imported at once.
          </InfoPara>
        </>
      }
      status={
        failed !== null
          ? { tone: 'warn', text: `Could not import that file: ${failed}` }
          : sentence !== null
            ? { tone: 'info', text: sentence }
            : undefined
      }
      {...resetTo(value, defaultId, select)}
      control={
        <div className={styles.schemeControl}>
          <Select label="Colour scheme" value={value} options={choices} onChange={select} />
          <ActionButton label="Import…" onClick={doImport} />
          <ActionButton
            label="Remove"
            onClick={remove}
            disabled={value === BUILTIN_SCHEME}
          />
        </div>
      }
    />
  )
}
