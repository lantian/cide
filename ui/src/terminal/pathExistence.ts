/** Index fast path plus a short-lived disk fallback for ignored and outside-project paths. */
export type PathKind = 'file' | 'dir' | 'absent'
export const DISK_TTL_MS = 3_000
const CACHE_MAX = 4096
const PROBE_MAX = 128

export class PathExistence {
  private readonly index = new Map<string, PathKind>()
  private readonly disk = new Map<string, { at: number; kind: PathKind }>()

  constructor(
    private readonly indexed: (project: string, paths: string[]) => Promise<PathKind[]>,
    private readonly stat: (paths: string[]) => Promise<PathKind[]>,
    private readonly now: () => number = Date.now,
  ) {}

  invalidate(paths: readonly string[]): void {
    for (const path of paths) {
      this.index.delete(path)
      this.disk.delete(path)
    }
  }

  private diskKnown(path: string): PathKind | undefined {
    const entry = this.disk.get(path)
    if (!entry) return undefined
    if (this.now() - entry.at >= DISK_TTL_MS) {
      this.disk.delete(path)
      return undefined
    }
    return entry.kind
  }

  kind(path: string): PathKind | undefined {
    return this.diskKnown(path) ?? this.index.get(path)
  }

  private remember<T>(cache: Map<string, T>, path: string, value: T): void {
    cache.delete(path)
    cache.set(path, value)
    while (cache.size > CACHE_MAX) {
      const first = cache.keys().next().value
      if (first === undefined) break
      cache.delete(first)
    }
  }

  private async probe(
    paths: string[], query: (paths: string[]) => Promise<PathKind[]>, disk: boolean,
  ): Promise<void> {
    for (let i = 0; i < paths.length; i += PROBE_MAX) {
      const batch = paths.slice(i, i + PROBE_MAX)
      let answers: PathKind[]
      try {
        answers = await query(batch)
      } catch {
        // Failed requests must remain retryable, including when the index is not ready yet.
        continue
      }
      batch.forEach((path, n) => {
        const kind = answers[n]
        if (kind === undefined) return
        if (disk) this.remember(this.disk, path, { at: this.now(), kind })
        else this.remember(this.index, path, kind)
      })
    }
  }

  async lookup(
    project: string, inside: readonly string[], outside: readonly string[], refresh = false,
  ): Promise<void> {
    const inProject = [...new Set(inside)]
    await this.probe(inProject.filter((path) => !this.index.has(path)),
      (paths) => this.indexed(project, paths), false)
    const disk = [...new Set([
      ...inProject.filter((path) => refresh || !this.index.has(path)
        || this.index.get(path) === 'absent'),
      ...outside,
    ])].filter((path) => refresh || this.diskKnown(path) === undefined)
    await this.probe(disk, this.stat, true)
  }
}
