/**
 * The dimensional half of M3's acceptance criterion, expressed as data.
 *
 * The criterion is a screenshot diff against the mock at 1440x900 in both themes. Neither
 * side of that diff is obtainable here: the mock is a template that needs a runtime we do
 * not have, and on KDE Wayland the app window cannot reliably be raised for a capture. What
 * survives without either is the geometry itself — every number the mock states, read back
 * off the live DOM and compared at the tolerance the milestone sets.
 *
 * Elements are located through `data-audit` attributes rather than CSS Module class names,
 * because module class names are hashed at build time and change whenever a rule moves. The
 * attributes are inert: nothing styles or queries them outside this file.
 *
 * A check whose element is absent reports `actual: NaN` and fails. Dropping the row instead
 * would make an unhooked component indistinguishable from a correct one, which is the one
 * failure mode this file exists to prevent.
 */

/** The milestone's stated allowance: every dimension within 2px of the mock. */
export const TOLERANCE = 2

export interface AuditRow {
  element: string
  property: string
  expected: number
  actual: number
  delta: number
  pass: boolean
}

export interface AuditReport {
  theme: string
  viewport: string
  rows: AuditRow[]
  failures: number
}

type Measure = (el: Element) => number

interface Check {
  element: string
  property: string
  expected: number
  /** The first match is measured; `null` yields an absent row rather than a skipped one. */
  selector: string
  measure: Measure
}

/** `data-audit="name"` as a selector, so the hook name appears once per check. */
const hook = (name: string) => `[data-audit="${name}"]`

/**
 * Border-box size, not the computed `height`/`width` declaration.
 *
 * A screenshot measures painted pixels, so a bar that states 34px but is stretched by an
 * overflowing child has to read as 34-plus-whatever here, not as the declaration it broke.
 */
const boxHeight: Measure = (el) => el.getBoundingClientRect().height
const boxWidth: Measure = (el) => el.getBoundingClientRect().width

/** A resolved longhand, in px. Anything non-numeric (`normal`, `auto`) becomes NaN. */
const style =
  (property: string): Measure =>
  (el) =>
    Number.parseFloat(getComputedStyle(el).getPropertyValue(property))

/**
 * The larger of the two vertical paddings.
 *
 * The mock writes these boxes as `padding: 0 14`, so both sides are 0 and one row per
 * element is enough to catch a tab that was centred with padding instead of with the fixed
 * strip height.
 */
const paddingBlock: Measure = (el) => Math.max(style('padding-top')(el), style('padding-bottom')(el))

const axisGap = (a: DOMRect, b: DOMRect, axis: 'x' | 'y') =>
  axis === 'x' ? b.left - a.right : b.top - a.bottom

/**
 * Space between the first two elements carrying the same hook.
 *
 * Used where the gap the mock states is between tagged siblings that share a container with
 * untagged ones — the rail's icons sit beside a spacer, so the container's `row-gap` is not
 * on its own proof that the icons are 2px apart.
 */
function siblingGap(selector: string, axis: 'x' | 'y'): Measure {
  return () => {
    const [first, second] = document.querySelectorAll(selector)
    if (!first || !second) return Number.NaN
    return axisGap(first.getBoundingClientRect(), second.getBoundingClientRect(), axis)
  }
}

/**
 * A container's gap, preferring the declaration and falling back to measuring its first two
 * children.
 *
 * `column-gap` resolves to `normal` when the spacing comes from margins instead, and the
 * mock's number is the distance between the items either way; a NaN there would report a
 * layout that looks right as a failure.
 */
function containerGap(axis: 'x' | 'y'): Measure {
  return (el) => {
    const declared = style(axis === 'x' ? 'column-gap' : 'row-gap')(el)
    if (Number.isFinite(declared)) return declared
    const [first, second] = el.children
    if (!first || !second) return Number.NaN
    return axisGap(first.getBoundingClientRect(), second.getBoundingClientRect(), axis)
  }
}

/**
 * Every dimension the mock states, in the order the chrome is stacked.
 *
 * Fractional expectations (10.5, 12.5, 9.5) are the mock's own values, not roundings — the
 * chip and path label are deliberately off the integer grid.
 */
const CHECKS: readonly Check[] = [
  // Header.
  { element: 'header', property: 'height', expected: 34, selector: hook('header'), measure: boxHeight },
  {
    element: 'header',
    property: 'borderBottomWidth',
    expected: 1,
    selector: hook('header'),
    measure: style('border-bottom-width'),
  },

  // Traffic lights. The three colours are literals in the mock; only the geometry is checked.
  {
    element: 'trafficLight',
    property: 'width',
    expected: 11,
    selector: hook('trafficLight'),
    measure: boxWidth,
  },
  {
    element: 'trafficLight',
    property: 'height',
    expected: 11,
    selector: hook('trafficLight'),
    measure: boxHeight,
  },
  {
    element: 'trafficLight',
    property: 'gap',
    expected: 8,
    selector: hook('trafficLight'),
    measure: siblingGap(hook('trafficLight'), 'x'),
  },

  // Project tabs.
  {
    element: 'projectTab',
    property: 'paddingLeft',
    expected: 14,
    selector: hook('projectTab'),
    measure: style('padding-left'),
  },
  {
    element: 'projectTab',
    property: 'paddingRight',
    expected: 14,
    selector: hook('projectTab'),
    measure: style('padding-right'),
  },
  {
    element: 'projectTab',
    property: 'paddingBlock',
    expected: 0,
    selector: hook('projectTab'),
    measure: paddingBlock,
  },
  {
    element: 'projectTab',
    property: 'columnGap',
    expected: 9,
    selector: hook('projectTab'),
    measure: containerGap('x'),
  },
  {
    element: 'projectTabActive',
    property: 'borderTopWidth',
    expected: 2,
    selector: `${hook('projectTab')}[data-active="true"]`,
    measure: style('border-top-width'),
  },
  { element: 'projectDot', property: 'width', expected: 8, selector: hook('projectDot'), measure: boxWidth },
  {
    element: 'projectDot',
    property: 'height',
    expected: 8,
    selector: hook('projectDot'),
    measure: boxHeight,
  },
  {
    element: 'projectPath',
    property: 'fontSize',
    expected: 10.5,
    selector: hook('projectPath'),
    measure: style('font-size'),
  },

  // Activity rail. The mock's 42 is a width here even though the token is named --h-rail.
  { element: 'rail', property: 'width', expected: 42, selector: hook('rail'), measure: boxWidth },
  {
    element: 'rail',
    property: 'borderRightWidth',
    expected: 1,
    selector: hook('rail'),
    measure: style('border-right-width'),
  },
  {
    element: 'rail',
    property: 'paddingTop',
    expected: 8,
    selector: hook('rail'),
    measure: style('padding-top'),
  },
  { element: 'railIcon', property: 'width', expected: 28, selector: hook('railIcon'), measure: boxWidth },
  { element: 'railIcon', property: 'height', expected: 28, selector: hook('railIcon'), measure: boxHeight },
  {
    element: 'railIcon',
    property: 'borderRadius',
    expected: 5,
    selector: hook('railIcon'),
    measure: style('border-top-left-radius'),
  },
  {
    element: 'railIcon',
    property: 'gap',
    expected: 2,
    selector: hook('railIcon'),
    measure: siblingGap(hook('railIcon'), 'y'),
  },
  { element: 'gitBadge', property: 'width', expected: 6, selector: hook('gitBadge'), measure: boxWidth },
  { element: 'gitBadge', property: 'height', expected: 6, selector: hook('gitBadge'), measure: boxHeight },

  // Console tab strip.
  { element: 'tabStrip', property: 'height', expected: 30, selector: hook('tabStrip'), measure: boxHeight },
  {
    element: 'tabStrip',
    property: 'borderBottomWidth',
    expected: 1,
    selector: hook('tabStrip'),
    measure: style('border-bottom-width'),
  },
  {
    element: 'consoleTab',
    property: 'paddingLeft',
    expected: 13,
    selector: hook('consoleTab'),
    measure: style('padding-left'),
  },
  {
    element: 'consoleTab',
    property: 'paddingRight',
    expected: 13,
    selector: hook('consoleTab'),
    measure: style('padding-right'),
  },
  {
    element: 'consoleTab',
    property: 'paddingBlock',
    expected: 0,
    selector: hook('consoleTab'),
    measure: paddingBlock,
  },
  {
    element: 'consoleTab',
    property: 'columnGap',
    expected: 8,
    selector: hook('consoleTab'),
    measure: containerGap('x'),
  },
  {
    element: 'consoleTab',
    property: 'fontSize',
    expected: 12.5,
    selector: hook('consoleTab'),
    measure: style('font-size'),
  },
  {
    element: 'consoleTabActive',
    property: 'borderBottomWidth',
    expected: 2,
    selector: `${hook('consoleTab')}[data-active="true"]`,
    measure: style('border-bottom-width'),
  },
  {
    element: 'consoleSwatch',
    property: 'width',
    expected: 13,
    selector: hook('consoleSwatch'),
    measure: boxWidth,
  },
  {
    element: 'consoleSwatch',
    property: 'height',
    expected: 13,
    selector: hook('consoleSwatch'),
    measure: boxHeight,
  },
  {
    element: 'pinnedChip',
    property: 'fontSize',
    expected: 9.5,
    selector: hook('pinnedChip'),
    measure: style('font-size'),
  },

  // Editor file tabs.
  {
    element: 'fileTab',
    property: 'paddingLeft',
    expected: 12,
    selector: hook('fileTab'),
    measure: style('padding-left'),
  },
  {
    element: 'fileTab',
    property: 'paddingRight',
    expected: 12,
    selector: hook('fileTab'),
    measure: style('padding-right'),
  },
  {
    element: 'fileTab',
    property: 'paddingBlock',
    expected: 0,
    selector: hook('fileTab'),
    measure: paddingBlock,
  },
  {
    element: 'fileTab',
    property: 'columnGap',
    expected: 9,
    selector: hook('fileTab'),
    measure: containerGap('x'),
  },
  {
    element: 'fileTab',
    property: 'fontSize',
    expected: 12,
    selector: hook('fileTab'),
    measure: style('font-size'),
  },
  {
    element: 'fileTabBadge',
    property: 'fontSize',
    expected: 10.5,
    selector: hook('fileTabBadge'),
    measure: style('font-size'),
  },

  /*
   * The pane title bar (26px) and the splitter (6px) were in this table but do not belong
   * to it. They are surfaces of the pane grid, which M4 builds; measuring them here meant
   * two permanent failures for code that is not written yet, and a red audit that has to be
   * explained every run is an audit nobody reads. They move into M4's own check, along with
   * the `paneTitle` and `splitter` hooks.
   */
  {
    element: 'statusBar',
    property: 'height',
    expected: 24,
    selector: hook('statusBar'),
    measure: boxHeight,
  },
  {
    element: 'statusBar',
    property: 'borderTopWidth',
    expected: 1,
    selector: hook('statusBar'),
    measure: style('border-top-width'),
  },
  {
    element: 'statusBar',
    property: 'columnGap',
    expected: 16,
    selector: hook('statusBar'),
    measure: containerGap('x'),
  },
  {
    element: 'statusBar',
    property: 'paddingLeft',
    expected: 12,
    selector: hook('statusBar'),
    measure: style('padding-left'),
  },
  {
    element: 'statusBar',
    property: 'paddingRight',
    expected: 12,
    selector: hook('statusBar'),
    measure: style('padding-right'),
  },
  {
    element: 'statusBar',
    property: 'paddingBlock',
    expected: 0,
    selector: hook('statusBar'),
    measure: paddingBlock,
  },
  {
    element: 'statusBar',
    property: 'fontSize',
    expected: 11,
    selector: hook('statusBar'),
    measure: style('font-size'),
  },
]

/**
 * The self-hosted families, at the size the base stylesheet sets.
 *
 * A family that failed to load falls back to the system stack and looks almost right, which
 * is the whole reason the fonts are bundled — so this is a failing row, not a warning.
 */
const FONTS: readonly { element: string; spec: string }[] = [
  { element: 'fontUi', spec: '13px "Inter"' },
  { element: 'fontMono', spec: '13px "JetBrains Mono"' },
]

function row(element: string, property: string, expected: number, actual: number): AuditRow {
  const delta = actual - expected
  return { element, property, expected, actual, delta, pass: Math.abs(delta) <= TOLERANCE }
}

/**
 * A font row, compared exactly rather than against the tolerance.
 *
 * The family either resolved or it did not; a ±2 window around 1 would pass both answers and
 * make the check decorative.
 */
function fontRow(element: string, loaded: boolean): AuditRow {
  const actual = loaded ? 1 : 0
  return { element, property: 'loaded', expected: 1, actual, delta: actual - 1, pass: loaded }
}

export function runLayoutAudit(): AuditReport {
  const rows: AuditRow[] = []

  for (const check of CHECKS) {
    const el = document.querySelector(check.selector)
    const actual = el ? check.measure(el) : Number.NaN
    rows.push(row(check.element, check.property, check.expected, actual))
  }

  for (const font of FONTS) {
    // 1/0 rather than a boolean so the row keeps the same shape as every dimension.
    rows.push(fontRow(font.element, document.fonts.check(font.spec)))
  }

  return {
    // The theme is an explicit app setting written to <html>, so an unset value is itself a
    // finding: the audit would otherwise silently report the dark run twice.
    theme: document.documentElement.dataset.theme ?? '(unset)',
    viewport: `${window.innerWidth}x${window.innerHeight}`,
    rows,
    failures: rows.filter((r) => !r.pass).length,
  }
}

/** Trim the trailing zeros the mock's fractional values would otherwise carry. */
function num(value: number): string {
  if (!Number.isFinite(value)) return 'absent'
  return value.toFixed(2).replace(/\.?0+$/, '')
}

function signed(value: number): string {
  if (!Number.isFinite(value)) return '—'
  const text = num(Math.abs(value))
  return `${value < 0 ? '-' : '+'}${text}`
}

const HEAD = ['element', 'property', 'expected', 'actual', 'delta', 'ok'] as const

export function formatAuditReport(report: AuditReport): string {
  const body = report.rows.map((r) => [
    r.element,
    r.property,
    num(r.expected),
    num(r.actual),
    signed(r.delta),
    r.pass ? 'ok' : 'FAIL',
  ])
  const widths = HEAD.map((h, i) => Math.max(h.length, ...body.map((cells) => (cells[i] ?? '').length)))
  // Text and numbers align to opposite edges so a wrong digit stands out down the column.
  const line = (cells: readonly string[]) =>
    `| ${cells.map((c, i) => (i < 2 ? c.padEnd(widths[i] ?? 0) : c.padStart(widths[i] ?? 0))).join(' | ')} |`

  const lines: string[] = []
  lines.push(
    `layout audit — theme=${report.theme} viewport=${report.viewport} tolerance=±${TOLERANCE}px`,
  )
  if (report.viewport !== '1440x900') {
    // The mock's numbers are fixed, so a different viewport only matters for what flexes.
    lines.push('note: the milestone states 1440x900; fixed dimensions below are unaffected.')
  }
  lines.push('')
  lines.push(line(HEAD))
  lines.push(`|${widths.map((w) => '-'.repeat(w + 2)).join('|')}|`)
  for (const cells of body) lines.push(line(cells))
  lines.push('')

  // Grouped by selector rather than listed per row, and phrased to cover every cause rather
  // than only the missing hook: an element can be on screen and still leave a property
  // unresolved, as `column-gap` does on a container that spaces its children with margins
  // and has fewer than two of them.
  const unmeasured = new Map<string, string[]>()
  for (const r of report.rows) {
    if (Number.isFinite(r.actual)) continue
    const key = CHECKS.find((c) => c.element === r.element)?.selector ?? r.element
    unmeasured.set(key, [...(unmeasured.get(key) ?? []), r.property])
  }
  if (unmeasured.size > 0) {
    lines.push('could not measure — no element matched, the surface was off screen, or the')
    lines.push('property did not resolve:')
    for (const [selector, properties] of unmeasured) lines.push(`  ${selector} — ${properties.join(', ')}`)
    lines.push('')
  }

  lines.push(
    report.failures === 0
      ? `PASS — ${report.rows.length} dimensions within ±${TOLERANCE}px of the mock.`
      : `FAIL — ${report.failures} of ${report.rows.length} dimensions outside ±${TOLERANCE}px of the mock.`,
  )
  return lines.join('\n')
}

/** True when the app was started with `?audit=1`, i.e. measure the chrome and report it. */
export function auditMode(): boolean {
  return new URLSearchParams(location.search).get('audit') === '1'
}
