/**
 * Local History for a file, a folder or a whole project: versions on the left, a diff on the
 * right.
 *
 * A page that replaces the tab's pane tree, as Settings and the OpenSpec tab do. Every tab stays
 * mounted, so everything here fetches only while `active`, and refetches when it next becomes so.
 *
 * # What a row means
 *
 * A row is a *state* of the file, and selecting it shows that state against the file as it is
 * now. **Restore This Version** then makes the file exactly what the left side of the diff shows.
 * JetBrains' *Revert Selected and Later Changes* undoes the selected change instead, which lands
 * one row earlier and is the wrong thing to have to work out while recovering from a mistake.
 */
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type ReactNode,
} from 'react'
import {
  getDiffView,
  getServerDiffView,
  setDiffView,
  subscribeDiffView,
} from '@/editor/diffViewMode'
import { VirtualList } from '@/chrome/VirtualList'
import {
  events,
  localHistory,
  settings as settingsApi,
  type LocalHistoryGroup,
  type LocalHistoryStatus,
  type LocalVersion,
  type ProjectId,
  type RevisionDiff,
} from '@/ipc/client'
import { Badge } from '@/kit/components/Status'
import { Button } from '@/kit/components/Button'
import { Banner, EmptyState, Spinner } from '@/kit/components/Feedback'
import { SearchField } from '@/kit/components/Field'
import { Row, Toolbar, ToolbarSpacer } from '@/kit/components/Surface'
import { useContextMenu } from '@/menus/useContextMenu'
import { GitDiffView, useDiffTokens } from '@/panes/GitDiffPane'
import { ConfirmDestructive, type ConfirmState } from '@/chrome/ConfirmDestructive'
import { notify, notifyFailure } from '@/chrome/notices'
import { compareVersion, copyPatch, requestPutLabel, restoreVersion } from './actions'
import {
  compareRefusal,
  comparedPair,
  groupTitle,
  kindLabel,
  reportLine,
  restoreRefusal,
  sourceLabel,
  timeLabel,
  versionTitle,
  withDays,
  type TimelineLine,
} from './model'
import styles from './LocalHistoryTab.module.css'

const PAGE = 200

/** One fetch at a time per key: an answer for a row no longer selected is dropped. */
function useLatest<T>(
  load: (() => Promise<T>) | null,
  key: string,
): { value: T | null; error: string | null; busy: boolean } {
  const [state, setState] = useState<{ value: T | null; error: string | null; busy: boolean }>({
    value: null,
    error: null,
    busy: false,
  })
  const loader = useRef(load)
  loader.current = load
  useEffect(() => {
    const run = loader.current
    if (run === null) {
      setState({ value: null, error: null, busy: false })
      return
    }
    let alive = true
    setState((old) => ({ ...old, busy: true }))
    run().then(
      (value) => {
        if (alive) setState({ value, error: null, busy: false })
      },
      (reason) => {
        if (alive) setState({ value: null, error: String(reason), busy: false })
      },
    )
    return () => {
      alive = false
    }
  }, [key])
  return state
}

function VersionDiff({
  project,
  path,
  version,
  other,
  nonce,
  visible,
}: {
  project: ProjectId
  path: string
  version: LocalVersion
  other: LocalVersion | null
  /** Moves when the disk may have, because "current" is not a frozen side. */
  nonce: number
  visible: boolean
}) {
  const { from, to } = comparedPair(version, other)
  const key = `${path}:${from.id}:${to?.id ?? 'current'}:${nonce}`
  const { value: diff, error, busy } = useLatest<RevisionDiff>(
    () =>
      localHistory.diff(
        project,
        path,
        { kind: 'version', id: from.id },
        to === null ? { kind: 'current' } : { kind: 'version', id: to.id },
      ),
    key,
  )
  const [collapsed, setCollapsed] = useState<ReadonlySet<number>>(new Set())
  const [gaps, setGaps] = useState<ReadonlySet<number>>(new Set())
  const [current, setCurrent] = useState(0)
  const mode = useSyncExternalStore(subscribeDiffView, getDiffView, getServerDiffView)
  const tokens = useDiffTokens(diff)
  return (
    <GitDiffView
      readOnly
      diff={diff}
      path={path}
      busy={busy && diff === null}
      reason={error}
      note={null}
      collapsed={collapsed}
      revisions={{
        from: timeLabel(from.ts),
        to: to === null ? 'Current' : timeLabel(to.ts),
      }}
      view={mode.view}
      onView={mode.writable ? setDiffView : undefined}
      tokens={tokens}
      visible={visible}
      currentChange={current}
      onCurrentChange={setCurrent}
      onCollapse={(hunk) =>
        setCollapsed((old) => {
          const next = new Set(old)
          if (next.has(hunk)) next.delete(hunk)
          else next.add(hunk)
          return next
        })
      }
      foldUnchanged
      expandedGaps={gaps}
      onExpandGap={(gap) => setGaps((old) => new Set([...old, gap]))}
    />
  )
}

/** A list that scrolls, with a header per day. `Row` needs a `listbox` around it. */
function Timeline<T>({
  label,
  lines,
  render,
}: {
  label: string
  lines: readonly TimelineLine<T>[]
  render: (row: T) => ReactNode
}) {
  return (
    <div role="listbox" aria-label={label} className={styles.timeline}>
      <VirtualList
        items={lines}
        itemKey={(line) => line.key}
        estimateSize={(line) => (line.kind === 'day' ? 24 : 44)}
        className={styles.scroller}
        render={(line) =>
          line.kind === 'day' ? <div className={styles.day}>{line.label}</div> : render(line.row)
        }
      />
    </div>
  )
}

/** Off for this project, or held by another cide. Versions stay readable under either. */
function StateBanner({
  project,
  status,
  onEnabled,
}: {
  project: ProjectId
  status: LocalHistoryStatus | null
  onEnabled: () => void
}) {
  if (status === null) return null
  if (status.readOnly) {
    return (
      <Banner tone="warn">
        Another cide holds this history, so it can be read here and not restored from.
      </Banner>
    )
  }
  if (status.enabled) return null
  return (
    <Banner
      tone="info"
      action={
        <Button
          size="sm"
          onClick={() => {
            void settingsApi.setProjectLocalHistory(project, true).then(onEnabled)
          }}
        >
          Turn on
        </Button>
      }
    >
      Local History is off for this project. Existing versions can still be restored.
    </Banner>
  )
}

function FileHistory({
  project,
  path,
  active,
  nonce,
  onChanged,
}: {
  project: ProjectId
  path: string
  active: boolean
  nonce: number
  onChanged: () => void
}) {
  const { value: history, error } = useLatest(
    active ? () => localHistory.file(project, path) : null,
    `${path}:${nonce}:${active}`,
  )
  const [held, setHeld] = useState<LocalVersion[]>([])
  useEffect(() => {
    if (history !== null) setHeld(history.versions)
  }, [history])
  const [selected, setSelected] = useState<number | null>(null)
  const [other, setOther] = useState<number | null>(null)
  const [query, setQuery] = useState('')
  const shift = useRef(false)

  /*
   * Content search, debounced because each keystroke would otherwise start a scan of every
   * distinct version. Rust stops the scan a newer query replaced, so the debounce is about not
   * starting work, and `alive` is what keeps a late answer from overwriting a newer one.
   */
  const [found, setFound] = useState<{ matches: ReadonlySet<number>; partial: boolean } | null>(
    null,
  )
  useEffect(() => {
    const needle = query.trim()
    if (needle === '') {
      setFound(null)
      return
    }
    let alive = true
    const timer = setTimeout(() => {
      localHistory.search(project, path, needle).then(
        (result) => {
          if (alive) setFound({ matches: new Set(result.matches), partial: result.partial })
        },
        (reason) => {
          if (alive) notifyFailure(reason)
        },
      )
    }, 200)
    return () => {
      alive = false
      clearTimeout(timer)
    }
  }, [project, path, query, nonce])

  const versions = useMemo(
    () => (found === null ? held : held.filter((v) => found.matches.has(v.id))),
    [held, found],
  )
  const version = versions.find((v) => v.id === selected) ?? versions[0] ?? null
  const second = versions.find((v) => v.id === other) ?? null
  const lines = useMemo(
    () => withDays(versions, Date.now(), (v) => String(v.id)),
    [versions],
  )
  const menu = useContextMenu({
    label: 'Version',
    items: ({ target }) => {
      const id = Number(target?.closest<HTMLElement>('[data-version-id]')?.dataset.versionId)
      const row = versions.find((v) => v.id === id)
      if (row === undefined) return []
      const refusal = restoreRefusal(row)
      return [
        {
          id: 'restore',
          label: 'Restore This Version',
          ...(refusal === null
            ? {
                run: () => {
                  void restoreVersion(project, path, row.id).then(onChanged)
                },
              }
            : { disabledReason: refusal }),
        },
        {
          id: 'compare',
          label: 'Show Difference',
          ...(compareRefusal(row) === null
            ? { run: () => compareVersion(project, path, row.id, timeLabel(row.ts)) }
            : { disabledReason: compareRefusal(row) ?? undefined }),
        },
        {
          id: 'patch',
          label: 'Copy Patch to Current',
          ...(row.kept
            ? { run: () => copyPatch(project, path, row.id, null) }
            : { disabledReason: 'No copy of this version was kept.' }),
        },
      ]
    },
  })

  if (history?.untracked) {
    return (
      <EmptyState icon="undo-2" title="Not kept">
        {history.untracked}
      </EmptyState>
    )
  }
  if (error !== null) {
    return (
      <EmptyState icon="triangle-alert" title="Could not read this history">
        {error}
      </EmptyState>
    )
  }
  if (history === null && versions.length === 0) return <Spinner label="Reading history" />
  const search = (
    <div className={styles.search}>
      <SearchField
        aria-label="Search versions by content"
        placeholder="Search content"
        value={query}
        onChange={(event) => setQuery(event.target.value)}
      />
      {found?.partial === true && <span className={styles.hint}>Older versions not searched.</span>}
    </div>
  )
  if (version === null) {
    return found !== null ? (
      <div className={styles.split}>
        <div className={styles.list}>{search}</div>
        <EmptyState icon="search" title="No version holds this text" />
      </div>
    ) : (
      <EmptyState icon="undo-2" title="No versions yet">
        A version appears here the next time this file changes.
      </EmptyState>
    )
  }
  const refusal = restoreRefusal(version)
  const compare = compareRefusal(version)
  return (
    <div className={styles.split} onContextMenu={menu.onContextMenu}>
      {menu.menu}
      <div className={styles.list} onClickCapture={(event) => (shift.current = event.shiftKey)}>
        {search}
        <Timeline
          label="Versions"
          lines={lines}
          render={(row) => (
            <div data-version-id={row.id} className={styles.rowHost}>
              <Row
                label={versionTitle(row)}
                detail={timeLabel(row.ts)}
                trailing={<Badge>{sourceLabel(row.source)}</Badge>}
                selected={row.id === version.id || row.id === second?.id}
                onSelect={() => {
                  if (shift.current && row.id !== version.id) {
                    setOther(row.id)
                  } else {
                    setSelected(row.id)
                    setOther(null)
                  }
                }}
              />
            </div>
          )}
        />
      </div>
      <div className={styles.detail}>
        <Toolbar label="Version actions">
          <Button
            variant="primary"
            size="sm"
            disabled={refusal !== null}
            title={refusal ?? undefined}
            onClick={() => {
              void restoreVersion(project, path, version.id).then(onChanged)
            }}
          >
            Restore This Version
          </Button>
          <Button
            size="sm"
            disabled={compare !== null}
            title={compare ?? 'Open beside the live file, to restore block by block'}
            onClick={() => {
              void compareVersion(project, path, version.id, timeLabel(version.ts))
            }}
          >
            Show Difference
          </Button>
          <Button
            size="sm"
            disabled={!version.kept}
            onClick={() => copyPatch(project, path, version.id, second?.id ?? null)}
          >
            Copy Patch
          </Button>
          <ToolbarSpacer />
          <span className={styles.hint}>Shift-click a second version to compare the two.</span>
        </Toolbar>
        <div className={styles.diff}>
          <VersionDiff
            project={project}
            path={path}
            version={version}
            other={second}
            nonce={nonce}
            visible={active}
          />
        </div>
      </div>
    </div>
  )
}

function FolderHistory({
  project,
  path,
  active,
  nonce,
  onChanged,
}: {
  project: ProjectId
  path: string | null
  active: boolean
  nonce: number
  onChanged: () => void
}) {
  const [groups, setGroups] = useState<LocalHistoryGroup[]>([])
  const [more, setMore] = useState(false)
  const [loaded, setLoaded] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [selected, setSelected] = useState<number | null>(null)
  const [file, setFile] = useState<number | null>(null)
  const [filter, setFilter] = useState('')
  const [confirm, setConfirm] = useState<ConfirmState | null>(null)

  const load = useCallback(
    (before: number | null) =>
      localHistory.groups(project, path, before, PAGE).then(
        (page) => {
          setLoaded(true)
          setError(null)
          setMore(page.length === PAGE)
          setGroups((old) => (before === null ? page : [...old, ...page]))
        },
        (reason) => setError(String(reason)),
      ),
    [project, path],
  )
  useEffect(() => {
    if (active) void load(null)
  }, [active, nonce, load])

  const group = groups.find((g) => g.group === selected) ?? groups[0] ?? null
  const { value: rows } = useLatest(
    active && group !== null && !group.snapshot
      ? () => localHistory.groupFiles(project, path, group.group)
      : null,
    `${group?.group ?? 'none'}:${nonce}:${active}`,
  )
  const shown = useMemo(() => {
    const needle = filter.trim().toLowerCase()
    const files = rows ?? []
    return needle === '' ? files : files.filter((f) => f.path.toLowerCase().includes(needle))
  }, [rows, filter])
  const row = shown.find((f) => f.id === file) ?? shown[0] ?? null
  const lines = useMemo(
    () => withDays(groups, Date.now(), (g) => String(g.group)),
    [groups],
  )

  if (error !== null) {
    return (
      <EmptyState icon="triangle-alert" title="Could not read this history">
        {error}
      </EmptyState>
    )
  }
  if (!loaded) return <Spinner label="Reading history" />
  if (group === null) {
    return (
      <EmptyState icon="undo-2" title="No versions yet">
        Changes under this folder appear here as they happen.
      </EmptyState>
    )
  }
  const refusal = row === null ? 'Select a file.' : restoreRefusal(row)
  /*
   * Every path at stake, named: `ConfirmDestructive`'s rule, and the reason this is a list and
   * not a count. The plan is a preview and Rust works it out again when this runs, because it
   * ages from the moment it is drawn.
   */
  const reviewFolderRestore = (entry: LocalHistoryGroup) => {
    localHistory.plan(project, path, entry.group).then((plan) => {
      if (plan.write.length === 0 && plan.remove.length === 0) {
        notify('Nothing to restore: the folder already matches this point.', { kind: 'ok' })
        return
      }
      const left =
        plan.left.length > 0
          ? ` ${plan.left.length} file(s) have no version that old and stay as they are.`
          : ''
      setConfirm({
        title: 'Restore to this point?',
        body:
          `${plan.write.length} file(s) will be rewritten or brought back, and ` +
          `${plan.remove.length} that did not exist then moved to the trash.${left} ` +
          'Everything replaced is kept, and a label marks the point to return to.',
        files: [...plan.write, ...plan.remove],
        confirmLabel: 'Restore',
        danger: true,
        run: () => {
          localHistory
            .restoreTree(
              project,
              path,
              entry.group,
              `Before restoring to ${timeLabel(entry.ts)}`,
            )
            .then((report) => {
              notify(reportLine(report), {
                kind: report.failed.length === 0 ? 'ok' : 'warn',
                ...(report.failed[0]
                  ? { hint: `${report.failed[0].path}: ${report.failed[0].reason}` }
                  : {}),
              })
              onChanged()
            })
            .catch(notifyFailure)
        },
      })
    }, notifyFailure)
  }
  return (
    <div className={styles.split}>
      {confirm !== null && (
        <ConfirmDestructive
          state={confirm}
          virtualizeFiles
          onCancel={() => setConfirm(null)}
          onConfirm={() => {
            confirm.run()
            setConfirm(null)
          }}
        />
      )}
      <div className={styles.list}>
        <Timeline
          label="Changes"
          lines={lines}
          render={(entry) => (
            <Row
              label={groupTitle(entry)}
              detail={timeLabel(entry.ts)}
              trailing={<Badge>{sourceLabel(entry.source)}</Badge>}
              selected={entry.group === group.group}
              onSelect={() => {
                setSelected(entry.group)
                setFile(null)
              }}
            />
          )}
        />
        {more && (
          <Button
            size="sm"
            onClick={() => {
              void load(groups.at(-1)?.group ?? null)
            }}
          >
            Load older
          </Button>
        )}
      </div>
      <div className={styles.detail}>
        {group.snapshot ? (
          <EmptyState icon="undo-2" title="First snapshot">
            The state of every file when Local History first scanned this project. Open a file's
            history to restore from it.
          </EmptyState>
        ) : group.label !== null ? (
          <EmptyState icon="circle-dot" title={group.label}>
            A label you put on this moment.
          </EmptyState>
        ) : (
          <>
            <Toolbar label="Changed files">
              <SearchField
                aria-label="Filter files"
                placeholder="Filter files"
                value={filter}
                onChange={(event) => setFilter(event.target.value)}
              />
              <ToolbarSpacer />
              <Button
                size="sm"
                title="Put every file here back to how it was after this change"
                onClick={() => reviewFolderRestore(group)}
              >
                Restore All to This Point
              </Button>
              <Button
                variant="primary"
                size="sm"
                disabled={refusal !== null}
                title={refusal ?? undefined}
                onClick={() => {
                  if (row !== null) void restoreVersion(project, row.path, row.id).then(onChanged)
                }}
              >
                Restore This File
              </Button>
            </Toolbar>
            <div className={styles.files} role="listbox" aria-label="Files in this change">
              {shown.map((entry) => (
                <Row
                  key={entry.id}
                  label={entry.path}
                  detail={kindLabel(entry.kind)}
                  selected={entry.id === row?.id}
                  onSelect={() => setFile(entry.id)}
                />
              ))}
            </div>
            {row !== null && (
              <div className={styles.diff}>
                <VersionDiff
                  project={project}
                  path={row.path}
                  version={row}
                  other={null}
                  nonce={nonce}
                  visible={active}
                />
              </div>
            )}
          </>
        )}
      </div>
    </div>
  )
}

export function LocalHistoryTab({
  project,
  path,
  folder,
  active,
}: {
  project: ProjectId
  path: string | null
  folder: boolean
  active: boolean
}) {
  const [nonce, setNonce] = useState(0)
  const bump = useCallback(() => setNonce((n) => n + 1), [])
  useEffect(() => {
    let stop: (() => void) | null = null
    let cancelled = false
    void events
      .onLocalHistoryChanged((changed) => {
        if (changed === project) bump()
      })
      .then((unlisten) => {
        if (cancelled) unlisten()
        else stop = unlisten
      })
    return () => {
      cancelled = true
      stop?.()
    }
  }, [project, bump])

  const { value: status } = useLatest(
    active ? () => localHistory.status(project) : null,
    `status:${nonce}:${active}`,
  )
  const [held, setHeld] = useState<LocalHistoryStatus | null>(null)
  useEffect(() => {
    if (status !== null) setHeld(status)
  }, [status])

  const title =
    path === null ? 'Local History' : `Local History: ${path.slice(path.lastIndexOf('/') + 1)}`
  return (
    <div className={styles.page} data-audit="localHistoryTab">
      <header className={styles.header}>
        <h2 className={styles.title}>{title}</h2>
        {path !== null && <span className={styles.path}>{path}</span>}
        <span className={styles.spacer} />
        <Button size="sm" onClick={() => requestPutLabel(project)}>
          Put Label
        </Button>
      </header>
      <StateBanner project={project} status={held} onEnabled={bump} />
      {path !== null && !folder ? (
        <FileHistory
          project={project}
          path={path}
          active={active}
          nonce={nonce}
          onChanged={bump}
        />
      ) : (
        <FolderHistory
          project={project}
          path={path}
          active={active}
          nonce={nonce}
          onChanged={bump}
        />
      )}
    </div>
  )
}
