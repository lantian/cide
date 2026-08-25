/**
 * The impure half of the Extensions panel: the store, the gestures, the notices. (M22)
 *
 * `AgentsPanelHost`'s shape. The view above it reads no store, calls no IPC and never reads the
 * clock, which is what lets `check-ext-render.mjs` render it under node with nothing stubbed
 * but `window` — the only gate in this project that can see a panel that compiles, mounts and
 * draws nothing. A `useExtStore` call moved down into the view would drag `@/ipc/client`, and with
 * it `@tauri-apps/api`, into a check whose whole value is that it needs neither.
 */
import { memo, useMemo, useState } from 'react'

import { notifyFailure } from '@/chrome/notices'
import { ext as extApi } from '@/ipc/client'
import type { Capability, ExtensionSnapshot, ProjectId } from '@/ipc/client'

export interface ExtensionsPanelProps {
  /**
   * The project whose tab strip an extension's page opens in.
   *
   * `null` when no project is open — and the panel still works, which is the point of it being
   * reachable from a window with nothing open: connecting a marketplace and installing an
   * extension are global acts. What is absent then is the *page*, because there is no strip to
   * put a tab in, and the row's name is drawn as plain text rather than as a dead link.
   */
  readonly project: ProjectId | null
}
import { useExtStore } from '@/ext/extStore'
import { ExtensionsPanelView } from './ExtensionsPanel'
import {
  NO_QUERY,
  buildModel,
  type ExtQuery,
  type InstalledIn,
  type LanguageIn,
  type MarketRowIn,
  type ProblemIn,
} from './model'

/**
 * Memoised, like every sidebar panel host: the panel is a direct child of `App`, which
 * re-renders on every store notification it subscribes to, and everything this panel draws
 * arrives through its own store subscriptions or through props `App` pins with `useCallback`
 * for exactly this. Without the memo, every App render re-walked the panel's full
 * unvirtualized row list for events that had nothing to do with it.
 */
export const ExtensionsPanel = memo(ExtensionsPanelImpl)

function ExtensionsPanelImpl({ project }: ExtensionsPanelProps): React.JSX.Element {
  const snapshot = useExtStore((state) => state.snapshot)
  const busy = useExtStore((state) => state.busy)
  const run = useExtStore((state) => state.run)

  /*
   * The search text and the filter chip.
   *
   * Host `useState` and not the store, on the rule `AgentsPanelHost` states for its own
   * `integrateArmed`: this is transient gesture state, and a second window showing the same
   * registry should not find its search box filled in because somebody typed here. It is also not
   * Rust's — `store/workspace.ts`'s header names "which overlay is open" and a mid-drag splitter
   * as the two things the webview legitimately owns, and a search box is the first of those.
   *
   * It does not survive the panel being closed, which is deliberate and matches every other
   * search in the app: `SearchPanel` and the pickers all start empty, because a filter still
   * applied from ten minutes ago is a panel that looks broken.
   */
  const [query, setQuery] = useState<ExtQuery>(NO_QUERY)

  // Built in the host and handed down as a plain value, so the view stays ignorant of the wire.
  // Memoised on the snapshot's identity — which the store only ever replaces wholesale — and on
  // the query, which is the only other input.
  const model = useMemo(() => buildModel(...adapt(snapshot), query), [snapshot, query])

  /**
   * Every write surfaces its own failure.
   *
   * `guarded` is `AgentsPanelHost`'s, and the placement is the point: the store deliberately does
   * not import `chrome/notices`, so the surfacing stays at the gesture. A store that notified
   * would notify for a refresh nobody asked for.
   */
  const guarded = (work: () => Promise<ExtensionSnapshot>): void => {
    void run(work).catch(notifyFailure)
  }

  return (
    <ExtensionsPanelView
      model={model}
      busy={busy}
      onConnect={(source) => guarded(() => extApi.connect(source))}
      onRefresh={(marketplace) => guarded(() => extApi.refresh(marketplace))}
      onDisconnect={(marketplace) => guarded(() => extApi.disconnect(marketplace))}
      onInstall={(marketplace, extension, capabilities) =>
        guarded(() =>
          // The capabilities the *row was drawn with*, echoed back. Rust refuses the install if the
          // manifest has changed since — which is the whole consent mechanism, and the reason this
          // is threaded through the view rather than re-read here from the snapshot.
          extApi.install({ marketplace, extension }, capabilities as readonly Capability[]),
        )
      }
      onUninstall={(marketplace, extension) =>
        guarded(() => extApi.uninstall({ marketplace, extension }))
      }
      onSetEnabled={(marketplace, extension, enabled) =>
        guarded(() => extApi.setEnabled({ marketplace, extension }, enabled))
      }
      onQuery={(text, filter) => setQuery({ text, filter })}
      {...(project === null
        ? {}
        : {
            onOpen: (marketplace: string, extension: string, name: string) => {
              // Not through `run`: opening a tab is a workspace mutation and answers with a
              // `TabId`, not a snapshot, so it goes back through `cide://workspace-changed` like
              // every other tab. `busy` is the extension registry's spinner and this touches none
              // of it.
              void extApi
                .openTab(project, { marketplace, extension }, name)
                .catch(notifyFailure)
            },
          })}
    />
  )
}

/**
 * Wire snapshot to model input.
 *
 * Only shapes change: `model.ts` restates its inputs structurally so it can be compiled standalone
 * by a check script, and this is the one place the two descriptions meet.
 */
function adapt(
  snapshot: ExtensionSnapshot,
): [
  readonly MarketRowIn[],
  readonly InstalledIn[],
  readonly ProblemIn[],
  readonly LanguageIn[],
] {
  return [
    snapshot.marketplaces.map((market) => ({
      id: market.id,
      name: market.name,
      source: market.source,
      authenticated: market.authenticated,
      state: market.state,
      entries: market.entries.map((entry) => ({
        id: entry.id,
        name: entry.name,
        version: entry.version,
        description: entry.description,
        capabilities: entry.capabilities,
        installed: entry.installed,
        updateAvailable: entry.updateAvailable,
        unavailable: entry.unavailable,
      })),
      problems: market.problems.map(problem),
    })),
    snapshot.extensions.map((row) => ({
      marketplace: row.marketplace,
      extension: row.extension,
      name: row.name,
      version: row.version,
      enabled: row.enabled,
      capabilities: row.capabilities,
      unavailable: row.unavailable,
      problems: row.problems.map(problem),
    })),
    [...snapshot.problems.map(problem), ...snapshot.resolved.conflicts.map(problem)],
    snapshot.resolved.languages.map((binding) => ({
      id: binding.def.id,
      label: binding.def.label,
      extensions: binding.def.extensions.map((ext) => ext.ext),
      source: sourceName(binding.source),
      supersedes: binding.supersedes === undefined ? null : sourceName(binding.supersedes),
    })),
  ]
}

/** `null` for a builtin, else the extension that contributed it. Matches `cide-headless ext`. */
function sourceName(source: ExtensionSnapshot['resolved']['languages'][number]['source']): string | null {
  return source.kind === 'builtin'
    ? null
    : `${source.extension.marketplace}.${source.extension.extension}`
}

function problem(item: ExtensionSnapshot['problems'][number]): ProblemIn {
  return {
    path: item.path,
    // `bigint` on the wire (a `u32` through ts-rs), a plain number in the model — the same
    // narrowing `ProblemsPanel/adapt.ts` does, and for the same reason: a line number is never
    // large enough to need one and every consumer would otherwise have to remember the `n`.
    line: item.line === undefined ? undefined : Number(item.line),
    severity: item.severity,
    message: item.message,
  }
}
