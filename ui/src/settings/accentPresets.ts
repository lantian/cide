/**
 * The accent swatches Settings offers. Each was chosen to read as a distinct brand colour next to
 * the status palette, and each passes `cide_core::accent::fit`. Its test holds a copy of these
 * hexes (`every_preset_fits_and_the_default_is_kept_as_picked_on_light`), and `check:accent`
 * keeps the two lists equal. A preset that Rust refuses would be a button that shows an error.
 *
 * Import-free, so a check can compile it standalone.
 */

/** The shipped light-theme `--accent`, used to draw the `null` preset's swatch. */
export const DEFAULT_ACCENT = '#dc1f2b'

export const ACCENT_PRESETS: ReadonlyArray<{ value: string | null; label: string }> = [
  { value: null, label: 'Red (default)' },
  { value: '#2563eb', label: 'Blue' },
  { value: '#4f46e5', label: 'Indigo' },
  { value: '#7c3aed', label: 'Violet' },
  { value: '#db2777', label: 'Pink' },
  { value: '#ea580c', label: 'Orange' },
  { value: '#0d9488', label: 'Teal' },
  { value: '#16a34a', label: 'Green' },
]
