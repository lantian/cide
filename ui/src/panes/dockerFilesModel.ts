/**
 * The container file browser's rules, as functions over values. (M44)
 *
 * **Import-free on purpose**, `DockerPanel/model.ts`'s reason: `check:docker` compiles this
 * standalone with the TypeScript in `node_modules` and executes it under node, which is only
 * possible while it imports nothing.
 */

export interface Entry {
  readonly name: string
  readonly directory: boolean
  readonly size?: number | undefined
  readonly link?: string | undefined
  readonly mode?: string | undefined
  readonly owner?: string | undefined
  readonly modified?: string | undefined
}

/**
 * Join a directory and a name the way a container's filesystem spells it.
 *
 * **Always `/`.** The container is Linux whatever cide is running on, so nothing here may reach
 * for a platform path API. `cide_docker::files::join` is the same rule in Rust, and both are
 * tested, because the two are used at different ends of the same walk.
 */
export function join(dir: string, name: string): string {
  const base = dir.replace(/\/+$/, '')
  return base === '' ? `/${name}` : `${base}/${name}`
}

/**
 * The directory above this one, or `null` at the root.
 *
 * `null` and not `'/'` at the root: the two would make the breadcrumb's first crumb clickable
 * into itself, which is a control that appears to do nothing.
 */
export function parent(path: string): string | null {
  const base = path.replace(/\/+$/, '')
  if (base === '') return null
  const cut = base.lastIndexOf('/')
  return cut <= 0 ? '/' : base.slice(0, cut)
}

/** One crumb: what it says, and where clicking it goes. */
export interface Crumb {
  readonly label: string
  readonly path: string
}

/**
 * The breadcrumb for a path, root first.
 *
 * The root is always present and is spelled `/` — a trail that started at the first *named*
 * segment would have no way back to the root, which is the one directory every container has.
 */
export function crumbs(path: string): readonly Crumb[] {
  const out: Crumb[] = [{ label: '/', path: '/' }]
  let at = ''
  for (const segment of path.split('/')) {
    if (segment === '') continue
    at = `${at}/${segment}`
    out.push({ label: segment, path: at })
  }
  return out
}

/**
 * A byte count for a list where the interesting digit is the first one.
 *
 * `undefined` is **not** zero and must not read as it: the daemon did not report a size, which
 * happens for a directory and for a device node, and `0 B` would be a claim about an empty file.
 */
export function sizeText(size: number | undefined): string {
  if (size === undefined) return ''
  const units = ['B', 'KB', 'MB', 'GB']
  let value = size
  let unit = 0
  while (value >= 1024 && unit + 1 < units.length) {
    value /= 1024
    unit += 1
  }
  return unit === 0 ? `${value} B` : `${value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`
}

/**
 * What double-clicking a row should do.
 *
 * A **symlink is not followed**, and that is the decision worth stating: `GET /archive` does not
 * follow one either — it returns a link entry with no content — so a browser that descended into
 * one would walk to a path the reader cannot then open. The target is shown on the row instead,
 * and the user can type it. `/bin/sh` on Alpine is a symlink to busybox, so this is the common
 * case rather than an edge one.
 */
export type Activation =
  | { readonly kind: 'descend'; readonly path: string }
  | { readonly kind: 'open'; readonly path: string }
  | { readonly kind: 'refuse'; readonly why: string }

export function activate(dir: string, entry: Entry): Activation {
  const path = join(dir, entry.name)
  if (entry.link !== undefined) {
    return {
      kind: 'refuse',
      why: `${entry.name} is a symlink to ${entry.link}, and the daemon does not follow one.`,
    }
  }
  return entry.directory ? { kind: 'descend', path } : { kind: 'open', path }
}

/** What a right-click on a row offers. */
export type RowAction = 'open' | 'download' | 'copyPath' | 'info'

/**
 * The actions a row offers, in the order they are drawn.
 *
 * A **symlink offers no open**, for [`activate`]'s reason: `GET /archive` does not follow one, so
 * an open would refuse and a download would hand back an empty link entry. It keeps *Copy path*
 * and *Info*, which are the two that still mean something — and Info is where its target is
 * shown, which is what somebody right-clicking a symlink actually wants.
 *
 * A **directory offers a download**, unlike an open: it comes out as a tar, which is what
 * `docker cp` gives you and the only thing the API can produce for one.
 */
export function rowActions(entry: Entry): readonly RowAction[] {
  if (entry.link !== undefined) return ['copyPath', 'info']
  return ['open', 'download', 'copyPath', 'info']
}

/** What each action is called. */
export const ROW_ACTION_LABEL: Record<RowAction, string> = {
  open: 'Open',
  download: 'Save to…',
  copyPath: 'Copy path',
  info: 'Info',
}

/**
 * The lines an Info view shows for one entry.
 *
 * Built from the listing rather than from a second round trip: `ls -lAp` already reported the
 * size, the kind and the link target, and a `HEAD /archive` per right-click would be a request
 * per menu open for facts that are already on screen.
 */
export function entryInfo(dir: string, entry: Entry): readonly DetailLine[] {
  const lines: DetailLine[] = [
    { name: 'Name', value: entry.name },
    { name: 'Path', value: join(dir, entry.name) },
    { name: 'Kind', value: entry.link !== undefined ? 'symlink' : entry.directory ? 'directory' : 'file' },
  ]
  if (entry.link !== undefined) lines.push({ name: 'Target', value: entry.link })
  // `undefined` is not zero: the daemon reported no size, which happens for a directory and a
  // device node, and `0 B` would be a claim about an empty file.
  if (entry.size !== undefined) lines.push({ name: 'Size', value: sizeText(entry.size) })
  // Each of these is `undefined` on a line the parser could not read all of, and a missing row
  // is the honest rendering: `ls`'s long format is not a standard, and a blank `Mode` would be a
  // claim that the file has none.
  if (entry.mode !== undefined) lines.push({ name: 'Mode', value: entry.mode })
  if (entry.owner !== undefined) lines.push({ name: 'Owner', value: entry.owner })
  if (entry.modified !== undefined) lines.push({ name: 'Modified', value: entry.modified })
  return lines
}

export interface DetailLine {
  readonly name: string
  readonly value: string
}
