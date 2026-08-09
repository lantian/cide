/**
 * Text the overlays and the sidebar render verbatim from the mock.
 *
 * Pure and import-free, so `ui/scripts/check-picker.mjs` can compile it standalone — the
 * counter is a string the mock states exactly (`6 of 2,418`) and getting the grouping wrong
 * is the kind of thing that reads fine in review and is wrong on screen.
 */

/**
 * Group a non-negative integer with commas: `2418` → `2,418`.
 *
 * Hand-rolled rather than `toLocaleString`, which is locale-sensitive: under a `de-DE`
 * system locale it produces `2.418`, and under `en-IN` it produces `2,418` for this number
 * but `2,41,801` for the next one up. The mock states a comma; a desktop app whose counter
 * changes shape with `LC_ALL` is not transcribing the mock, it is guessing.
 */
export function groupDigits(value: number): string {
  const whole = Math.trunc(Math.abs(value))
  const sign = value < 0 ? '-' : ''
  const digits = String(whole)

  let out = ''
  for (let i = 0; i < digits.length; i++) {
    if (i > 0 && (digits.length - i) % 3 === 0) out += ','
    out += digits[i]
  }
  return sign + out
}

/** The picker's right-aligned counter: `6 of 2,418`. */
export function matchCounter(shown: number, total: number): string {
  return `${groupDigits(shown)} of ${groupDigits(total)}`
}

/** The name after the last path separator. */
export function basename(path: string): string {
  const cut = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'))
  return path.slice(cut + 1)
}

/** Everything before the last separator, without a trailing one. `''` for a bare name. */
export function dirname(path: string): string {
  const cut = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'))
  return cut <= 0 ? '' : path.slice(0, cut)
}

/** Colour roles a kind badge can take. Each maps to one token; see `Overlay.module.css`. */
export type BadgeTone =
  | 'accent'
  | 'blue'
  | 'cyan'
  | 'yellow'
  | 'green'
  | 'purple'
  | 'dim'
  | 'faint'

export interface KindBadge {
  label: string
  tone: BadgeTone
}

/**
 * The mock's language badge table, keyed by extension.
 *
 * Duplicated from `chrome/TabStrip.tsx` rather than imported. Not because duplication is
 * good: that copy returns CSS Module class names, and class names are scoped to the module
 * that declares them, so importing it would hand this component a `TabStrip.module.css`
 * class that does not exist in `Overlay.module.css`. Returning a *tone name* instead is what
 * makes this one shareable, and if the tab strip is ever refactored the two should meet
 * here. The table itself is the mock's and is stated in the plan, so both copies have the
 * same source of truth to be checked against.
 */
export function kindBadge(path: string): KindBadge {
  const name = basename(path)
  const dot = name.lastIndexOf('.')
  // `> 0` rather than `>= 0`: a leading dot makes a hidden file, not an extension, so
  // `.gitignore` falls through to the neutral marker instead of claiming a GITIGNORE badge.
  const ext = dot > 0 ? name.slice(dot + 1).toLowerCase() : ''

  switch (ext) {
    case 'rs':
      return { label: 'RS', tone: 'accent' }
    case 'ts':
      return { label: 'TS', tone: 'blue' }
    case 'tsx':
      return { label: 'TSX', tone: 'cyan' }
    case 'js':
      return { label: 'JS', tone: 'yellow' }
    case 'toml':
      return { label: 'TOML', tone: 'green' }
    case 'lock':
      return { label: 'LOCK', tone: 'faint' }
    case 'md':
      return { label: 'MD', tone: 'dim' }
    case 'yml':
    case 'yaml':
      return { label: 'YML', tone: 'purple' }
    case 'json':
      return { label: 'JSON', tone: 'yellow' }
    case 'sh':
      return { label: 'SH', tone: 'green' }
    case 'css':
      return { label: 'CSS', tone: 'purple' }
    case 'html':
      return { label: 'HTML', tone: 'accent' }
    default:
      return { label: '·', tone: 'faint' }
  }
}
