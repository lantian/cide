/**
 * The Docker panel's impure half: the store, the gestures, the first read. (M41)
 *
 * `ExtensionsPanelHost`'s shape, including the `memo`: this panel is a direct child of `App`,
 * which re-renders on every workspace notification, and without it every keystroke in a terminal
 * would re-render a list of containers that had not changed.
 *
 * **It does not subscribe to `cide://docker-changed`.** That listener lives in `App.tsx`, and it
 * has to: the rail's badge must stay live while the sidebar is shut or showing Files, and a
 * listener registered here goes stale the moment this panel unmounts.
 */
import { memo, useEffect, useState } from 'react'

import { ConfirmDestructive, type ConfirmState } from '@/chrome/ConfirmDestructive'
import { docker as dockerApi } from '@/ipc/client'
import { errorText } from '@/ipc/errorText'
import { activeProjectIdOf, activeTabOf } from '@/keys/target'
import type { InspectTarget, PaneId, ProjectId, TabId } from '@/ipc/client'
import { useWorkspace } from '@/store/workspace'
import { useDocker } from '../dockerStore'
import { removalPrompt, resolveRef, type Removable } from './model'
import { DockerPanel as DockerPanelView, type OpenStream } from './DockerPanel'
import { DetailPane } from './DetailPane'
import { adaptDetail } from './detailAdapt'
import type { Detail, DetailPort, VolumeSize } from './detailModel'
import { isDestructive } from './model'

/**
 * Where a new pane goes: this window's active project, its active tab, its focused pane.
 *
 * `activeProjectIdOf` and `activeTabOf` rather than reading `workspace.active` directly — a
 * detached-pane window is not a shell and answers `null` for both, which is what keeps this
 * panel's buttons from splitting a tab in the *other* window. `keys/target.ts` carries that
 * argument at length.
 *
 * Three separate selectors, each returning a **primitive**. One selector returning
 * `{project, tab, pane}` would build a fresh object on every store notification and re-render
 * for ever — `check:selectors`' whole subject — so the object is assembled here, outside the
 * store, where a new identity costs nothing.
 */
function useSplitTarget(): { project: ProjectId; tab: TabId; pane: PaneId } | null {
  const project = useWorkspace((s) => activeProjectIdOf(s.boot))
  const tab = useWorkspace((s) => activeTabOf(s.boot)?.id ?? null)
  const pane = useWorkspace((s) => activeTabOf(s.boot)?.tree.focused ?? null)
  if (project === null || tab === null || pane === null) return null
  return { project, tab, pane }
}

export interface DockerPanelHostProps {
  /**
   * Where this panel is drawn. `ExtPanelHost`'s prop and its spelling, deliberately: a panel that
   * can sit in either place is the same question those already answer, and a second vocabulary
   * for it would be a second thing to keep matched.
   *
   * The only difference it makes is the **title**. In the bottom panel the tab strip immediately
   * above already says "Docker", and a panel heading under it is the same word twice — which is
   * exactly what a sidebar panel dropped into the bottom panel looks like.
   */
  readonly placement?: 'sidebar' | 'bottom'
}

export const DockerPanel = memo(function DockerPanelHost({
  placement = 'sidebar',
}: DockerPanelHostProps) {
  const board = useDocker((state) => state.board)
  const busy = useDocker((state) => state.busy)
  const refresh = useDocker((state) => state.refresh)
  const act = useDocker((state) => state.act)
  const use = useDocker((state) => state.use)
  const removeResource = useDocker((state) => state.remove)
  const target = useSplitTarget()
  const splitPane = useWorkspace((s) => s.splitPane)
  const composeAct = useDocker((state) => state.compose)
  /*
   * The pending destructive question, or `null`. Held here rather than resolved inline because
   * `ConfirmDestructive` is a *rendered* dialog and not a blocking call — which is the whole
   * reason it works where `confirm()` did not.
   */
  /*
   * Tell Rust this panel is on screen, and tell it when it stops being. (M52)
   *
   * The daemon's event subscription is an open socket and a **full board read** per event, and it
   * was started lazily and never stopped — so a session that opened this tab once kept reading the
   * daemon for the rest of its life. Reported as continuous `bollard` DEBUG lines in the log with
   * the panel closed, which is how it is visible; it was really four API calls per daemon event.
   *
   * `App.tsx` subscribes to `cide://docker-changed` app-wide and always will, because that is a
   * listener and costs nothing. The *work* is here, where the thing that consumes it is — and
   * nothing outside this panel has read the board since M46 moved it out of the rail.
   *
   * The cleanup runs on a project switch and a tab change as well as a window close, which is
   * right: all three mean nobody is looking. Reopening starts it again on the next board read.
   */
  useEffect(() => {
    void dockerApi.watch(true).catch(() => {})
    return () => {
      void dockerApi.watch(false).catch(() => {})
    }
  }, [])

  const [pendingRemove, setPendingRemove] = useState<ConfirmState | null>(null)
  /*
   * The selected row and what it is.
   *
   * Two pieces of state rather than one, because they move at different times: the selection is
   * immediate (a click) and the detail is a round trip. Collapsing them would blank the pane for
   * a frame on every click, which is the flicker a detail pane exists to avoid.
   */
  const [selected, setSelected] = useState<InspectTarget | null>(null)

  /*
   * A volume's size, measured only while one is selected. (M57)
   *
   * Keyed by the volume's name so a second selection cannot be answered with the first one's
   * number: this is a round trip of a second or more, and `selected` can move twice inside it.
   * The `cancelled` flag is the other half — a late answer for a volume nobody is looking at any
   * more must not be drawn.
   *
   * **Never for any other kind, and never on a board change.** `GET /volumes` carries no size at
   * all; `/system/df` walks the filesystem and was measured at 8.4 seconds cold against a real
   * daemon. Asking for it on every refresh — of which the event stream produces one per container
   * that starts anywhere on the machine — would be that walk, continuously.
   */
  const [volumeSize, setVolumeSize] = useState<VolumeSize | undefined>(undefined)
  const volumeName = selected?.kind === 'volume' ? selected.name : null

  useEffect(() => {
    if (volumeName === null) {
      setVolumeSize(undefined)
      return
    }
    let cancelled = false
    setVolumeSize({ state: 'measuring' })
    void dockerApi
      .volumeSize(volumeName)
      .then((bytes) => {
        if (cancelled) return
        // `null` is the daemon declining to measure, and it is **not** zero: a volume drawn as
        // `0 B` when nobody counted is a claim about somebody's data.
        setVolumeSize(bytes === null ? { state: 'unmeasured' } : { state: 'known', bytes: Number(bytes) })
      })
      .catch(() => {
        // The refusal has already been reported by whatever rejected; this pane only needs to
        // stop saying "measuring…" for ever.
        if (!cancelled) setVolumeSize({ state: 'unmeasured' })
      })
    return () => {
      cancelled = true
    }
  }, [volumeName])

  const [detail, setDetail] = useState<Detail | null>(null)

  /*
   * Re-read whenever the selection changes **or the board does** — so a container the user
   * started shows its new state without a second click, and one that was removed turns into the
   * `missing` sentence rather than sitting there as a stale description.
   */
  useEffect(() => {
    if (selected === null) {
      setDetail(null)
      return
    }
    let disposed = false
    void dockerApi
      .detail(selected)
      .then((wire) => {
        if (!disposed) setDetail(adaptDetail(wire))
      })
      .catch((error: unknown) => {
        if (!disposed) setDetail({ kind: 'missing', reason: errorText(error) })
      })
    return () => {
      disposed = true
    }
  }, [selected, board])

  // The first read. `App.tsx` keeps the board fresh afterwards through the event, so this fires
  // when the panel is first shown and not again — and the store's own `newerBoard` drop rule
  // means a read that lands after an event cannot roll the board back.
  useEffect(() => {
    if (board.kind === 'unknown') void refresh()
  }, [board.kind, refresh])

  return (
    <>
      {pendingRemove !== null && (
        <ConfirmDestructive
          state={pendingRemove}
          onCancel={() => setPendingRemove(null)}
          onConfirm={() => {
            const run = pendingRemove.run
            setPendingRemove(null)
            run()
          }}
        />
      )}
    <DockerPanelView
      placement={placement}
      board={board}
      busy={busy}
      onRefresh={() => void refresh()}
      onAction={(container, action) => {
        if (!isDestructive(action)) {
          void act(container, action)
          return
        }
        const name =
          board.containers.find((row) => row.id === container)?.name ?? container
        setPendingRemove(removeConfirm(name, container, () => void act(container, action)))
      }}
      /*
       * Remove an image, a volume or a network. (M56)
       *
       * Absent unless the board is ready, so the buttons are **not drawn** rather than drawn
       * dead — `DetailPaneProps.onApplyPorts`' rule, and the one this panel keeps relearning.
       *
       * Confirmed first, like a container's removal, and the dialog says the thing the user
       * actually needs to know: Docker **refuses while anything is using it**. That is the
       * ordinary outcome rather than the rare one, so a dialog warning only about
       * irreversibility would surprise them with the common case.
       */
      onRemove={
        board.kind !== 'ready'
          ? undefined
          : (what, name) => {
              setPendingRemove(resourceConfirm(what, name, () => void removeResource(what)))
            }
      }
      onUseEndpoint={(endpoint) => void use(endpoint)}
      /*
       * Absent when no project is open, which is why the buttons are simply not drawn rather
       * than drawn dead: a pane lives in a tab and a tab lives in a project, and this panel is
       * machine-scoped so it is shown either way.
       *
       * A **row** split (`'row'`, `'after'`) — beside the focused pane rather than under it,
       * because a terminal wants columns and a log follow wants width for long lines.
       */
      onOpenStream={
        target === null
          ? undefined
          : (container, name, stream: OpenStream) => {
              // The file browser is a **tab**, not a pane: it is a document with its own chrome
              // — a breadcrumb, a list and a viewer — and there is nothing for a split to split.
              // The other two are sessions and belong in the pane grid beside a terminal.
              if (stream === 'files') {
                void dockerApi.openFiles(target.project, container, name)
                return
              }
              void splitPane(target.project, target.tab, target.pane, 'row', 'after', {
                kind: 'docker',
                container,
                name,
                stream,
              })
            }
      }
      /*
       * Absent when no project is open — an inspect tab lives in a project. The rows are still
       * drawn and are still useful; only the double-click does nothing.
       */
      onInspect={
        target === null
          ? undefined
          : (inspectTarget, name) => {
              void dockerApi.openInspect(target.project, inspectTarget, name)
            }
      }
      /*
       * Absent when `docker compose` is missing, which the *view* decides from the board —
       * `stackBlocked`. Passing the callback unconditionally and letting it fail would be a
       * button that reports a problem after it is pressed, which is the shape this project has
       * paid for repeatedly.
       */
      selected={selected}
      onSelect={setSelected}
      detail={
        <DetailPane
          detail={detail}
          busy={busy}
          volumeSize={volumeSize}
          /*
           * What the **list** says this image occupies. (M58)
           *
           * The board already has it, so no round trip — and it has to come from here rather than
           * from the detail, because the two endpoints disagree on purpose and only the caller
           * sees both. `undefined` when the row has gone, which `imageSizeRows` handles.
           */
          imageOnDisk={
            selected?.kind === 'image' && board.kind === 'ready'
              ? board.images
                  .filter((row) => row.id === selected.id)
                  .map((row) => Number(row.size))[0]
              : undefined
          }
          /*
           * Follow a reference to another object. (M50)
           *
           * Passed as `undefined` when the board is not ready, so the pane draws plain text
           * rather than links that could not resolve — `DetailPane.Row`'s rule, and the reason it
           * is the *host* that decides: whether `nginx:latest` is a row in the left column is a
           * question about the board, which the pane has never seen.
           *
           * Selecting is the whole of the action here. Scrolling the row into view and opening
           * the heading it lives under are the **panel's** to do, because only it knows which
           * sections are folded — see `DockerPanel`'s effect on `selected`.
           */
          onFollow={
            board.kind !== 'ready'
              ? undefined
              : (ref) => {
                  const target = resolveRef(board, ref)
                  // `null` is ordinary: a bind mount names a host path, an image may have been
                  // removed, a container may be gone since this snapshot was taken. The pane
                  // asked because the row carried a ref; there being no row is not a failure and
                  // says nothing.
                  if (target !== null) setSelected(target)
                }
          }
          onOpenRaw={
            target === null || selected === null
              ? undefined
              : () => {
                  const name = detail !== null && detail.kind !== 'missing' ? detail.title : ''
                  void dockerApi.openInspect(target.project, selected, name)
                }
          }
          /*
           * Only a container can be edited, and only its ports. Everything else Docker treats as
           * immutable too, but a port is the one people actually change — and it is the one the
           * recreate can carry without asking the user to restate the container.
           */
          onApplyPorts={
            detail !== null && detail.kind === 'container'
              ? (ports) => void applyPorts(detail.id, ports)
              : undefined
          }
        />
      }
      onStackAction={
        board.kind === 'ready' && board.compose.present
          ? (project, action, workingDir, files) =>
              void composeAct(project, action, workingDir, files)
          : undefined
      }
    />
    </>
  )
})

/**
 * Ask before anything irreversible.
 *
 * # Why not `confirm()`
 *
 * Because it does not work here, and the way it fails is not a dialog that looks wrong — it is a
 * *notification* saying `dialog.confirm not allowed. Command not found`. Tauri intercepts
 * `window.confirm` and routes it to its dialog plugin, which needs a capability cide does not
 * grant; the reply comes back as a rejected command whose text reads like a stale binary. So the
 * first version of this asked a question nobody was shown and removed nothing.
 *
 * `ConfirmDestructive` is the app's own dialog and the one every other destructive act in cide
 * goes through — the file tree's *Move to Trash*, the git panel's reverts, the log's resets. It
 * also gets the two things `confirm()` cannot: the **list of what is at risk**, which is this
 * dialog's whole rule, and `defaultButton`, which decides whether Enter confirms.
 */
/**
 * Replace a container with one published on these ports.
 *
 * Named `applyPorts` at the call site and `recreate` everywhere behind it, which is the honest
 * split: *apply* is what the user is doing, *recreate* is what Docker makes it mean. There is no
 * confirmation dialog — that was a deliberate choice — so the button's own label and the line
 * beside it are where the consequence is stated.
 */
async function applyPorts(container: string, ports: readonly DetailPort[]): Promise<void> {
  const store = useDocker.getState()
  if (store.busy) return
  await store.recreate(container, ports)
}

/**
 * The question asked before removing an image, a volume or a network. (M56)
 *
 * Separate from [`removeConfirm`] rather than a parameterised version of it, because the two say
 * genuinely different things. A container's removal **forces** — it is stopped first and it will
 * happen — so that dialog is about irreversibility. These refuse while anything is using them, so
 * this one is about the refusal: it is the likely answer, and a user told only "this cannot be
 * undone" would read a refusal as a bug.
 */
function resourceConfirm(what: Removable, name: string, run: () => void): ConfirmState {
  const { title, body } = removalPrompt(what, name)
  return {
    title,
    body,
    // What the act will actually touch. A volume is addressed by name and the other two by id,
    // which is the daemon's asymmetry — showing the handle beside the name is what lets somebody
    // check the dialog is about the row they clicked.
    files: [what.kind === 'volume' ? what.name : `${name}  (${what.id.slice(0, 12)})`],
    confirmLabel: `Remove ${what.kind}`,
    mark: 'trash-2',
    // **Not** `confirm`, `removeConfirm`'s rule: irreversible when it succeeds, so Enter must not
    // be the answer.
    defaultButton: 'cancel',
    run,
  }
}

function removeConfirm(name: string, container: string, run: () => void): ConfirmState {
  return {
    title: `Remove ${name}?`,
    // A container is not a file: there is no trash and nothing to restore from. The sentence
    // says so rather than leaving "remove" to be read as reversible.
    body:
      'The container is stopped first if it is running. This cannot be undone — but its image ' +
      'and any named volumes it used are left alone, so the data survives.',
    // The one thing at risk, named. `ConfirmDestructive`'s rule is that the list is what the act
    // will actually touch, and a container is addressed by id even though the name is what is
    // shown — so the row carries both.
    files: [`${name}  (${container.slice(0, 12)})`],
    confirmLabel: `Remove ${name}`,
    mark: 'trash-2',
    // **Not** `confirm`: this is irreversible, so Enter must not be the answer. The file tree's
    // *Move to Trash* opts in precisely because it is reversible.
    defaultButton: 'cancel',
    run,
  }
}
