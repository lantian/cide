/**
 * What the Docker panel draws, as rules over values. (M41)
 *
 * **Import-free on purpose.** `ui/scripts/check-docker.mjs` compiles this module standalone with
 * the TypeScript in `node_modules` and executes it under node, which is only possible while it
 * imports nothing — not React, not `@/ipc/client`, not the generated wire types. Everything the
 * rules need arrives as a parameter. The shapes below therefore *restate* the wire types rather
 * than importing them, and `adapt.ts` is the one seam allowed to bridge the two.
 */

/** A daemon's state, flattened from the wire's tagged union. */
export type BoardKind = 'unknown' | 'absent' | 'unusable' | 'ready'

export interface Port {
  readonly private: number
  readonly public?: number | undefined
  readonly protocol: string
  readonly hostIp?: string | undefined
}

export interface Container {
  readonly id: string
  readonly name: string
  readonly image: string
  readonly state: string
  readonly status: string
  readonly health?: string | undefined
  readonly created: bigint
  readonly ports: readonly Port[]
  readonly compose?:
    | {
        readonly project: string
        readonly service: string
        readonly workingDir?: string | undefined
        /**
         * Every file Compose was given, from `com.docker.compose.project.config_files`.
         *
         * Not decoration: a stack brought up from a file whose name is not one of Compose's four
         * defaults cannot be acted on without it. See [`Group.files`].
         */
        readonly configFiles: readonly string[]
      }
    | undefined
}

/**
 * Which thing a row is, as `cide_ipc::docker::InspectTarget` serialises.
 *
 * **Restated structurally**, `detailModel.ts`'s rule and this module's own: `check:docker`
 * compiles this file standalone and runs it under node, which is only possible while it imports
 * nothing. `adapt.ts` is the one seam allowed to bridge to the generated type, and `tsc` checks
 * the two agree wherever a value crosses.
 *
 * Note the asymmetry, which is the daemon's and not cide's: a **volume** is addressed by name and
 * the other three by id. Getting that backwards selects nothing and reports nothing.
 */
export type InspectTarget =
  | { readonly kind: 'container'; readonly id: string }
  | { readonly kind: 'image'; readonly id: string }
  | { readonly kind: 'volume'; readonly name: string }
  | { readonly kind: 'network'; readonly id: string }

export interface Image {
  readonly id: string
  readonly tags: readonly string[]
  readonly created: bigint
  /**
   * `bigint`, and every numeric field here is, because `ts-rs` maps Rust's `i64` to it and the
   * generated types are the contract. Converted to `number` only inside the display helpers
   * below, where the loss is unreachable — an image larger than 2^53 bytes is nine petabytes.
   */
  readonly size: bigint
  readonly containers: bigint
}

export interface Volume {
  readonly name: string
  readonly driver: string
  readonly mountpoint: string
  readonly project?: string | undefined
  /** `undefined` means the daemon did not count — which is not "nothing uses it". */
  readonly inUseBy?: number | undefined
}

export interface Network {
  readonly id: string
  readonly name: string
  readonly driver: string
  readonly scope: string
  readonly project?: string | undefined
  readonly subnets: readonly string[]
}

/** Whether stack buttons can work, and the sentence when they cannot. */
export interface Compose {
  readonly present: boolean
  /** The version when present, the remedy when not. */
  readonly detail: string
}

export interface DockerContext {
  readonly name: string
  readonly description: string
  readonly endpoint: string
  readonly current: boolean
}

export interface Board {
  readonly kind: BoardKind
  /** The sentence for `absent` and `unusable`. Empty otherwise. */
  readonly message: string
  /** The endpoint a failure was against, when there was one. */
  readonly endpoint: string
  readonly context?: string | undefined
  readonly contexts: readonly DockerContext[]
  readonly apiVersion: string
  readonly server: string
  readonly containers: readonly Container[]
  readonly images: readonly Image[]
  readonly volumes: readonly Volume[]
  readonly networks: readonly Network[]
  readonly compose: Compose
}

/**
 * Before anybody has looked.
 *
 * A fourth state beside the wire's three, and the panel needs it: `absent` says *this machine has
 * no Docker*, which is a claim, and drawing it in the frame before the first read answers would
 * tell a user with a running daemon that they have none. `BOARD_UNKNOWN` draws a quiet nothing.
 */
export const BOARD_UNKNOWN: Board = {
  kind: 'unknown',
  message: '',
  endpoint: '',
  contexts: [],
  apiVersion: '',
  server: '',
  containers: [],
  images: [],
  volumes: [],
  networks: [],
  // Before a read, Compose is not *absent* — nobody has asked. The distinction matters because
  // `absent` puts a sentence on screen recommending an install, and doing that in the frame
  // before the first answer would tell a user with Compose installed that they have not.
  compose: { present: false, detail: '' },
}

/**
 * Adopt `next` unless it is a step backwards.
 *
 * `specStore`'s `newerBoard`, and the one drop that matters is the same: **a real board must
 * never be replaced by "nobody has asked"**. Returns the *identical object* when it drops, so a
 * selector reading `s.board` sees the same reference and does not re-render — nothing may defeat
 * that by wrapping the result.
 *
 * There is deliberately no revision to compare. `cide://docker-changed` carries none, because
 * `docker_state`'s coalescer runs exactly one read at a time; ordering is closed by construction
 * rather than by a number.
 */
export function newerBoard(current: Board, next: Board): Board {
  if (next.kind === 'unknown' && current.kind !== 'unknown') return current
  return next
}

/**
 * Whether a *state* means the container is doing something.
 *
 * Split from [`isRunning`] because two callers ask the question about different things: the badge
 * counts containers, and the panel's terminal button asks about a state it already has in hand.
 */
export function isRunningState(state: string): boolean {
  // `restarting` counts. A container in a crash loop is emphatically not stopped, and a badge
  // that ignored it would read zero on a machine that is busy failing.
  return state === 'running' || state === 'restarting'
}

/** Whether a container is doing anything — the badge count, and what the Stop button needs. */
export function isRunning(container: Container): boolean {
  return isRunningState(container.state)
}

/**
 * How many containers the rail badge should show, or `null` when nobody has looked.
 *
 * `null` and `0` must never draw alike — the rail's rule. `null` means no read has answered;
 * `0` means a live daemon with nothing running, which is a fact worth not decorating.
 */
export function runningCount(board: Board): number | null {
  if (board.kind !== 'ready') return null
  return board.containers.filter(isRunning).length
}

/** Which lifecycle verbs apply to a container in this state. */
export type Action = 'start' | 'stop' | 'restart' | 'pause' | 'unpause' | 'kill' | 'remove'

/**
 * The buttons a row offers, in the order they are drawn.
 *
 * # Why this is a function of the state and not a fixed set
 *
 * A Start button on a running container and a Stop on an exited one are both requests the daemon
 * refuses — with a sentence the user then has to translate into "that button never applied". The
 * daemon is the authority on whether an action *succeeds*; this decides what is worth offering,
 * which is a different question and belongs here where it can be tested.
 *
 * `remove` is offered in every state, including `running`, because `cide_docker::api::act` forces
 * — the confirm is the gate, not the button's absence.
 */
export function actionsFor(state: string): readonly Action[] {
  switch (state) {
    case 'running':
      return ['stop', 'restart', 'pause', 'kill', 'remove']
    case 'paused':
      return ['unpause', 'stop', 'kill', 'remove']
    case 'restarting':
      // No Start: it is already trying. Stop is the thing somebody reaching for this row wants,
      // and Kill is what they reach for when Stop does not take.
      return ['stop', 'kill', 'remove']
    case 'created':
    case 'exited':
    case 'dead':
      return ['start', 'remove']
    case 'removing':
      // It is going away. Every verb here would race the daemon.
      return []
    default:
      // A state cide has not heard of — Docker adds them. Offering the two that are meaningful
      // in every state cide *does* know beats offering nothing, and the daemon still refuses
      // anything that does not apply.
      return ['start', 'stop', 'remove']
  }
}

/** Whether an action destroys something and must be confirmed first. */
export function isDestructive(action: Action): boolean {
  return action === 'remove'
}

/** A row's tone, which is the only colour a state carries. */
export type Tone = 'running' | 'warning' | 'stopped' | 'neutral'

export function toneFor(container: Container): Tone {
  if (container.health === 'unhealthy') return 'warning'
  if (container.state === 'restarting') return 'warning'
  if (container.state === 'running') return 'running'
  if (container.state === 'dead') return 'warning'
  if (container.state === 'exited' || container.state === 'created' || container.state === 'paused')
    return 'stopped'
  return 'neutral'
}

/** One group of rows under a heading. */
export interface Group {
  readonly key: string
  /** The compose project's name, or [`LOOSE_LABEL`] for the containers that are in no stack. */
  readonly label: string
  /**
   * Whether this is a real compose stack, and therefore whether `up`/`down` can mean anything.
   *
   * Split from "has a label" because the two stopped being the same thing. The ungrouped rows
   * used to draw no heading at all — which was right about stack actions and wrong the moment
   * groups became collapsible, because a group with no heading has nothing to click to open it
   * again. So everything gets a heading and this decides which ones get buttons.
   */
  readonly stack: boolean
  readonly containers: readonly Container[]
  /**
   * Where the stack was brought up from, for `docker compose -f … up`.
   *
   * Taken from **any** member that carries the label rather than from the first: a stack whose
   * first container was recreated without labels would otherwise lose its directory and its
   * buttons, while its siblings still knew.
   */
  readonly workingDir?: string | undefined
  /**
   * The config files this stack was brought up with, in order.
   *
   * # Why an empty list is not good enough
   *
   * It was one, hardcoded, until M53. Compose then received no `-f` and fell back to searching the
   * working directory for a default-named file — which works by accident for `compose.yaml` and
   * `docker-compose.yml`, and answers *"no configuration file provided: not found"* for every
   * other name. A stack from `docker.compose.yaml` could not be restarted or recreated at all.
   *
   * Order matters as much as presence: a stack brought up with an override file must be acted on
   * with the same set, or Compose resolves a different service list — `cide_docker::compose::argv`
   * carries that half.
   *
   * Taken from **any** member that carries the label rather than from the first, exactly as
   * [`Self.workingDir`] is and for the same reason: one container recreated without labels must
   * not cost the whole stack its actions while its siblings still know.
   */
  readonly files: readonly string[]
}

/**
 * Which groups start open.
 *
 * **A stack with something running is open; everything else is shut.** That is the rule, and it
 * is a claim about what a panel is for: a machine with eight stacks on it has one or two the user
 * is working in, and the rest are last week's. Opening all of them buries the live ones under a
 * list that has to be scrolled past every time.
 *
 * The ungrouped rows (`label === ''`) follow the same rule rather than getting an exception —
 * they are loose containers, and a machine where none of them is running has nothing there worth
 * the space either.
 *
 * Returned as the *initial* set rather than applied on every read: once a user has opened a
 * group, a container starting or stopping inside it must not close it under them.
 */
export function initiallyOpen(groups: readonly Group[]): readonly string[] {
  return groups.filter((group) => group.containers.some(isRunning)).map((group) => group.key)
}

/**
 * Whether a group is drawn open, given what the user has changed.
 *
 * `toggled` holds only the keys whose state differs from the default, which is what makes the
 * default able to change underneath it: a stack whose last container stops keeps whatever the
 * user last chose, and one that appears for the first time gets the rule above rather than a
 * remembered answer about a different stack with the same name.
 */
export function isOpen(group: Group, defaults: readonly string[], toggled: readonly string[]): boolean {
  return openFor(group.key, defaults, toggled)
}

/**
 * The same rule, by key, for the three headings that are not compose stacks.
 *
 * Images, Volumes and Networks were plain labels until M49 — not collapsible at all, which is
 * what the container groups had already been fixed for. They fold now, through **this** function
 * rather than a second mechanism: one `toggled` set, one meaning for "the user changed this from
 * the default", and one place for the next reader to look.
 *
 * [`isOpen`] is kept as the `Group` spelling because that is what the panel and `check:docker`
 * already call, and because a group always has a key while a section only has a name.
 */
export function openFor(
  key: string,
  defaults: readonly string[],
  toggled: readonly string[],
): boolean {
  const byDefault = defaults.includes(key)
  return toggled.includes(key) ? !byDefault : byDefault
}

/**
 * The three headings below the containers, and what they hold.
 *
 * A table rather than three copies of the same JSX: the count, the key and the label have to
 * agree across the heading, the toggle and the collapsed summary, and three hand-written blocks
 * are three chances for one of them to say a different number than it draws.
 */
export type SectionKey = 'images' | 'volumes' | 'networks'

/** The keys a section toggles under. Distinct from a stack's, which is a project name. */
export const SECTION_KEYS: readonly SectionKey[] = ['images', 'volumes', 'networks']

export const SECTION_LABEL: Record<SectionKey, string> = {
  images: 'Images',
  volumes: 'Volumes',
  networks: 'Networks',
}

/**
 * Which sections start open, which is **none of them**.
 *
 * The same claim [`initiallyOpen`] makes about stacks, applied to the lists that are longest and
 * acted on least: this panel is about containers, and a machine with sixty images buries them
 * under a list that has to be scrolled past every time. IDEA collapses the same three.
 *
 * A function rather than a constant so it reads like `initiallyOpen` beside it, and so a future
 * rule — "open Images if nothing is running", say — has somewhere to go that is not the view.
 */
export function sectionsInitiallyOpen(): readonly string[] {
  return []
}

/**
 * Whether a toggle key names one of the three sections rather than a compose group.
 *
 * Safe to ask by value because the two families **cannot collide**: a group's key is
 * `stack:<project>` or `loose` (see [`groupByCompose`]), never a bare word. That is what lets one
 * `toggled` set carry both, and it is worth stating because the alternative — two sets — is what
 * a reader reaches for first and it would give two answers to "is this heading open".
 *
 * The caller needs it to pick the right *defaults*: a section's are [`sectionsInitiallyOpen`] and
 * a group's are [`initiallyOpen`], and `toggled` is the difference from whichever applies.
 */
export function isSectionKey(key: string): key is SectionKey {
  return (SECTION_KEYS as readonly string[]).includes(key)
}

/** How many rows a section is hiding, so a collapsed heading still says how much. */
export function sectionCount(board: Board, key: SectionKey): number {
  switch (key) {
    case 'images':
      return board.images.length
    case 'volumes':
      return board.volumes.length
    case 'networks':
      return board.networks.length
  }
}

/**
 * What the containers in no compose stack are called.
 *
 * They need a name because they need a heading, and they need a heading because a collapsed
 * group with nothing to click is a group the user cannot get back. Not "Other" alone: this sits
 * under stack names, and a bare adjective reads as a stack called Other.
 */
export const LOOSE_LABEL = 'Other containers'

/**
 * Something the detail pane names that is also a row in the list. (M50)
 *
 * A container's detail says which **image** it runs, which **volumes** it mounts and which
 * **networks** it is on; a network's says which containers are attached. Every one of those is a
 * row somewhere in the left column, and until M50 they were dead text — the user read the name,
 * then went looking for it by hand.
 *
 * Carried as the **text as drawn** rather than as a resolved id, because the adapter that builds a
 * row has the wire's detail and not the board: an image is named by tag, a network by name, and
 * turning either into the id the selection needs is a lookup over rows the adapter cannot see.
 * [`resolveRef`] is that lookup, and it lives here with the board.
 */
export interface DetailRef {
  readonly kind: 'container' | 'image' | 'volume' | 'network'
  /** The tag, name or id exactly as the detail pane draws it. */
  readonly text: string
}

/**
 * The row a reference points at, or `null` when there is no such row.
 *
 * # `null` is an ordinary answer, and the caller must draw plain text for it
 *
 * Half of these legitimately resolve to nothing, and none of them is an error:
 *
 * * a **bind mount** names a host path, not a volume — there is no row and never will be;
 * * an image the container runs may have been **removed** since, or pulled by a digest this
 *   daemon no longer lists under any tag;
 * * a network or container named in one detail may be **gone** by the time the pane is read,
 *   because the board behind it is a snapshot.
 *
 * So this returns `null` rather than a target that selects nothing, and `DetailPane` draws a
 * button only where it resolves. A link that looks like a link and does nothing is the
 * listed-and-inert control `cide_core::commands` makes unrepresentable one layer down.
 *
 * Matching is deliberately generous in one direction only: an image is found by **any** of its
 * tags and then by an id prefix, because `docker inspect` reports whichever tag was used to
 * start the container and the row carries them all. Nothing here matches on a substring.
 */
export function resolveRef(board: Board, ref: DetailRef): InspectTarget | null {
  if (board.kind !== 'ready') return null
  const text = ref.text.trim()
  if (text === '') return null

  switch (ref.kind) {
    case 'container': {
      const row = board.containers.find(
        (container) => container.name === text || container.id === text || container.id.startsWith(text),
      )
      return row === undefined ? null : { kind: 'container', id: row.id }
    }
    case 'image': {
      const byTag = board.images.find((image) => image.tags.includes(text))
      if (byTag !== undefined) return { kind: 'image', id: byTag.id }
      // `sha256:…` as `docker inspect` reports it, and the bare hex a person pastes.
      const bare = text.replace('sha256:', '')
      const byId = board.images.find(
        (image) => image.id === text || image.id.replace('sha256:', '').startsWith(bare),
      )
      return byId === undefined ? null : { kind: 'image', id: byId.id }
    }
    case 'volume': {
      const row = board.volumes.find((volume) => volume.name === text)
      return row === undefined ? null : { kind: 'volume', name: row.name }
    }
    case 'network': {
      const row = board.networks.find((network) => network.name === text || network.id === text)
      return row === undefined ? null : { kind: 'network', id: row.id }
    }
  }
}

/**
 * Which heading a selected row lives under, so following a link can open it.
 *
 * Load-bearing since the three sections started shut: selecting an image that is inside a
 * collapsed *Images* draws no row at all, so there is nothing to scroll to and the click appears
 * to do nothing. `null` for a container, whose heading is a compose group — see [`groupKeyOf`].
 */
export function sectionOf(target: InspectTarget): SectionKey | null {
  switch (target.kind) {
    case 'image':
      return 'images'
    case 'volume':
      return 'volumes'
    case 'network':
      return 'networks'
    case 'container':
      return null
  }
}

/**
 * Which compose group a container is drawn under, so following a link can open that too.
 *
 * The same key [`groupByCompose`] mints, derived the same way rather than re-derived: a second
 * spelling of "which group is this container in" is a second answer, and the one that is wrong
 * opens a heading the row is not under.
 */
export function groupKeyOf(board: Board, target: InspectTarget): string | null {
  if (board.kind !== 'ready' || target.kind !== 'container') return null
  const group = groupByCompose(board.containers).find((candidate) =>
    candidate.containers.some((container) => container.id === target.id),
  )
  return group?.key ?? null
}

/**
 * The keys that must be open for a selected row to be on screen at all.
 *
 * One function so the *view* has one thing to call and cannot open a section and forget the
 * group, which is the half that only shows up when a network detail links to a container inside
 * a collapsed stack.
 */
export function keysToOpenFor(board: Board, target: InspectTarget): readonly string[] {
  const section = sectionOf(target)
  const group = groupKeyOf(board, target)
  return [section, group].filter((key): key is string => key !== null)
}

/**
 * Something the panel can remove, as `cide_ipc::docker::Removable` spells it. (M56)
 *
 * **Restated structurally**, this module's own rule, and `tsc` checks the two agree wherever a
 * value crosses. A *container* is deliberately absent: removing one has its own road through
 * [`Action`], which forces on purpose after a confirmation, while everything here refuses while
 * anything is using it. The Rust enum carries the argument at length.
 */
export type Removable =
  | { readonly kind: 'image'; readonly id: string }
  | { readonly kind: 'volume'; readonly name: string }
  | { readonly kind: 'network'; readonly id: string }

/**
 * The same thing, as a row's [`InspectTarget`] — or `null` when it cannot be removed.
 *
 * One function so no surface has to re-derive "is this removable", and so a container answers
 * `null` in one place rather than four. The rows already hold an `InspectTarget` for selection,
 * which is why the conversion goes this way round.
 */
export function removableFrom(target: InspectTarget): Removable | null {
  switch (target.kind) {
    case 'image':
      return { kind: 'image', id: target.id }
    case 'volume':
      return { kind: 'volume', name: target.name }
    case 'network':
      return { kind: 'network', id: target.id }
    case 'container':
      // Not an oversight and not a gap to fill later: a container is removed through
      // `docker_container_action`, which forces. See `cide_ipc::docker::Removable`.
      return null
  }
}

/**
 * What a removal is called, and what the confirmation says.
 *
 * The sentence names the **refusal** rather than promising success, because refusing is the
 * ordinary outcome: Docker declines while anything is using the thing, and a dialog that said
 * "this cannot be undone" and nothing else would leave the user surprised by the common case
 * instead of the rare one.
 */
export function removalPrompt(target: Removable, name: string): { title: string; body: string } {
  const noun = target.kind
  return {
    title: `Remove ${noun} ${name}?`,
    body:
      `Docker will refuse if anything is still using this ${noun}, and will say what. ` +
      `Otherwise it is removed, and that cannot be undone.`,
  }
}

/** What a stack heading's buttons ask for. */
export type StackAction = 'up' | 'down' | 'restart' | 'recreate'

/**
 * Whether a stack's buttons can do anything, and the sentence when they cannot.
 *
 * `null` is a green light; a string is the reason, and it is shown rather than the buttons being
 * silently missing. `Command::unavailable`'s rule one layer down: this project has paid
 * twenty-four times for a control that is listed and inert.
 */
export function stackBlocked(board: Board): string | null {
  if (board.kind !== 'ready') return 'no daemon'
  if (!board.compose.present) return board.compose.detail
  return null
}

/**
 * Group containers by compose stack, ungrouped last.
 *
 * # Why the ungrouped rows go last and are not a stack called "other"
 *
 * A stack heading is actionable — M43 hangs `up`/`down`/`restart` off it — and a heading over
 * containers that belong to no stack would offer buttons that cannot work. So the ungrouped rows
 * carry no heading at all, and they sort after the stacks because a machine with three stacks and
 * one stray `postgres` should read as three things and a stray, not as a list interrupted.
 *
 * Stacks are sorted by name and rows within a stack by service, so the order does not shuffle
 * when the daemon reports them differently between reads — which it does.
 */
export function groupByCompose(containers: readonly Container[]): readonly Group[] {
  const stacks = new Map<string, Container[]>()
  const loose: Container[] = []
  for (const container of containers) {
    // `== null`, which catches **both** `null` and `undefined`, and it is the only comparison in
    // this file that may be loosened that way.
    //
    // The type says `compose?: ComposeMembership`, so `=== undefined` looks right and is what
    // this shipped with. It was wrong: `#[ts(optional)]` on the Rust side changes the emitted
    // *type* and not what serde writes, so an absent value arrived as `null`, the check was
    // false, and the next line threw `null is not an object` — on the common case, a container
    // with no compose labels. The DTO pairs `skip_serializing_if` now so the wire matches the
    // type; this stays because a type that was wrong once can be wrong again, and a crash here
    // takes the whole panel down through its boundary.
    if (container.compose == null) {
      loose.push(container)
      continue
    }
    const existing = stacks.get(container.compose.project)
    if (existing === undefined) stacks.set(container.compose.project, [container])
    else existing.push(container)
  }

  const groups: Group[] = [...stacks.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([label, members]) => ({
      key: `stack:${label}`,
      label,
      stack: true,
      // `?? undefined` so a `null` from the wire cannot be mistaken for a directory, and
      // `!= null` on the find for the same reason `groupByCompose` uses it above.
      workingDir: members.map((m) => m.compose?.workingDir ?? undefined).find((dir) => dir != null),
      // Any member that has them, `workingDir`'s rule one line up. `?? []` so a group with no
      // labelled member is an empty list rather than `undefined` — the caller passes it straight
      // to Compose, and one shape is easier to be right about than two.
      files: members.map((m) => m.compose?.configFiles).find((list) => list != null && list.length > 0) ?? [],
      containers: members
        .slice()
        .sort((a, b) => (a.compose?.service ?? '').localeCompare(b.compose?.service ?? '')),
    }))

  if (loose.length > 0) {
    groups.push({
      key: 'loose',
      label: LOOSE_LABEL,
      stack: false,
      // No stack, so no config files — and `stackBlocked`/`Group.stack` already withhold every
      // verb from this heading, so nothing ever reads it. Empty rather than absent, because one
      // shape for the field is easier to be right about than two.
      files: [],
      containers: loose.slice().sort((a, b) => a.name.localeCompare(b.name)),
    })
  }
  return groups
}

/** Published ports as one line, deduplicated by the pair that is actually shown. */
export function portSummary(ports: readonly Port[]): string {
  const shown: string[] = []
  for (const port of ports) {
    if (port.public === undefined) continue
    const text = `${port.public}→${port.private}`
    if (!shown.includes(text)) shown.push(text)
  }
  return shown.join(' ')
}

/** A byte count, for an image list where the interesting digit is the first one. */
export function sizeText(bytes: bigint): string {
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let value = Number(bytes)
  let unit = 0
  while (value >= 1024 && unit + 1 < units.length) {
    value /= 1024
    unit += 1
  }
  return unit === 0 ? `${value} B` : `${value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`
}

/** What an image row is called. An untagged image is a dangling layer and says so. */
export function imageLabel(image: Image): string {
  return image.tags.length === 0 ? '<none>' : (image.tags[0] ?? '<none>')
}
