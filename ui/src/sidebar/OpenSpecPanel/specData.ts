/** Shared reads and content generations. Summary equality never decides content freshness. */
import { create } from 'zustand'
import { spec, type ProjectId, type ChangeName } from '@/ipc/client'
import type { SpecInvalidation, SpecSnapshot } from '@/ipc/generated'

export const useSpecData = create<{ epochs: Readonly<Record<string, number>> }>(() => ({ epochs: {} }))
let clock = 0
const prefix = (project: ProjectId) => `${project}\0`
const normalize = (path: string) => path.replaceAll('\\', '/').replace(/\/$/, '')

export function resourceRevision(epochs: Readonly<Record<string, number>>, project: ProjectId, path: string, detail = false): number {
  const base = prefix(project)
  let latest = epochs[base] ?? 0
  let current = normalize(path)
  if (detail) {
    const cwd = current.split('/openspec/')[0] ?? current
    latest = Math.max(latest, epochs[`${base}validation:${cwd}`] ?? 0)
  }
  while (current !== '') {
    latest = Math.max(latest, epochs[base + current] ?? 0)
    const slash = current.lastIndexOf('/')
    if (slash < 0) break
    current = current.slice(0, slash)
  }
  return latest
}

export function invalidateSpecData(project: ProjectId, snapshot: SpecSnapshot, invalidated: SpecInvalidation): void {
  const epochs = { ...useSpecData.getState().epochs }
  const base = prefix(project)
  const revision = ++clock
  if (invalidated.full || snapshot.board.kind !== 'ready') epochs[base] = revision
  else {
    for (const relative of invalidated.paths) {
      const path = normalize(`${snapshot.board.root}/${relative}`)
      // Change documents share one content scope; canonical specs keep their individual paths.
      const change = /^(.*\/openspec\/changes\/[^/]+)(?:\/|$)/.exec(path)
      epochs[base + (change?.[1] ?? path)] = revision
      if (path.includes('/openspec/changes/archive')) epochs[`${base}validation:${path.split('/openspec/')[0]}`] = revision
      if (path.includes('/openspec/specs') || path.includes('/openspec/config') || path.includes('/openspec/schemas')) {
        epochs[`${base}validation:${path.split('/openspec/')[0]}`] = revision
      }
    }
  }
  useSpecData.setState({ epochs })
}

type Entry = { revision: number; value?: unknown; pending?: Promise<unknown> }
const caches = new Map<ProjectId, Map<string, Entry>>()

async function read<T>(project: ProjectId, key: string, path: string, detail: boolean, fetch: () => Promise<T>): Promise<T> {
  let cache = caches.get(project)
  if (!cache) { cache = new Map(); caches.set(project, cache) }
  const revision = () => resourceRevision(useSpecData.getState().epochs, project, path, detail)
  let entry = cache.get(key)
  if (entry?.pending) return entry.pending as Promise<T>
  if (entry?.revision === revision() && entry.value !== undefined) {
    cache.delete(key); cache.set(key, entry)
    return entry.value as T
  }
  entry = { revision: revision() }
  cache.set(key, entry)
  const mine = entry
  mine.pending = (async () => {
    try {
      for (let attempt = 0; ; attempt++) {
        const before = revision()
        const value = await fetch()
        if (before !== revision()) {
          if (attempt >= 2) throw new Error('OpenSpec files kept changing during the read. Retry once the current edits finish.')
          continue
        }
        mine.revision = before
        mine.value = value
        return value
      }
    } finally {
      delete mine.pending
      while (cache.size > 64) {
        const oldest = [...cache].find(([candidate, value]) => candidate !== key && !value.pending)
        if (!oldest) break
        cache.delete(oldest[0])
      }
    }
  })()
  return mine.pending as Promise<T>
}

export const changePath = (root: string, change: string) => `${normalize(root)}/openspec/changes/${change}`

export function readChange(project: ProjectId, root: string, change: string, worktree?: string) {
  const path = changePath(worktree ?? root, change)
  return read(project, `change:${path}`, path, true, () => spec.change(project, change as ChangeName, worktree))
}

export function readArtifact(project: ProjectId, path: string, worktree?: string) {
  return read(project, `artifact:${worktree ?? ''}:${path}`, path, false, () => spec.artifact(project, path, worktree))
}

export function retainSpecProjects(projects: readonly ProjectId[]): void {
  const keep = new Set(projects)
  for (const project of caches.keys()) if (!keep.has(project)) caches.delete(project)
  const epochs = useSpecData.getState().epochs
  const next = Object.fromEntries(Object.entries(epochs).filter(([key]) => keep.has(key.split('\0')[0] as ProjectId)))
  if (Object.keys(next).length !== Object.keys(epochs).length) useSpecData.setState({ epochs: next })
}
