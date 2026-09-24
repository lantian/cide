import { Icon } from '@/icons/Icon'
import { FileIcon } from '@/icons/FileIcon'
import type { IconTheme } from '@/icons/iconFor'
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react'
import styles from './GitLab.module.css'
export interface TreeFile {
  path: string
  label?: string
  discussions?: { resolved: number; unresolved: number }
  /** Local, unpublished draft comments on this file. (M85) */
  drafts?: number
  detail?: string
}
interface Node {
  name: string
  path: string
  children: Map<string, Node>
  file?: TreeFile
}
function compareText(a: string, b: string): number {
  const left = a[Symbol.iterator]()
  const right = b[Symbol.iterator]()
  while (true) {
    const x = left.next()
    const y = right.next()
    if (x.done || y.done) return Number(!x.done) - Number(!y.done)
    const order = x.value.codePointAt(0)! - y.value.codePointAt(0)!
    if (order) return order
  }
}
/** Match cide-fs index::sort_children, including its stable case-sensitive tie-break. */
function compareNodes(a: Node, b: Node): number {
  const folded = (name: string) =>
    Array.from(name, (c) => c.toLowerCase()).join('')
  return (
    Number(b.children.size > 0) - Number(a.children.size > 0) ||
    compareText(folded(a.name), folded(b.name)) ||
    compareText(a.name, b.name)
  )
}
function build(files: readonly TreeFile[]): Node {
  const root: Node = { name: '', path: '', children: new Map() }
  for (const file of files) {
    let at = root
    for (const part of file.path.split('/')) {
      const path = at.path ? `${at.path}/${part}` : part
      let next = at.children.get(part)
      if (!next) {
        next = { name: part, path, children: new Map() }
        at.children.set(part, next)
      }
      at = next
    }
    at.file = file
  }
  return root
}
/**
 * Which folders are open, held by the tree rather than by each row. (M96)
 *
 * It used to be a `useState` per `Entry`, which is the shape that cannot answer *Collapse all*:
 * no one above the rows knew what was open. `base` is what a folder does by default and
 * `toggled` is the set of folders the user flipped against it, so the two bulk actions are
 * "reset `toggled`, set `base`" — and a folder that appears *after* one (typing in the filter
 * re-builds the tree) follows the last bulk action instead of snapping back to the prop.
 */
interface Folding {
  base: boolean
  toggled: ReadonlySet<string>
}
function isOpen(folding: Folding, path: string): boolean {
  return folding.base !== folding.toggled.has(path)
}
function withOpen(folding: Folding, path: string, open: boolean): Folding {
  if (isOpen(folding, path) === open) return folding
  const toggled = new Set(folding.toggled)
  if (toggled.has(path)) toggled.delete(path)
  else toggled.add(path)
  return { base: folding.base, toggled }
}
/** Open every folder above `selected`, answering the same object when nothing changes. */
function revealed(folding: Folding, selected: string | null): Folding {
  if (!selected) return folding
  const parts = selected.split('/')
  let next = folding
  for (let i = 1; i < parts.length; i++)
    next = withOpen(next, parts.slice(0, i).join('/'), true)
  return next
}
function Entry({
  node,
  depth,
  selected,
  onOpen,
  theme,
  folding,
  setOpen,
}: {
  theme: IconTheme
  node: Node
  depth: number
  selected: string | null
  onOpen: (path: string) => void
  folding: Folding
  setOpen: (path: string, open: boolean) => void
}) {
  const folder = node.children.size > 0
  const isSelected = !folder && selected === node.path
  const row = useRef<HTMLButtonElement>(null)
  const expanded = folder && isOpen(folding, node.path)
  const setExpanded = (open: boolean) => setOpen(node.path, open)
  // Run on the row itself so newly expanded ancestors have mounted it before scrolling.
  // Keep keyboard focus in the diff, where the next/previous file shortcuts remain active.
  useLayoutEffect(() => {
    if (isSelected)
      row.current?.scrollIntoView({ block: 'nearest', inline: 'nearest' })
  }, [isSelected])
  return (
    <div role="none">
      <button
        ref={row}
        role="treeitem"
        aria-expanded={folder ? expanded : undefined}
        aria-selected={isSelected}
        className={styles.file}
        style={{ paddingLeft: 8 + depth * 14 }}
        title={node.file?.label ?? node.path}
        onClick={() => (folder ? setExpanded(!expanded) : onOpen(node.path))}
        onKeyDown={(e) => {
          if (folder && e.key === 'ArrowLeft') {
            setExpanded(false)
            e.preventDefault()
          }
          if (folder && e.key === 'ArrowRight') {
            setExpanded(true)
            e.preventDefault()
          }
        }}
      >
        <span className={styles.chevron} aria-hidden>
          {folder && (
            <Icon name={expanded ? 'chevron-down' : 'chevron-right'} size={1} />
          )}
        </span>
        <FileIcon
          row={{ name: node.name, kind: folder ? 'dir' : 'file', expanded }}
          theme={theme}
        />
        <span>{node.name}</span>
        {!!node.file?.drafts && (
          <span
            className={`${styles.threadCounts} ${styles.draftCount}`}
            aria-label={`${node.file.drafts} unpublished drafts`}
            title={`${node.file.drafts} draft ${node.file.drafts === 1 ? 'comment' : 'comments'}, not published`}
          >
            <Icon name="pencil" size={1} /> {node.file.drafts}
          </span>
        )}
        {node.file?.discussions &&
          node.file.discussions.unresolved + node.file.discussions.resolved >
            0 && (
            <span
              className={styles.threadCounts}
              aria-label={`${node.file.discussions.unresolved} unresolved, ${node.file.discussions.resolved} resolved threads`}
              title={`${node.file.discussions.unresolved} unresolved · ${node.file.discussions.resolved} resolved discussions`}
            >
              <span data-resolved="false">
                <Icon name="circle-dot" size={1} />{' '}
                {node.file.discussions.unresolved}
              </span>
              <span data-resolved="true">
                <Icon name="check" size={1} /> {node.file.discussions.resolved}
              </span>
            </span>
          )}
        {node.file?.detail && (
          <span className={styles.muted}>{node.file.detail}</span>
        )}
      </button>
      {folder && expanded && (
        <div role="group">
          {Array.from(node.children.values())
            .sort(compareNodes)
            .map((child) => (
              <Entry
                key={child.path}
                theme={theme}
                node={child}
                depth={depth + 1}
                selected={selected}
                onOpen={onOpen}
                folding={folding}
                setOpen={setOpen}
              />
            ))}
        </div>
      )}
    </div>
  )
}
export function FileTree({
  files,
  selected,
  onOpen,
  label,
  theme = 'dark',
  defaultExpanded = false,
}: {
  theme?: IconTheme
  files: readonly TreeFile[]
  selected: string | null
  onOpen: (path: string) => void
  label: string
  /**
   * Whether a folder starts open. The MR's *changed* files pass `true` — a review is read
   * file by file and a tree of closed folders hides exactly the list the reviewer came for —
   * while *Browse MR source*, the whole repository, stays closed. Read once; the host keys the
   * tree on the mode so switching modes starts from that mode's default.
   */
  defaultExpanded?: boolean
}) {
  const root = useMemo(() => build(files), [files])
  // The initial state already holds the reveal, so a tree opened on a selected file draws it
  // on the first frame instead of one frame later.
  const [folding, setFolding] = useState<Folding>(() =>
    revealed({ base: defaultExpanded, toggled: new Set() }, selected),
  )
  // Only a *new* selection reveals: a rerender with the same one must leave a folder the user
  // closed around it closed.
  useEffect(() => {
    setFolding((f) => revealed(f, selected))
  }, [selected])
  const setOpen = useCallback(
    (path: string, open: boolean) =>
      setFolding((f) => withOpen(f, path, open)),
    [],
  )
  return (
    <>
      <div className={styles.treeActions}>
        <button
          type="button"
          className={styles.treeAction}
          title="Expand all"
          aria-label="Expand all"
          onClick={() => setFolding({ base: true, toggled: new Set() })}
        >
          <Icon name="chevrons-up-down" size={1} />
        </button>
        <button
          type="button"
          className={styles.treeAction}
          title="Collapse all"
          aria-label="Collapse all"
          onClick={() => setFolding({ base: false, toggled: new Set() })}
        >
          <Icon name="chevrons-down-up" size={1} />
        </button>
      </div>
      <FileRows
        root={root}
        label={label}
        theme={theme}
        selected={selected}
        onOpen={onOpen}
        folding={folding}
        setOpen={setOpen}
      />
    </>
  )
}
function FileRows({
  root,
  label,
  theme,
  selected,
  onOpen,
  folding,
  setOpen,
}: {
  root: Node
  label: string
  theme: IconTheme
  selected: string | null
  onOpen: (path: string) => void
  folding: Folding
  setOpen: (path: string, open: boolean) => void
}) {
  return (
    <div
      role="tree"
      aria-label={label}
      onKeyDown={(e) => {
        if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(e.key)) return
        const rows = Array.from(
          e.currentTarget.querySelectorAll<HTMLButtonElement>(
            '[role=treeitem]',
          ),
        )
        const at = rows.indexOf(e.target as HTMLButtonElement)
        const next =
          e.key === 'Home'
            ? 0
            : e.key === 'End'
              ? rows.length - 1
              : at + (e.key === 'ArrowDown' ? 1 : -1)
        rows[Math.max(0, Math.min(rows.length - 1, next))]?.focus()
        e.preventDefault()
      }}
    >
      {Array.from(root.children.values())
        .sort(compareNodes)
        .map((node) => (
          <Entry
            key={node.path}
            theme={theme}
            node={node}
            depth={0}
            selected={selected}
            onOpen={onOpen}
            folding={folding}
            setOpen={setOpen}
          />
        ))}
    </div>
  )
}
