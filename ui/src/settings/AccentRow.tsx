/**
 * Settings → Appearance → Accent colour: the one brand colour every accent token is drawn in.
 *
 * The swatches are the kit's `ColorSwatches`. The first preset is the shipped red, and it is
 * `null` rather than `#dc1f2b`: choosing it sends `reset`, which puts the stored accent back to
 * `None` and so gives back `tokens.css`'s hand-picked red exactly. A picked `#dc1f2b` would be the
 * red run through the derivation, which is close to the original but not the same.
 *
 * Validation and fitting are Rust's (`cide_core::accent`). This row sends the colour the user
 * picked. If it is refused (black is reserved for the danger fill, and greys and near-whites have
 * no hue to be an accent) it shows the sentence Rust wrote. A colour that is accepted but
 * adjusted for contrast is simply shown: the picker keeps the colour the user picked, and the
 * app draws its legible variants.
 */
import { useState } from 'react'

import { ColorSwatches } from '@/kit/components/Choice'
import { settings as settingsApi } from '@/ipc/client'
import { errorText } from '@/ipc/errorText'
import type { Accent } from '@/ipc/generated'
import { ACCENT_PRESETS, DEFAULT_ACCENT } from './accentPresets'
import { Row } from './controls'

export function AccentRow({ accent }: { accent: Accent | null }) {
  const [failed, setFailed] = useState<string | null>(null)

  const choose = (base: string | null): void => {
    setFailed(null)
    // Not `useSettingsActions.patch`, which swallows the rejection: a refused colour is the one
    // outcome here the user has to be told about, and it never shows up in a snapshot.
    void settingsApi
      .set({ accent: base === null ? 'reset' : { set: { base } } })
      .catch((e: unknown) => setFailed(errorText(e)))
  }

  return (
    <Row
      label="Accent colour"
      hint="Buttons, focus rings, the current tab, selections and the waiting counters. It is adjusted so it stays readable in both themes. Black is reserved for destructive actions, so black and greys cannot be chosen."
      control={
        <ColorSwatches
          label="Accent colour"
          value={accent?.base ?? null}
          onChange={choose}
          presets={ACCENT_PRESETS}
          defaultColor={DEFAULT_ACCENT}
          error={failed}
        />
      }
    />
  )
}
