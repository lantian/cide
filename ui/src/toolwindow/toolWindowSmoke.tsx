/**
 * Server-renders the git tool window's **frame** and prints a digest of what came out.
 *
 * `check:toolwindow` already exercises the rules as functions — which tab comes back after a
 * close, whether the panel stays open. This proves the frame *paints* them: that the tab row
 * really does put Log first, really does withhold a close control from it, and really does give
 * every history tab one. Those three are the kind of rule a refactor breaks silently, because
 * every one of them still renders a tab row that looks plausible.
 *
 * Nothing here imports a store, an `invoke`, or anything that touches `document` at module scope
 * — that is the whole reason `ToolWindow.tsx` (the view) is split from `ToolWindowHost.tsx` (the
 * wiring), and the split only pays off if this entry can stay on the pure side of it.
 *
 * Consumed by `ui/scripts/check-toolwindow-render.mjs`; nothing in the app imports this file, so
 * it is tree-shaken out of the bundle.
 */
import { renderToStaticMarkup } from 'react-dom/server'
import { ToolWindowView } from './ToolWindow'
import { openHistory, showLog, tabRow, TABS_INITIAL, type ToolWindowTabs } from './toolWindowModel'

export interface FrameDigest {
  story: string
  /** Every tab's label, in drawing order. */
  labels: string[]
  /** The label of the tab marked `aria-selected`, or `null`. */
  active: string | null
  /** Labels of the tabs that render a close control. */
  closable: string[]
  /** Whether the panel's own hide control is drawn. */
  hide: boolean
  /** The body text, which is what the host slots the active tab's view into. */
  body: string
  /** How many elements claim `role="tab"` — the Log tab plus each history tab, and nothing else. */
  tabRoles: number
}

function digest(story: string, tabs: ToolWindowTabs): FrameDigest {
  const rows = tabRow(tabs)
  const html = renderToStaticMarkup(
    <ToolWindowView
      rows={rows}
      onActivate={() => {}}
      onClose={() => {}}
      onHide={() => {}}
    >
      <div>body for {rows.find((r) => r.active)?.label ?? 'nothing'}</div>
    </ToolWindowView>,
  )
  const tabs_ = [
    ...html.matchAll(/role="tab"[^>]*aria-selected="(\w+)"[^>]*>([^<]*)</g),
  ]
  return {
    story,
    labels: tabs_.map((m) => m[2] ?? ''),
    active: tabs_.find((m) => m[1] === 'true')?.[2] ?? null,
    closable: [...html.matchAll(/aria-label="Close ([^"]*)"/g)].map((m) => m[1] ?? ''),
    hide: html.includes('data-audit="toolWindowHide"'),
    body:
      /data-audit="toolWindowBody"[^>]*>([\s\S]*?)<\/div>/
        .exec(html)?.[1]
        ?.replace(/<[^>]*>/g, '')
        .trim() ?? '',
    tabRoles: [...html.matchAll(/role="tab"/g)].length,
  }
}

const one = openHistory(TABS_INITIAL, 'h1', 'r1', 'crates/cide-git/src/log.rs')
const two = openHistory(one, 'h2', 'r1', 'ui/src/App.tsx')

const digests: FrameDigest[] = [
  digest('logOnly', TABS_INITIAL),
  digest('oneHistory', one),
  digest('twoHistories', two),
  // The Log tab back in front while two history tabs stay open — the state a close falls back to.
  digest('logInFront', showLog(two)),
]

console.log(JSON.stringify(digests))
