/**
 * A container's filesystem, browsed. (M44)
 *
 * One directory at a time with a breadcrumb above it, and the stylesheet's header argues why that
 * rather than a tree. The short version: every expansion is a round trip *into a container*, cide
 * has no reusable tree component, and a list has no expansion state to keep consistent with a
 * filesystem that is changing underneath it.
 *
 * # Read-only, and the pane says so
 *
 * `PUT /archive` exists and cide does not call it — see `cide_docker::files`. An edit written
 * back is lost the moment the container is recreated, which for anything under compose is the
 * ordinary way it is restarted.
 */
import { useCallback, useEffect, useRef, useState } from 'react'

import { EditorSurface } from '@/editor/EditorSurface'
import { Icon } from '@/icons/Icon'
import { docker as dockerApi } from '@/ipc/client'
import { useContextMenu, type MenuEntry } from '@/menus'
import { errorText } from '@/ipc/errorText'

import styles from './DockerFilesPane.module.css'
import {
  ROW_ACTION_LABEL,
  activate,
  crumbs,
  entryInfo,
  join,
  rowActions,
  sizeText,
  type DetailLine,
  type Entry,
} from './dockerFilesModel'

export interface DockerFilesPaneProps {
  /** The full container id — what every call is addressed by. */
  readonly container: string
  /**
   * Its name. Not read by the pane — the *tab* carries the title — and kept on the props because
   * the tab and the pane are opened from the same call and a prop that exists only in one of them
   * is a prop somebody adds back the first time the pane wants a heading.
   */
  readonly name?: string | undefined
}

interface Opened {
  readonly path: string
  readonly text: string
}

export function DockerFilesPane({ container }: DockerFilesPaneProps) {
  const [dir, setDir] = useState('/')
  const [entries, setEntries] = useState<readonly Entry[] | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [opened, setOpened] = useState<Opened | null>(null)
  /** The entry an Info view is open for, or `null`. */
  const [info, setInfo] = useState<readonly DetailLine[] | null>(null)
  /*
   * The row the menu was opened on. A ref rather than state: it is read once, inside `items`,
   * which `useContextMenu` calls at open time — so a re-render between the right-click and the
   * click would be a render nobody needed.
   */
  const menuEntry = useRef<Entry | null>(null)

  useEffect(() => {
    let disposed = false
    setEntries(null)
    setNotice(null)
    dockerApi
      .listFiles(container, dir)
      .then((listing) => {
        if (disposed) return
        if (listing.kind === 'ready') {
          // `bigint` on the wire (Rust's `i64`), a `number` here. The conversion is exact well
          // past any file a container holds — `Number.MAX_SAFE_INTEGER` is nine petabytes — and
          // `undefined` is carried through untouched, because it means the daemon reported no
          // size and must never become `0`.
          setEntries(
            listing.entries.map((entry) => ({
              name: entry.name,
              directory: entry.directory,
              size: entry.size === undefined ? undefined : Number(entry.size),
              link: entry.link,
              mode: entry.mode,
              owner: entry.owner,
              modified: entry.modified,
            })),
          )
          return
        }
        // The sentence, never an empty list. A distroless image has no shell and therefore no
        // `ls`, and a browser that drew nothing would be asserting the filesystem is empty.
        setEntries(null)
        setNotice(listing.reason)
      })
      .catch((error: unknown) => {
        if (!disposed) setNotice(errorText(error))
      })
    return () => {
      disposed = true
    }
  }, [container, dir])

  const onActivate = useCallback(
    (entry: Entry) => {
      const next = activate(dir, entry)
      if (next.kind === 'descend') {
        setOpened(null)
        setDir(next.path)
        return
      }
      if (next.kind === 'refuse') {
        setNotice(next.why)
        return
      }
      setNotice(null)
      dockerApi
        .readFile(container, next.path)
        .then((text) => setOpened({ path: next.path, text }))
        .catch((error: unknown) => {
          // A directory, a symlink, a binary, something over the cap — each carries its own
          // sentence naming which, and showing it beats an empty buffer that reads as an empty
          // file.
          setOpened(null)
          setNotice(errorText(error))
        })
    },
    [container, dir],
  )

  const menuItems = useCallback((): readonly MenuEntry[] => {
    const entry = menuEntry.current
    if (entry === null) return []
    const path = join(dir, entry.name)
    return rowActions(entry).map((action): MenuEntry => {
      switch (action) {
        case 'open':
          return { id: action, label: ROW_ACTION_LABEL[action], run: () => onActivate(entry) }
        case 'download':
          return {
            // `Save to…` rather than `Download`: the ellipsis is what says a dialog is coming,
            // and every other destination-picking action in cide is spelled the same way.
            id: action,
            label: ROW_ACTION_LABEL[action],
            run: () => {
              void dockerApi
                .download(container, path, entry.directory)
                .then((written) => {
                  // `null` is a cancelled dialog, which is an answer rather than a failure and
                  // must say nothing at all — the same rule `FsError::NoClipboardImage` follows.
                  if (written !== null) setNotice(`Saved to ${written}`)
                })
                .catch((error: unknown) => setNotice(errorText(error)))
            },
          }
        case 'copyPath':
          return {
            id: action,
            label: ROW_ACTION_LABEL[action],
            run: () => void navigator.clipboard.writeText(path).catch(() => {}),
          }
        case 'info':
          return {
            id: action,
            label: ROW_ACTION_LABEL[action],
            run: () => setInfo(entryInfo(dir, entry)),
          }
      }
    })
  }, [container, dir, onActivate])

  const { openAt, menu } = useContextMenu({ label: 'Container files', items: menuItems })

  return (
    <div className={styles.pane} data-audit="dockerFiles">
      <div className={styles.trail}>
        {crumbs(dir).map((crumb, index, all) => {
          const current = index === all.length - 1
          return (
            <span key={crumb.path}>
              {index > 0 && <span className={styles.sep}>/</span>}
              <button
                type="button"
                className={current ? styles.crumbCurrent : styles.crumb}
                disabled={current}
                onClick={() => {
                  setOpened(null)
                  setDir(crumb.path)
                }}
              >
                {crumb.label}
              </button>
            </span>
          )
        })}
      </div>

      {notice !== null && <div className={styles.notice}>{notice}</div>}

      {entries !== null && (
        <div className={styles.list}>
          {entries.length === 0 && <div className={styles.notice}>This directory is empty.</div>}
          {entries.map((entry) => (
            <button
              key={entry.name}
              type="button"
              className={styles.row}
              // Double-click, `sidebar/FileTree`'s rule: a single click on a row somebody is
              // scanning would fetch a file per row brushed, and each fetch is a round trip into
              // a container.
              onDoubleClick={() => onActivate(entry)}
              onContextMenu={(event) => {
                event.preventDefault()
                menuEntry.current = entry
                openAt(event.clientX, event.clientY, event.currentTarget)
              }}
            >
              <span className={styles.icon}>
                <Icon name={entry.directory ? 'folder' : 'file'} size={1} />
              </span>
              <span className={styles.name}>{entry.name}</span>
              {entry.link !== undefined && (
                <span className={styles.link}>→ {entry.link}</span>
              )}
              <span className={styles.size}>{sizeText(entry.size)}</span>
            </button>
          ))}
        </div>
      )}

      {menu}

      {info !== null && (
        <div className={styles.info}>
          <div className={styles.viewerBar}>
            <span className={styles.viewerPath}>Info</span>
            <button
              type="button"
              className={styles.close}
              title="Close"
              aria-label="Close info"
              onClick={() => setInfo(null)}
            >
              <Icon name="x" size={1} />
            </button>
          </div>
          {info.map((line) => (
            <div key={line.name} className={styles.infoRow}>
              <span className={styles.infoName}>{line.name}</span>
              <span className={styles.infoValue}>{line.value}</span>
            </div>
          ))}
        </div>
      )}

      {opened !== null && (
        <div className={styles.viewer}>
          <div className={styles.viewerBar}>
            <span className={styles.viewerPath}>{opened.path}</span>
            <button
              type="button"
              className={styles.close}
              title="Close this file"
              aria-label="Close this file"
              onClick={() => setOpened(null)}
            >
              <Icon name="x" size={1} />
            </button>
          </div>
          <div className={styles.surface}>
            <EditorSurface
              // The container's own path as the buffer's name, so the language is picked the way
              // it would be for any other file — a `.json` inside a container colours like one.
              path={opened.path}
              // A made-up scheme, `DockerInspectPane`'s rule: it is what guarantees no write path
              // can ever resolve this buffer to something on disk.
              identity={`docker://files/${container}${opened.path}`}
              doc={opened.text}
              readOnly
              highlight="none"
            />
          </div>
        </div>
      )}
    </div>
  )
}

/** Exported for the tab strip's title, which wants the same words the pane uses. */
export function filesTabTitle(name: string): string {
  return `${name} : files`
}
