/**
 * The page a "page tab" draws — Settings, an extension, an OpenSpec change, a symbol's docs, a
 * `docker inspect` document, a container's files.
 *
 * # Why this is one file
 *
 * These pages used to be drawn by `App.tsx`'s `renderTree`, which *replaced* the tab's pane tree
 * with the page. That made a split on such a tab invisible: the domain added the new pane to a
 * tree nothing rendered, so the header's split gesture looked dead. Splits are now allowed, and
 * the page becomes one pane of the grid — the tree's **editor-kind** pane, exactly the way a File
 * tab's editor is (`PaneBody` dispatches on the *pane* kind, never the tab's, so a split's
 * Claude/shell pane is never mistaken for the page). Two places draw the page — `renderTree`
 * while the tab is still one pane (no frame around the screen, no remount cost for the common
 * case) and `PaneBody` once the tab has been split — and one switch here keeps their props from
 * drifting apart.
 *
 * Every tab is mounted, hidden or not (`TabContent` hides with `visibility`), so a page that
 * bound a window-level key listener would bind one per open tab. `active` tells such a page its
 * tab is the one in front; the OpenSpec page reads it.
 */
import type { ReactNode } from 'react'
import type { ProjectId, TabKind } from '@/ipc/client'
import { ExtensionTab } from '@/ext/ExtensionTab'
import { DockerFilesPane } from './DockerFilesPane'
import { DockerInspectPane } from './DockerInspectPane'
import { DocsPane } from './DocsPane'
import { SettingsTab } from '@/settings/SettingsTab'
import { SpecTab } from '@/sidebar/OpenSpecPanel/SpecTab'

/** The tab kinds whose content is a page rather than a pane grid. Not `conversation`: a saved
 *  conversation is read-only and the domain refuses to split it. */
export function isPageTab(kind: TabKind): boolean {
  switch (kind.kind) {
    case 'settings':
    case 'extension':
    case 'openSpec':
    case 'docs':
    case 'docker':
    case 'dockerFiles':
      return true
    default:
      return false
  }
}

/**
 * Whether a page tab still holds nothing but its page — the placeholder editor pane the domain
 * creates with the tab (`open_settings_tab` and its siblings: no session, no process).
 *
 * Anything else — a second pane, or the page's own pane closed and only a shell left — is a grid,
 * and `SplitTree` draws it.
 */
export function isPageOnly(tree: { panes: Record<string, { kind: string }> }): boolean {
  const panes = Object.values(tree.panes)
  return panes.length === 1 && panes[0]?.kind === 'editor'
}

export function PageTabBody({
  project,
  kind,
  active,
}: {
  project: ProjectId
  kind: TabKind
  active: boolean
}): ReactNode {
  switch (kind.kind) {
    case 'settings':
      return <SettingsTab project={project} section={kind.section} />
    case 'extension':
      // A page *about* cide rather than a document in the project.
      return (
        <ExtensionTab
          id={{ marketplace: kind.marketplace, extension: kind.extension }}
          name={kind.name}
        />
      )
    case 'openSpec':
      // A change or a capability: a page about a *directory*, which is why opening
      // `proposal.md` was not enough (`TabKind::OpenSpec` argues it).
      return (
        <SpecTab
          project={project}
          active={active}
          {...(kind.subject.kind === 'change'
            ? { change: kind.subject.change }
            : { spec: kind.subject.spec })}
        />
      )
    case 'docs':
      return <DocsPane project={project} subject={kind.subject} title={kind.title} />
    case 'docker':
      return <DockerInspectPane target={kind.target} title={kind.title} />
    case 'dockerFiles':
      return <DockerFilesPane container={kind.container} name={kind.name} />
    default:
      return null
  }
}
