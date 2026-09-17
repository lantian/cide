/**
 * What the detail pane draws, and the port syntax it edits. (M47)
 *
 * **Import-free on purpose**, `DockerPanel/model.ts`'s reason: `check:docker` compiles this
 * standalone and executes it under node, which is only possible while it imports nothing. The
 * wire types are restated structurally and `detailAdapt.ts` is the one seam allowed to bridge.
 */

export interface DetailPort {
  readonly private: number
  readonly public?: number | undefined
  readonly protocol: string
  readonly hostIp?: string | undefined
}

/**
 * Something this row names that is also a row in the left column. (M50)
 *
 * Structural, and it must stay that way — `model.ts` declares the same shape as `DetailRef` and
 * `tsc` checks the two agree wherever a value crosses. Neither file may import the other: both are
 * compiled standalone by `check:docker`.
 *
 * Carried as the **text as drawn** and not as a resolved id, because the adapter that builds a row
 * sees the wire's detail and not the board. `model.resolveRef` is the lookup, and it answers
 * `null` for the several references that legitimately point at no row — a bind mount, a removed
 * image, a container that has gone since the snapshot.
 */
export interface DetailRef {
  readonly kind: 'container' | 'image' | 'volume' | 'network'
  readonly text: string
}

/** One `name: value` line. */
export interface DetailRow {
  readonly name: string
  readonly value: string
  /**
   * A sentence for the row's tooltip, where the label alone cannot carry the meaning. (M58)
   *
   * Rare on purpose. A row that needs explaining is usually a row that is named badly, and the
   * first fix is the name — this exists for the handful where the honest name is still a term of
   * art, and where guessing wrong costs the reader something.
   */
  readonly hint?: string | undefined
  /**
   * What this row points at, when it points anywhere.
   *
   * Absent on the overwhelming majority — an environment variable, a label, a port — and present
   * on the four that name another object: a container's Image, its Mounts (named volumes only)
   * and its Networks, and a network's attached Containers.
   */
  readonly ref?: DetailRef | undefined
}

/** A titled block of rows — Environment, Labels, Mounts, Networks. */
/**
 * What a volume's size row says while it is being worked out, and afterwards. (M57)
 *
 * Three states and not two, because "not measured" and "empty" are different claims and a volume
 * drawn as `0 B` when nobody counted is a lie about somebody's data. The daemon declines to
 * measure often enough for this to matter — a `local` driver on a remote machine, a plugin volume.
 *
 * The *measuring* state is not politeness either: the size comes from `/system/df`, which walks
 * the filesystem and took 8.4 seconds cold on the machine this was built against. A row that was
 * simply absent until it arrived would read as "this volume has no size".
 */
export type VolumeSize =
  | { readonly state: 'measuring' }
  | { readonly state: 'known'; readonly bytes: number }
  | { readonly state: 'unmeasured' }

/**
 * The size row's text, in the three states.
 *
 * Import-free and pure so `check:docker` can drive it: this is the one place that decides a
 * volume drawn as `0 B` means somebody counted.
 */
export function volumeSizeText(size: VolumeSize, bytes: (n: number) => string): string {
  switch (size.state) {
    case 'measuring':
      // Says *why* it is slow. `docker system df` is the command a user would reach for, and
      // naming it is what makes a two-second wait read as a filesystem walk rather than a hang.
      return 'measuring… (this walks the filesystem, like `docker system df`)'
    case 'known':
      return bytes(size.bytes)
    case 'unmeasured':
      return 'the daemon did not measure this volume'
  }
}

/**
 * The size rows an image's detail draws. (M58)
 *
 * # Why there can be two numbers, and why neither is a bug
 *
 * Reported as *"why is image size different in left and right panel?"* — `nginx:alpine` drawn as
 * 98 MB in the list and 28 MB in the detail. Both came from a field called `Size`, and **the
 * daemon means something different by each**:
 *
 * * `GET /images/json` — what the list draws, and what `docker images` prints — reported
 *   `102,437,698`;
 * * `GET /images/{id}/json` reported `29,017,379`.
 *
 * Asking the same daemon for the manifest explains it exactly: the image is a 16-platform index,
 * one platform of which is present locally, and `101,547,206` (that platform **unpacked**) plus
 * `890,492` (its attestation) is the list's number to the byte. The inspect number is the
 * **content** — the compressed blobs as they were downloaded.
 *
 * So the list is "what this occupies on disk" and the detail was "what it cost to pull". Both are
 * worth knowing and only one was labelled.
 *
 * # Why the second row appears only when they differ
 *
 * Because on the **classic** image store they are the same number, and drawing one value twice
 * under two labels would invent a distinction that does not exist there. The rule is therefore
 * about the values rather than about the storage driver, which cide does not have to detect.
 *
 * `onDisk` is `undefined` when the board has no row for this image — it was removed between the
 * click and the answer, say — and then the only honest thing to show is the one number there is.
 */
export function imageSizeRows(
  onDisk: number | undefined,
  contentSize: number,
  bytes: (n: number) => string,
): readonly DetailRow[] {
  if (onDisk === undefined) {
    return [{ name: 'Size', value: bytes(contentSize) }]
  }
  const rows: DetailRow[] = [
    {
      name: 'Size',
      value: bytes(onDisk),
      hint: 'What this image occupies unpacked on disk. The same number `docker images` prints.',
    },
  ]
  if (onDisk !== contentSize) {
    rows.push({
      /*
       * **`Compressed`, not `Download`.**
       *
       * The first cut called it Download and the very next question was *"and what is download
       * size?"* — which is the label failing. It said what the number is *for* and not what it
       * *is*, and it over-claimed besides: under the containerd store these blobs stay in the
       * content store after unpacking, so they are not only a thing you once transferred, they
       * are also on the disk right now.
       *
       * `Compressed` is what the daemon's own manifest calls `Content`, in a word a reader
       * already owns, and the value spells out the rest so nobody has to hover to find out.
       */
      name: 'Compressed',
      value: `${bytes(contentSize)} — the layers as pulled`,
      hint:
        'The layer blobs for this platform in their compressed form: what `docker pull` ' +
        'transfers, and what the content store keeps after unpacking them.',
    })
  }
  return rows
}

export interface DetailGroup {
  readonly label: string
  readonly rows: readonly DetailRow[]
}

export type Detail =
  | {
      readonly kind: 'container'
      readonly id: string
      readonly title: string
      readonly facts: readonly DetailRow[]
      readonly ports: readonly DetailPort[]
      readonly groups: readonly DetailGroup[]
    }
  | {
      readonly kind: 'image'
      readonly id: string
      readonly title: string
      readonly facts: readonly DetailRow[]
      readonly groups: readonly DetailGroup[]
      /**
       * What `GET /images/{id}/json` reports as `Size`. (M58)
       *
       * **Not the number the list shows**, and the difference is not cide's: under the containerd
       * image store these two endpoints measure different things. See [`imageSizeRows`].
       */
      readonly contentSize: number
    }
  | {
      readonly kind: 'volume' | 'network'
      readonly id: string
      readonly title: string
      readonly facts: readonly DetailRow[]
      readonly groups: readonly DetailGroup[]
    }
  | { readonly kind: 'missing'; readonly reason: string }

/**
 * Ports as the text a person edits — `docker run`'s own `-p` syntax, one per line.
 *
 * Not a grid of number inputs. This is the spelling every user of Docker already knows, and a
 * grid would need add/remove rows, reordering and per-field validation to express what one line
 * expresses exactly. The round trip is pinned by a test: what this writes, [`parsePorts`] reads.
 */
export function portsText(ports: readonly DetailPort[]): string {
  return ports
    .map((port) => {
      const proto = port.protocol === 'tcp' ? '' : `/${port.protocol}`
      // An unpublished port is written as the container port alone, which is what `docker run`
      // means by `--expose` — and is distinguishable from `0:80`, which asks for a random one.
      if (port.public === undefined) return `${port.private}${proto}`
      const host = port.hostIp === undefined ? '' : `${port.hostIp}:`
      return `${host}${port.public}:${port.private}${proto}`
    })
    .join('\n')
}

export interface ParsedPorts {
  readonly ports: readonly DetailPort[]
  /** The first thing wrong, in words, or `null`. */
  readonly problem: string | null
}

/**
 * Read the port syntax back.
 *
 * Accepts what `docker run -p` accepts, minus ranges: `80`, `8080:80`, `127.0.0.1:8080:80`, and
 * any of those with `/udp`. Ranges (`8000-8010:8000-8010`) are refused **by name** rather than
 * silently mis-parsed — a form that quietly published one port of ten would be worse than one
 * that says it cannot.
 *
 * The first problem stops the parse, because a partial list applied to a recreate would publish
 * some ports and drop others on a container that has already been destroyed.
 */
export function parsePorts(text: string): ParsedPorts {
  const ports: DetailPort[] = []
  for (const raw of text.split('\n')) {
    const line = raw.trim()
    if (line === '') continue

    let body = line
    let protocol = 'tcp'
    const slash = body.lastIndexOf('/')
    if (slash >= 0) {
      protocol = body.slice(slash + 1).toLowerCase()
      body = body.slice(0, slash)
      if (protocol !== 'tcp' && protocol !== 'udp' && protocol !== 'sctp') {
        return { ports: [], problem: `\`${protocol}\` is not a protocol Docker publishes.` }
      }
    }
    if (body.includes('-')) {
      return {
        ports: [],
        problem: `\`${line}\` looks like a port range, which cide does not publish. Write each port on its own line.`,
      }
    }

    const parts = body.split(':')
    if (parts.length > 3) {
      return { ports: [], problem: `\`${line}\` has too many colons to be a port mapping.` }
    }

    const numbers = parts.slice(-2).map((p) => Number(p))
    const hostIp = parts.length === 3 ? parts[0] : undefined
    if (parts.length === 1) {
      const only = Number(parts[0])
      if (!isPort(only)) return { ports: [], problem: badPort(parts[0] ?? '') }
      ports.push({ private: only, protocol })
      continue
    }
    const [pub, priv] = numbers as [number, number]
    if (!isPort(pub)) return { ports: [], problem: badPort(String(parts[parts.length - 2])) }
    if (!isPort(priv)) return { ports: [], problem: badPort(String(parts[parts.length - 1])) }
    ports.push({
      private: priv,
      public: pub,
      protocol,
      ...(hostIp !== undefined && hostIp !== '' ? { hostIp } : {}),
    })
  }
  return { ports, problem: null }
}

function isPort(value: number): boolean {
  return Number.isInteger(value) && value > 0 && value <= 65535
}

function badPort(text: string): string {
  return `\`${text}\` is not a port number between 1 and 65535.`
}
