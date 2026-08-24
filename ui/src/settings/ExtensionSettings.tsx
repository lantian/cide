/**
 * Settings the installed extensions declare. (M22)
 *
 * # Why extensions have settings at all
 *
 * Because the alternative is each one inventing its own. An extension that needed a number would
 * otherwise put a field in its own panel, read a file of its own, or ask the user to edit JSON —
 * three surfaces cide cannot theme, cannot validate and cannot put in the one place a person looks
 * for *configure the thing*.
 *
 * # Why this section draws no controls of its own
 *
 * Every row here is `controls.tsx`'s: `ToggleRow`, `NumberField`, `Segmented`, and one text input.
 * That is the same bargain a contributed *panel* makes by being a view model — a third party names
 * a member of a closed set and cide decides what it draws — and it buys the same three things: the
 * theme is right in both palettes, `check:ui-scale` covers it, and an extension cannot ship CSS.
 *
 * The cost is stated rather than hidden: an extension cannot have a setting cide has no control
 * for. `SettingKind` is four members, and the answer to a fifth is to add it there, where it gets
 * a control, a coercion and a check.
 *
 * # Nothing here rides `SettingsPatch`
 *
 * These values live in `extensions.json`, not `workspace.json`, and go back through
 * `ext.setSetting`. So this component takes no `SectionProps` — the same shape `AgentsSection` has
 * for the same reason, and `sections.tsx` says so where it renders both.
 */
import { NumberField, Note, Row, Segmented, ToggleRow } from './controls'
import styles from './controls.module.css'
import { notifyFailure } from '@/chrome/notices'
import { ext as extApi } from '@/ipc/client'
import type { ExtensionRef, InstalledExtension, SettingDef } from '@/ipc/client'
import { useExtStore } from '@/ext/extStore'

/**
 * The store-reading half. `sections.tsx` renders this one.
 *
 * Split from the view below on `AgentsPanelHost`'s rule, and here it was forced rather than
 * chosen: `ui/scripts/check-ext-render.mjs` renders this section under node, and zustand's
 * `useSyncExternalStore` reads `getInitialState` on a server pass — so a component that took its
 * rows from the store rendered the *empty* screen in the check, whatever the fixture had put in
 * the store. A pure view with props is the only shape that gate can see at all.
 */
export function ExtensionSettings(): React.JSX.Element {
  const extensions = useExtStore((state) => state.snapshot.extensions)
  const busy = useExtStore((state) => state.busy)
  const run = useExtStore((state) => state.run)

  return (
    <ExtensionSettingsView
      extensions={extensions}
      busy={busy}
      onChange={(id, key, value) => {
        // Straight through `run`, so the page shows the same `busy` the Extensions panel does and
        // the answer is the whole snapshot — which is what re-renders these controls with the
        // value Rust actually stored, rather than the one this component sent. That difference is
        // not hypothetical: a number is clamped on the way in.
        void run(() => extApi.setSetting(id, key, value)).catch(notifyFailure)
      }}
    />
  )
}

export interface ExtensionSettingsViewProps {
  readonly extensions: readonly InstalledExtension[]
  /** Whether a call is in flight. Disables the controls that would race it. */
  readonly busy?: boolean
  /** Absent means the rows are read-only, which is what the render check drives. */
  readonly onChange?:
    | ((id: ExtensionRef, key: string, value: boolean | string | number) => void)
    | undefined
}

/** Pure: reads no store, calls no IPC, never reads the clock. */
export function ExtensionSettingsView({
  extensions,
  busy = false,
  onChange,
}: ExtensionSettingsViewProps): React.JSX.Element {
  /*
   * Only enabled extensions, and only those that declare something.
   *
   * A disabled extension's settings are *kept* — `extensions.json` holds them and they come back
   * when it is switched on — but showing them would be a page of controls that change the
   * behaviour of nothing that is running.
   */
  const rows = extensions.filter(
    (extension) => extension.enabled && extension.contributes.settings.length > 0,
  )

  if (extensions.length === 0) {
    return (
      <Note title="No extensions are installed">
        Extensions are installed from a marketplace, in the Extensions panel on the activity rail.
        Anything you install that has settings will add them here.
      </Note>
    )
  }
  if (rows.length === 0) {
    // Distinct from the sentence above, and the distinction is the point: "you have none" and
    // "yours have none to configure" are two different facts and only one of them suggests
    // installing something.
    return (
      <Note title="Nothing to configure">
        {extensions.length === 1 ? 'The installed extension declares' : 'The installed extensions declare'}
        {' '}no settings. An extension adds rows here by listing them under `contributes.settings`
        in its manifest.
      </Note>
    )
  }

  return (
    <>
      {rows.map((extension) => (
        <section key={`${extension.marketplace}.${extension.extension}`} className={styles.group}>
          <h3 className={styles.groupTitle}>{extension.name}</h3>
          {extension.contributes.settings.map((setting) => (
            <SettingRow
              key={setting.id}
              setting={setting}
              value={extension.settings[setting.id]}
              // Disabled while a call is in flight, and also when nothing can act on a change —
              // a control that moves and then springs back is worse than one that does not move.
              busy={busy || onChange === undefined}
              onChange={(next) => onChange?.(extension, setting.id, next)}
            />
          ))}
        </section>
      ))}
    </>
  )
}

function SettingRow({
  setting,
  value,
  busy,
  onChange,
}: {
  setting: SettingDef
  /** The resolved value — defaults already merged in by Rust, so this is never absent in practice. */
  value: InstalledExtension['settings'][string] | undefined
  busy: boolean
  onChange: (next: boolean | string | number) => void
}): React.JSX.Element {
  const kind = setting.kind
  const hint = setting.description

  switch (kind.type) {
    case 'toggle':
      return (
        <ToggleRow
          label={setting.label}
          hint={hint}
          // `?? kind.default` guards one real case: an extension updated to add a setting between
          // Rust resolving the snapshot and this render. Rust fills every declared key, so it is a
          // frame at most — but a `checked={undefined}` is an uncontrolled input, and React
          // converting one to controlled logs a warning and loses the first click.
          checked={typeof value === 'boolean' ? value : kind.default}
          onChange={onChange}
          disabled={busy}
        />
      )

    case 'number':
      return (
        <Row
          label={setting.label}
          hint={hint}
          control={
            <NumberField
              label={setting.label}
              value={typeof value === 'number' ? value : kind.default}
              // The declared band, so the spinner and the arrows stop where Rust would clamp.
              // `Number.MIN_SAFE_INTEGER` rather than 0 for an unbounded setting: a `min` of 0
              // would be cide inventing a constraint the manifest did not ask for.
              min={kind.min ?? Number.MIN_SAFE_INTEGER}
              max={kind.max ?? Number.MAX_SAFE_INTEGER}
              onChange={onChange}
            />
          }
        />
      )

    case 'choice':
      return (
        <Row
          label={setting.label}
          hint={hint}
          control={
            <Segmented
              label={setting.label}
              value={typeof value === 'string' ? value : kind.default}
              // The label falls back to the value, which is what a manifest that gave none meant.
              options={kind.choices.map((choice) => ({
                value: choice.value,
                label: choice.label ?? choice.value,
              }))}
              onChange={onChange}
            />
          }
        />
      )

    case 'text':
      return (
        <Row
          label={setting.label}
          hint={hint}
          control={
            <input
              className={styles.text}
              type="text"
              spellCheck={false}
              aria-label={setting.label}
              disabled={busy}
              placeholder={kind.placeholder ?? ''}
              value={typeof value === 'string' ? value : kind.default}
              // On change and not on blur, unlike `NumberField`. A number has to be committed late
              // because a half-typed one is a different number — `16` passes through `1` — and a
              // half-typed string is a prefix of the string, which is harmless. `NumberField`'s
              // own header sets that out at length.
              onChange={(event) => onChange(event.target.value)}
            />
          }
        />
      )
  }
}
