/**
 * The impure half of a contributed panel: the worker, the clock of visibility, the gestures. (M22)
 *
 * `AgentsPanel`'s host/view split, and here it earns itself twice over. The view is renderable
 * under node because it reads nothing; and the *worker* — which is where an extension's state
 * lives — is reached through `ext/host.ts`'s module-level map rather than through a hook, so this
 * component unmounting when the user clicks a different rail button does not stop the extension.
 */
import { useCallback, useEffect, useSyncExternalStore } from 'react'

import type { PanelBinding } from '@/ipc/client'
import { ExtPanelView } from './ExtPanelView'
import { invoke, notePanelHidden, notePanelShown, subscribe, viewOf } from './host'

export interface ExtPanelHostProps {
  readonly binding: PanelBinding
  readonly placement?: 'sidebar' | 'bottom'
}

export function ExtPanelHost({
  binding,
  placement = 'sidebar',
}: ExtPanelHostProps): React.JSX.Element {
  // `useSyncExternalStore` over the host's own listener set, not a zustand store: the views live
  // outside React because the workers do, and a second copy in a store would be a second thing to
  // keep in step. `viewOf` returns the *same object* for an unchanged view, which is what makes
  // this safe — `check:selectors` exists because a selector returning a fresh value re-renders for
  // ever and ends at Maximum update depth exceeded, which unmounts the whole root.
  const view = useSyncExternalStore(
    subscribe,
    () => viewOf(binding.extension, binding.def.id),
    () => viewOf(binding.extension, binding.def.id),
  )

  // Telling the worker when it is on screen is what lets a well-written extension do nothing at
  // all while its panel is closed. Without it every extension pays for every editor change for the
  // whole session, which on one JavaScript thread (ADR 0001) is a cost the user feels.
  useEffect(() => {
    notePanelShown(binding.extension, binding.def.id)
    return () => {
      notePanelHidden(binding.extension, binding.def.id)
    }
  }, [binding.extension, binding.def.id])

  const onAction = useCallback(
    (id: string) => {
      invoke(binding.extension, id, binding.def.id, null)
    },
    [binding.extension, binding.def.id],
  )
  const onActivate = useCallback(
    (row: string) => {
      // A row activation is an `invoke` with the reserved command name `activate`, rather than a
      // note of its own. One message shape for "the user pressed something" keeps the worker's
      // switch small, and an extension that wants to tell a toolbar press from a row press has the
      // `row` field to do it with.
      invoke(binding.extension, 'activate', binding.def.id, row)
    },
    [binding.extension, binding.def.id],
  )

  return (
    <ExtPanelView
      label={binding.def.label}
      view={view}
      placement={placement}
      onAction={onAction}
      onActivate={onActivate}
    />
  )
}
