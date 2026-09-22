import { Icon } from '@/icons/Icon'
import { FileIcon } from '@/icons/FileIcon'
import type { IconTheme } from '@/icons/iconFor'
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import styles from './GitLab.module.css'
export interface TreeFile {
  path: string
  label?: string
  discussions?: { resolved: number; unresolved: number }
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
function Entry({
  node,
  depth,
  selected,
  onOpen,
  theme,
}: {
  theme: IconTheme
  node: Node
  depth: number
  selected: string | null
  onOpen: (path: string) => void
}) {
  const folder = node.children.size > 0
  const isSelected = !folder && selected === node.path
  const row = useRef<HTMLButtonElement>(null)
  const containsSelected = folder && !!selected?.startsWith(node.path + '/')
  const [expanded, setExpanded] = useState(containsSelected)
  useEffect(() => {
    if (containsSelected) setExpanded(true)
  }, [containsSelected, selected])
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
}: {
  theme?: IconTheme
  files: readonly TreeFile[]
  selected: string | null
  onOpen: (path: string) => void
  label: string
}) {
  const root = useMemo(() => build(files), [files])
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
          />
        ))}
    </div>
  )
}
