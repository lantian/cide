/**
 * The Docker panel's view. (M41)
 *
 * Pure: reads no store, calls no IPC, never reads the clock. `ui/scripts/check-docker-render.mjs`
 * renders it under node through `react-dom/server`, which is the only gate in this project that
 * can see a panel that compiles, mounts and draws nothing.
 *
 * Every handler is optional, and **an absent one means the control is not drawn** rather than
 * drawn dead — `GitPanelProps`' contract, in its own words: the panel cannot act by itself and
 * must not pretend to.
 *
 * The one decision worth repeating from the stylesheet, because it is about content rather than
 * pixels: a container's state is carried by the dot's colour, its `title`, **and** the words on
 * the row's second line. That redundancy is deliberate and must not be tidied away — it is what
 * makes the panel work for somebody who cannot tell the two greens apart.
 */
import { useEffect, useMemo, useRef, useState } from 'react'

import { Icon } from '@/icons/Icon'
import type { IconName } from '@/icons/iconPaths'

import styles from './DockerPanel.module.css'
import {
  actionsFor,
  SECTION_KEYS,
  SECTION_LABEL,
  isSectionKey,
  keysToOpenFor,
  type Removable,
  initiallyOpen,
  isOpen,
  openFor,
  sectionCount,
  sectionsInitiallyOpen,
  isRunningState,
  stackBlocked,
  groupByCompose,
  imageLabel,
  isDestructive,
  portSummary,
  sizeText,
  toneFor,
  type Action,
  type Board,
  type Container,
  type Image,
  type Network,
  type StackAction,
  type Volume,
} from './model'

/** What a row's two pane-opening buttons ask for. */
export type OpenStream = 'exec' | 'logs' | 'files'

export interface DockerPanelProps {
  /** See `DockerPanelHostProps.placement`. Only the title differs. */
  readonly placement?: 'sidebar' | 'bottom'
  readonly board: Board
  /** True while a gesture is in flight; every button is disabled and nothing is re-entered. */
  readonly busy?: boolean
  /** The daemon's own words when the last gesture failed. */
  readonly onRefresh?: (() => void) | undefined
  readonly onAction?: ((container: string, action: Action) => void) | undefined
  /** `undefined` as the endpoint restores cide's own ladder. */
  readonly onUseEndpoint?: ((endpoint: string | undefined) => void) | undefined
  /**
   * Open a terminal into a container, or follow its logs, in a new pane.
   *
   * Absent when there is no project open — a pane lives in a tab, and a tab lives in a project.
   * The panel itself is machine-scoped and is drawn either way, so the buttons simply are not
   * there rather than being drawn dead: `GitPanelProps`' contract, which this file's header
   * already states.
   */
  readonly onOpenStream?: ((container: string, name: string, stream: OpenStream) => void) | undefined
  /**
   * Open a read-only inspect tab. Absent when no project is open — a tab lives in a project.
   */
  readonly onInspect?: ((target: InspectTarget, name: string) => void) | undefined
  /**
   * Bring a stack up, down, or restart it.
   *
   * Absent when `docker compose` is not installed. The heading is still drawn — the stack is
   * read from the daemon and is a real thing — and the sentence explaining why the buttons are
   * missing is drawn beside it rather than left to be guessed at.
   */
  /**
   * Remove an image, a volume or a network. (M56)
   *
   * Absent when the board is not ready, which is what makes the buttons *not drawn* rather than
   * drawn dead — a control that reports "no daemon" after it is pressed is the shape this panel
   * has already paid for twice.
   */
  readonly onRemove?: ((target: Removable, name: string) => void) | undefined
  readonly onStackAction?:
    | ((
        project: string,
        action: StackAction,
        workingDir: string | undefined,
        /**
         * Every config file the stack was brought up with, from its own labels.
         *
         * **Not optional and not droppable.** It was a hardcoded `[]` until M53, so Compose got
         * no `-f` and fell back to searching the working directory for a default-named file —
         * which works by accident for `compose.yaml` and answers *"no configuration file
         * provided: not found"* for every other name.
         */
        files: readonly string[],
      ) => void)
    | undefined
  /** Which row is selected, as its `InspectTarget`. `null` for none. */
  readonly selected?: InspectTarget | null
  /** A single click on a row. Absent makes rows unselectable rather than selectable-and-inert. */
  readonly onSelect?: ((target: InspectTarget) => void) | undefined
  /** The detail pane, already fetched by the host. Absent draws no second column at all. */
  readonly detail?: React.ReactNode
}

/** What an inspect click asks for. Mirrors `cide_ipc::docker::InspectTarget`. */
export type InspectTarget =
  | { readonly kind: 'container'; readonly id: string }
  | { readonly kind: 'image'; readonly id: string }
  | { readonly kind: 'volume'; readonly name: string }
  | { readonly kind: 'network'; readonly id: string }

/** The icon each verb draws, from the closed vendored set. */
const ACTION_ICON: Record<Action, IconName> = {
  start: 'play',
  stop: 'square',
  restart: 'refresh-cw',
  pause: 'pause',
  unpause: 'play',
  kill: 'circle-slash',
  remove: 'trash-2',
}

/** And what it is called, which is the tooltip and the accessible name. */
const ACTION_LABEL: Record<Action, string> = {
  start: 'Start',
  stop: 'Stop',
  restart: 'Restart',
  pause: 'Pause',
  unpause: 'Resume',
  kill: 'Kill',
  remove: 'Remove',
}

/** The stack verbs, in the order they are drawn. */
const STACK_ACTIONS: readonly StackAction[] = ['up', 'recreate', 'restart', 'down']

const STACK_ICON: Record<StackAction, IconName> = {
  up: 'play',
  // `square-plus` and not a second `refresh-cw`: recreate *destroys and rebuilds* the containers
  // and restart does not, and two rows drawn with one mark would read as two spellings of the
  // same button. The label carries the distinction too — see below.
  recreate: 'square-plus',
  restart: 'refresh-cw',
  down: 'square',
}

const STACK_LABEL: Record<StackAction, string> = {
  up: 'Start',
  /*
   * *Recreate*, and the word is doing real work.
   *
   * Docker cannot change a running container, so this is the only verb that answers "I have
   * edited the compose file, apply it" — `up` alone is a no-op on a stack Compose does not
   * consider stale, and it would report success while every container kept its old
   * configuration. `DetailPane`'s own recreate makes the same argument one surface over, and
   * `ComposeAction::Recreate` carries it in Rust.
   */
  recreate: 'Recreate',
  restart: 'Restart',
  down: 'Stop',
}

/**
 * Whether this row is the selected one.
 *
 * Compared on the **id**, not by object identity: `selected` is rebuilt on every render of the
 * host and a reference test would never be true.
 */
function isSelected(selected: InspectTarget | null, target: InspectTarget): boolean {
  if (selected === null || selected.kind !== target.kind) return false
  return 'id' in selected && 'id' in target
    ? selected.id === target.id
    : 'name' in selected && 'name' in target && selected.name === target.name
}

const TONE_CLASS = {
  running: styles.toneRunning,
  warning: styles.toneWarning,
  stopped: styles.toneStopped,
  neutral: styles.toneNeutral,
} as const

export function DockerPanel({
  placement = 'sidebar',
  board,
  busy = false,
  onRefresh,
  onAction,
  onUseEndpoint,
  onOpenStream,
  onInspect,
  onRemove,
  onStackAction,
  selected = null,
  onSelect,
  detail,
}: DockerPanelProps) {
  return (
    <div
      className={placement === 'bottom' ? `${styles.panel} ${styles.docked}` : styles.panel}
      data-audit="dockerPanel"
    >
      <div className={styles.header}>
        {/* No heading in the bottom panel: the tab strip above it already says Docker. */}
        {placement !== 'bottom' && <span className={styles.headerLabel}>Docker</span>}
        {/*
          * The switcher sits in the header row beside Refresh, which is where it was asked for
          * and where it belongs: both are controls over *which daemon this is*, and a line of
          * its own spent 22px saying so twice.
          *
          * Drawn whenever the context store has anything in it — **including on a board that
          * could not connect**. A switcher that disappeared on failure would take away the only
          * control that could undo a switch to a daemon that is down, which is a trap rather
          * than a missing feature.
          */}
        <ConnectionLine board={board} busy={busy} onUseEndpoint={onUseEndpoint} />
        {onRefresh !== undefined && (
          <button
            type="button"
            className={styles.headerAction}
            onClick={onRefresh}
            disabled={busy}
            title="Refresh"
            aria-label="Refresh"
          >
            <Icon name="refresh-cw" size={1} />
          </button>
        )}
      </div>

      {/*
        * List and detail side by side, IDEA's Services layout — the question a row-click asks is
        * about the row that is still on screen, so the list must not be replaced to answer it.
        */}
      <div className={styles.columns}>
        <div className={styles.body}>
          <Body
            board={board}
            busy={busy}
            onRefresh={onRefresh}
            onAction={onAction}
            onOpenStream={onOpenStream}
            onInspect={onInspect}
            onStackAction={onStackAction}
            onRemove={onRemove}
            selected={selected}
            onSelect={onSelect}
          />
        </div>
        {detail}
      </div>
    </div>
  )
}

/**
 * Which daemon this is, with the switcher.
 *
 * # Why the picker lists a "cide decides" row above the contexts
 *
 * Because pinning and following are different things and the panel has to be able to say which
 * it is doing. Selecting a context by name pins cide to that endpoint; the first row restores the
 * ladder, which *follows* `docker context use` made in a terminal. A picker that only listed
 * contexts would make the second state unreachable once anybody had used the first.
 */
function ConnectionLine({
  board,
  busy,
  onUseEndpoint,
}: {
  readonly board: Board
  readonly busy: boolean
  readonly onUseEndpoint?: ((endpoint: string | undefined) => void) | undefined
}) {
  // Nothing to switch between, and nothing to say. `unknown` is included deliberately: before
  // the first read there is no claim to make about which daemon this is.
  if (board.contexts.length === 0) {
    return <span className={styles.connectionSpacer} />
  }

  const current = board.context ?? ''
  if (onUseEndpoint === undefined) {
    return (
      <span className={styles.connection} title={board.endpoint}>
        {board.context ?? board.endpoint}
      </span>
    )
  }
  return (
    <>
      <select
        className={styles.contextPicker}
        value={current}
        disabled={busy}
        title={board.endpoint === '' ? 'Docker context' : board.endpoint}
        aria-label="Docker context"
        onChange={(event) => {
          const name = event.target.value
          if (name === '') {
            onUseEndpoint(undefined)
            return
          }
          const chosen = board.contexts.find((context) => context.name === name)
          if (chosen !== undefined) onUseEndpoint(chosen.endpoint)
        }}
      >
        <option value="">
          {/* The ladder, named as what it does rather than as "default" — Docker already has a
              context called `default` and it is a different thing. */}
          Follow the current context
        </option>
        {board.contexts.map((context) => (
          <option key={context.name} value={context.name}>
            {context.name}
            {context.current ? ' (current)' : ''}
          </option>
        ))}
      </select>
      {board.kind === 'ready' && (
        <span className={styles.apiVersion} title={`${board.server} — API ${board.apiVersion}`}>
          {board.apiVersion}
        </span>
      )}
    </>
  )
}

function Body({
  board,
  busy,
  onRefresh,
  onAction,
  onOpenStream,
  onInspect,
  onRemove,
  onStackAction,
  selected = null,
  onSelect,
}: {
  readonly board: Board
  readonly busy: boolean
  readonly onRefresh?: (() => void) | undefined
  readonly onAction?: ((container: string, action: Action) => void) | undefined
  readonly onOpenStream?:
    | ((container: string, name: string, stream: OpenStream) => void)
    | undefined
  readonly onInspect?: ((target: InspectTarget, name: string) => void) | undefined
  /**
   * Remove an image, a volume or a network. (M56)
   *
   * Absent when the board is not ready, which is what makes the buttons *not drawn* rather than
   * drawn dead — a control that reports "no daemon" after it is pressed is the shape this panel
   * has already paid for twice.
   */
  readonly onRemove?: ((target: Removable, name: string) => void) | undefined
  readonly onStackAction?:
    | ((
        project: string,
        action: StackAction,
        workingDir: string | undefined,
        /**
         * Every config file the stack was brought up with, from its own labels.
         *
         * **Not optional and not droppable.** It was a hardcoded `[]` until M53, so Compose got
         * no `-f` and fell back to searching the working directory for a default-named file —
         * which works by accident for `compose.yaml` and answers *"no configuration file
         * provided: not found"* for every other name.
         */
        files: readonly string[],
      ) => void)
    | undefined
  readonly selected?: InspectTarget | null
  readonly onSelect?: ((target: InspectTarget) => void) | undefined
}) {
  // Nobody has looked yet. Deliberately blank rather than "no Docker": `absent` is a *claim*, and
  // making it in the frame before the first read answers tells a user with a running daemon that
  // they have none.
  if (board.kind === 'unknown') return null

  if (board.kind === 'absent' || board.kind === 'unusable') {
    return (
      <div className={styles.notice}>
        <span>{board.message}</span>
        {board.endpoint !== '' && <span className={styles.noticeEndpoint}>{board.endpoint}</span>}
        {onRefresh !== undefined && (
          <button type="button" className={styles.retry} onClick={onRefresh} disabled={busy}>
            Retry
          </button>
        )}
      </div>
    )
  }

  if (board.containers.length === 0 && board.images.length === 0) {
    return (
      <div className={styles.notice}>
        <span>This daemon has no containers and no images.</span>
      </div>
    )
  }

  const groups = groupByCompose(board.containers)
  const blocked = stackBlocked(board)
  /*
   * Which groups the *rule* opens, recomputed from the board — so a stack that starts running
   * opens, and one that stops does not slam shut under a user who opened it. `toggled` below is
   * the difference from this, not an absolute set; see `isOpen`.
   */
  const defaults = useMemo(() => initiallyOpen(groups), [groups])
  /*
   * Only the keys the user has *changed*, never an absolute open-set. That is what lets the rule
   * above keep applying: a stack that starts running opens on its own, and one the user shut
   * stays shut whatever its containers do.
   *
   * Local state rather than a prop, because it is transient gesture state — `store/workspace.ts`'s
   * header names exactly this as the webview's own. `ExtensionsPanel` holds its filter the same
   * way, and the render check drives the first paint, which is the rule's own answer.
   */
  /*
   * The sections' own defaults — empty, so all three start shut. A constant in practice, held as
   * a call so it reads beside `initiallyOpen` above and so a future rule has somewhere to live
   * that is not this component.
   */
  const sectionDefaults = useMemo(() => sectionsInitiallyOpen(), [])
  const [toggled, setToggled] = useState<readonly string[]>([])
  const onToggle = (key: string) =>
    setToggled((keys) => (keys.includes(key) ? keys.filter((k) => k !== key) : [...keys, key]))

  /*
   * A selection arriving from somewhere else must be **on screen**. (M50)
   *
   * The detail pane's links select a row the user cannot see: an image lives under a heading that
   * starts shut, and a container reached from a network's attached list may be inside a collapsed
   * stack. Selecting it and doing nothing else draws an invisible highlight — a click that
   * appears to do nothing, which is the defect this project keeps finding.
   *
   * So: open whatever has to be open (`keysToOpenFor` answers the section *and* the group, in one
   * call, because opening one and forgetting the other is the half that only shows up on a
   * container inside a folded stack), then scroll the row to it.
   *
   * **`toggled` is the difference from the default**, so "open this key" is not `[...toggled,
   * key]` — for a key that is open by default that would *shut* it. `openFor` is asked first and
   * the toggle is only added when it is actually shut, which is the one line here that is easy
   * to get subtly wrong and impossible to see afterwards.
   */
  /*
   * **Only when the selection actually changes**, which the dependency list cannot express.
   *
   * The first cut depended on `[selected, board, defaults, sectionDefaults]` and re-ran on every
   * render: `groups` is recomputed each time (`groupByCompose(board.containers)` is a plain call),
   * so `defaults` is a fresh array each time, so the effect fired constantly — and re-opened the
   * selected row's heading the instant the user collapsed it. Reported as "I'm not able to
   * collapse the first group, it seems it auto expanding", and that is exactly what it was: a
   * click that shut the group, and an effect that re-opened it on the next render.
   *
   * A ref holding the selection this already ran for is what makes it fire once per *arrival*.
   * The board still has to be read — it is what maps a container to its group — but reading it is
   * not a reason to run.
   */
  const openedFor = useRef<string | null>(null)
  useEffect(() => {
    if (selected === null) {
      openedFor.current = null
      return
    }
    // The selection's identity, not the object: `selected` is rebuilt on every render of the
    // host, so comparing references would be the same bug in a different spelling.
    const id = 'id' in selected ? selected.id : selected.name
    const key = `${selected.kind}:${id}`
    if (openedFor.current === key) return
    openedFor.current = key

    const needed = keysToOpenFor(board, selected)
    setToggled((keys) => {
      const shut = needed.filter(
        (candidate) => !openFor(candidate, isSectionKey(candidate) ? sectionDefaults : defaults, keys),
      )
      return shut.length === 0 ? keys : [...keys, ...shut]
    })
  }, [selected, board, defaults, sectionDefaults])

  /*
   * And then scroll to it, once the open above has rendered.
   *
   * `block: 'nearest'` is what makes a row that is *already* visible not move at all — the
   * selection changes on every click in this list, and `'center'` would jump the list under the
   * pointer every time somebody picked a row by hand. Only a row that is off screen moves, which
   * is exactly the case the detail pane's links create.
   *
   * A DOM query rather than a ref threaded through four row components, scoped to
   * `[data-audit="dockerPanel"]` so it cannot reach another list that marks its rows the same
   * way. Both panels answering — the sidebar's and the bottom window's — is the right outcome
   * rather than a bug: they show the same selection and both should be looking at it.
   *
   * `useEffect` and not `useLayoutEffect`: this component is server-rendered by
   * `check:docker-render`, where a layout effect is a React warning and does nothing useful. One
   * frame before the scroll is imperceptible for a gesture that just crossed the pane.
   */
  useEffect(() => {
    if (selected === null || typeof document === 'undefined') return
    for (const row of document.querySelectorAll<HTMLElement>(
      '[data-audit="dockerPanel"] [data-selected="true"]',
    )) {
      row.scrollIntoView({ block: 'nearest' })
    }
  }, [selected, toggled, board])
  return (
    <>
      {groups.map((group) => {
        const open = isOpen(group, defaults, toggled)
        return (
        <div key={group.key}>
          <div className={styles.groupLabel} title={`compose stack ${group.label}`}>
              {/*
                * The twisty and the name are one button: a heading whose *name* did not toggle
                * would be a 6px target, and every tree in cide toggles on the row.
                */}
              <button
                type="button"
                className={styles.groupToggle}
                aria-expanded={open}
                onClick={() => onToggle(group.key)}
              >
                <Icon name={open ? 'chevron-down' : 'chevron-right'} size={1} />
                <span className={styles.groupName}>{group.label}</span>
                <span className={styles.groupCount}>{group.containers.length}</span>
              </button>
              {group.stack && onStackAction !== undefined && blocked === null ? (
                <span className={styles.groupActions}>
                  {STACK_ACTIONS.map((action) => (
                    <button
                      key={action}
                      type="button"
                      className={styles.action}
                      disabled={busy}
                      title={`${STACK_LABEL[action]} the ${group.label} stack`}
                      aria-label={`${STACK_LABEL[action]} the ${group.label} stack`}
                      onClick={() => onStackAction(group.label, action, group.workingDir, group.files)}
                    >
                      <Icon name={STACK_ICON[action]} size={1} />
                    </button>
                  ))}
                </span>
              ) : (
                /*
                 * The heading is still drawn and the buttons are not — and the *reason* is on
                 * screen rather than left to be guessed at. A stack is read from the daemon's
                 * own labels, so it is a real thing whether or not the CLI plugin that acts on
                 * it exists; a heading with silently missing controls would read as a bug in
                 * cide. `Command::unavailable`'s rule, one layer down.
                 */
                group.stack &&
                blocked !== null &&
                blocked !== 'no daemon' && (
                  <span className={styles.groupBlocked} title={blocked}>
                    actions unavailable
                  </span>
                )
              )}
          </div>
          {open &&
            group.containers.map((container) => (
            <ContainerRow
              key={container.id}
              container={container}
              busy={busy}
              onAction={onAction}
              onOpenStream={onOpenStream}
              onInspect={onInspect}
              selected={selected}
              onSelect={onSelect}
            />
            ))}
        </div>
        )
      })}

      {/*
        * Images, Volumes and Networks — **collapsible, and shut by default**. (M49)
        *
        * They were plain labels: no chevron, no state, nothing to click. The container groups had
        * been made foldable one milestone earlier and these were missed, which is the whole of the
        * report that prompted this.
        *
        * Shut by default for `initiallyOpen`'s reason, applied to the lists that are longest and
        * acted on least — this panel is about containers, and sixty images buries them. The count
        * rides on the heading so a shut section still says how much it is hiding.
        *
        * Driven from `SECTION_KEYS` rather than written out three times: the count, the key and
        * the label have to agree across the heading, the toggle and the rows, and three
        * hand-written blocks are three chances for one of them to disagree with what it draws.
        * They share the *same* `toggled` set as the stacks above — one meaning for "the user
        * changed this", one place to look.
        */}
      {SECTION_KEYS.map((key) => {
        const count = sectionCount(board, key)
        if (count === 0) return null
        const open = openFor(key, sectionDefaults, toggled)
        return (
          <div key={key}>
            <div className={styles.sectionLabel} data-audit="dockerSection">
              <button
                type="button"
                className={styles.groupToggle}
                aria-expanded={open}
                title={`${open ? 'Collapse' : 'Expand'} ${SECTION_LABEL[key]}`}
                onClick={() => onToggle(key)}
              >
                <Icon name={open ? 'chevron-down' : 'chevron-right'} size={1} />
                <span className={styles.groupName}>{SECTION_LABEL[key]}</span>
                <span className={styles.groupCount}>{count}</span>
              </button>
            </div>
            {open && key === 'images' &&
              board.images.map((image) => (
                <ImageRow
                  key={image.id}
                  image={image}
                  onInspect={onInspect}
                  selected={selected}
                  onSelect={onSelect}
                  busy={busy}
                  onRemove={onRemove}
                />
              ))}
            {open && key === 'volumes' &&
              board.volumes.map((volume) => (
                <VolumeRow
                  key={volume.name}
                  volume={volume}
                  onInspect={onInspect}
                  selected={selected}
                  onSelect={onSelect}
                  busy={busy}
                  onRemove={onRemove}
                />
              ))}
            {open && key === 'networks' &&
              board.networks.map((network) => (
                <NetworkRow
                  key={network.id}
                  network={network}
                  onInspect={onInspect}
                  selected={selected}
                  onSelect={onSelect}
                  busy={busy}
                  onRemove={onRemove}
                />
              ))}
          </div>
        )
      })}
    </>
  )
}

/**
 * The Remove button an image, a volume and a network row all carry. (M56)
 *
 * One component rather than three copies, for `STACK_ACTIONS`' reason: the label, the icon, the
 * hover reveal and the destructive tone have to agree across the three, and three hand-written
 * blocks are three chances for one of them to drift.
 *
 * **Absent rather than disabled when the panel cannot remove**, which is `DetailPaneProps`'
 * documented rule and `Command::unavailable`'s one layer down. And absent is genuinely the state
 * here: `onRemove` is only passed when the board is ready.
 *
 * It is *not* disabled when the thing is in use, deliberately. The board knows a count — an
 * image's `containers`, a volume's `inUseBy` — and that count is a snapshot, while the daemon is
 * the authority and answers with a sentence naming exactly what holds it. Greying the button on a
 * stale count would refuse removals that would have worked and would explain nothing; pressing it
 * gets the real answer. See `cide_docker::api::remove`.
 */
function RemoveButton({
  what,
  name,
  busy,
  onRemove,
}: {
  readonly what: Removable
  readonly name: string
  readonly busy: boolean
  readonly onRemove?: ((target: Removable, name: string) => void) | undefined
}) {
  if (onRemove === undefined) return null
  return (
    <span className={styles.actions}>
      <button
        type="button"
        className={`${styles.action} ${styles.actionDanger}`}
        disabled={busy}
        title={`Remove ${what.kind} ${name}`}
        aria-label={`Remove ${what.kind} ${name}`}
        onClick={(event) => {
          // The row beneath is a selection target; without this, removing something also selects
          // it, and the detail pane spends the round trip describing a thing being deleted.
          event.stopPropagation()
          onRemove(what, name)
        }}
      >
        <Icon name="trash-2" size={1} />
      </button>
    </span>
  )
}

function VolumeRow({
  volume,
  onInspect,
  selected = null,
  onSelect,
  busy = false,
  onRemove,
}: {
  readonly volume: Volume
  readonly onInspect?: ((target: InspectTarget, name: string) => void) | undefined
  readonly selected?: InspectTarget | null
  readonly onSelect?: ((target: InspectTarget) => void) | undefined
  readonly busy?: boolean | undefined
  readonly onRemove?: ((target: Removable, name: string) => void) | undefined
}) {
  const target: InspectTarget = { kind: 'volume', name: volume.name }
  const chosen = isSelected(selected, target)
  return (
    <div
      className={chosen ? `${styles.row} ${styles.rowSelected}` : styles.row}
      data-audit="dockerVolume"
      data-selected={chosen ? 'true' : undefined}
      onClick={onSelect === undefined ? undefined : () => onSelect(target)}
    >
      <span className={`${styles.dot} ${styles.toneNeutral}`} aria-hidden="true" />
      <span className={styles.rowText}>
        <span
          className={styles.rowName}
          title={volume.mountpoint}
          onDoubleClick={
            onInspect === undefined
              ? undefined
              : () => onInspect({ kind: 'volume', name: volume.name }, volume.name)
          }
        >
          {volume.name}
        </span>
        <span className={styles.rowDetail}>
          {/*
            * `undefined` means the daemon did not count, and `0` means nothing references it.
            * Only the second is a claim that removing it is safe, so they must not read alike —
            * the same rule the image row's `-1` follows.
            */}
          {volume.inUseBy === undefined
            ? volume.driver
            : `${volume.driver} · ${volume.inUseBy} container(s)`}
        </span>
      </span>
      <RemoveButton
        what={{ kind: 'volume', name: volume.name }}
        name={volume.name}
        busy={busy}
        onRemove={onRemove}
      />
    </div>
  )
}

function NetworkRow({
  network,
  onInspect,
  selected = null,
  onSelect,
  busy = false,
  onRemove,
}: {
  readonly network: Network
  readonly onInspect?: ((target: InspectTarget, name: string) => void) | undefined
  readonly selected?: InspectTarget | null
  readonly onSelect?: ((target: InspectTarget) => void) | undefined
  readonly busy?: boolean | undefined
  readonly onRemove?: ((target: Removable, name: string) => void) | undefined
}) {
  const target: InspectTarget = { kind: 'network', id: network.id }
  const chosen = isSelected(selected, target)
  return (
    <div
      className={chosen ? `${styles.row} ${styles.rowSelected}` : styles.row}
      data-audit="dockerNetwork"
      data-selected={chosen ? 'true' : undefined}
      onClick={onSelect === undefined ? undefined : () => onSelect(target)}
    >
      <span className={`${styles.dot} ${styles.toneNeutral}`} aria-hidden="true" />
      <span className={styles.rowText}>
        <span
          className={styles.rowName}
          title={network.id}
          onDoubleClick={
            onInspect === undefined
              ? undefined
              : () => onInspect({ kind: 'network', id: network.id }, network.name)
          }
        >
          {network.name}
        </span>
        <span className={styles.rowDetail}>
          {/*
            * A driver with no subnets — `host` and `none` both — draws its driver alone. That is
            * a fact about the driver rather than a value cide failed to read, so it must not
            * render as an empty gap after a separator.
            */}
          {network.subnets.length === 0
            ? network.driver
            : `${network.driver} · ${network.subnets.join(' ')}`}
        </span>
      </span>
      <RemoveButton
        what={{ kind: 'network', id: network.id }}
        name={network.name}
        busy={busy}
        onRemove={onRemove}
      />
    </div>
  )
}

function ContainerRow({
  container,
  busy,
  onAction,
  onOpenStream,
  onInspect,
  selected = null,
  onSelect,
}: {
  readonly container: Container
  readonly busy: boolean
  readonly onAction?: ((container: string, action: Action) => void) | undefined
  readonly onOpenStream?:
    | ((container: string, name: string, stream: OpenStream) => void)
    | undefined
  readonly onInspect?: ((target: InspectTarget, name: string) => void) | undefined
  readonly selected?: InspectTarget | null
  readonly onSelect?: ((target: InspectTarget) => void) | undefined
}) {
  const tone = toneFor(container)
  const target: InspectTarget = { kind: 'container', id: container.id }
  const chosen = isSelected(selected ?? null, target)
  const ports = portSummary(container.ports)
  // The state in words, which is what the dot's colour duplicates on purpose.
  const detail = [container.status, container.health, ports].filter(
    (part) => part !== undefined && part !== '',
  )
  return (
    <div
      className={chosen ? `${styles.row} ${styles.rowSelected}` : styles.row}
      data-audit="dockerContainer"
      data-state={container.state}
      data-selected={chosen ? 'true' : undefined}
      onClick={onSelect === undefined ? undefined : () => onSelect(target)}
    >
      <span
        className={`${styles.dot} ${TONE_CLASS[tone]}`}
        title={container.state}
        aria-hidden="true"
      />
      <span className={styles.rowText}>
        {/*
          * Double-click, not single. A single click on a row in this panel selects nothing and
          * has no other meaning, so it could have opened the tab — and that is exactly what
          * makes it wrong: a list somebody is scanning with the mouse would open a tab per row
          * brushed. Double-click has meant "open this properly" everywhere else in cide since
          * the file tree's click rules landed.
          */}
        <span
          className={styles.rowName}
          title={`${container.name} — ${container.image}`}
          onDoubleClick={
            onInspect === undefined
              ? undefined
              : () => onInspect({ kind: 'container', id: container.id }, container.name)
          }
        >
          {container.compose?.service ?? container.name}
        </span>
        <span className={`${styles.rowDetail} ${styles.rowPorts}`}>{detail.join(' · ')}</span>
      </span>
      {onOpenStream !== undefined && (
        <span className={styles.actions}>
          {/*
            * A terminal, only while the container is running. `docker exec` on a stopped
            * container is refused by the daemon, and a button that always fails is worse than
            * one that is not there — `actionsFor`'s argument, one row over.
            */}
          {isRunningState(container.state) && (
            <button
              type="button"
              className={styles.action}
              disabled={busy}
              title={`Open a terminal in ${container.name}`}
              aria-label={`Open a terminal in ${container.name}`}
              onClick={() => onOpenStream(container.id, container.name, 'exec')}
            >
              <Icon name="square-terminal" size={1} />
            </button>
          )}
          {/*
            * Logs, in every state. A stopped container's logs are the whole reason somebody
            * opens this panel after a crash, and `docker logs` on one works.
            */}
          <button
            type="button"
            className={styles.action}
            disabled={busy}
            title={`Follow the logs of ${container.name}`}
            aria-label={`Follow the logs of ${container.name}`}
            onClick={() => onOpenStream(container.id, container.name, 'logs')}
          >
            <Icon name="scroll-text" size={1} />
          </button>
          {/*
            * The file browser, and only while the container is running — `docker exec` is how a
            * directory is listed (there is no Engine API for it, see `cide_docker::files`), and
            * an exec on a stopped container is refused by the daemon.
            */}
          {isRunningState(container.state) && (
            <button
              type="button"
              className={styles.action}
              disabled={busy}
              title={`Browse the files in ${container.name}`}
              aria-label={`Browse the files in ${container.name}`}
              onClick={() => onOpenStream(container.id, container.name, 'files')}
            >
              <Icon name="folder" size={1} />
            </button>
          )}
        </span>
      )}
      {onAction !== undefined && (
        <span className={styles.actions}>
          {actionsFor(container.state).map((action) => (
            <button
              key={action}
              type="button"
              className={`${styles.action} ${isDestructive(action) ? styles.actionDanger : ''}`}
              disabled={busy}
              title={`${ACTION_LABEL[action]} ${container.name}`}
              aria-label={`${ACTION_LABEL[action]} ${container.name}`}
              onClick={() => onAction(container.id, action)}
            >
              <Icon name={ACTION_ICON[action]} size={1} />
            </button>
          ))}
        </span>
      )}
    </div>
  )
}

function ImageRow({
  image,
  onInspect,
  selected = null,
  onSelect,
  busy = false,
  onRemove,
}: {
  readonly image: Image
  readonly onInspect?: ((target: InspectTarget, name: string) => void) | undefined
  readonly selected?: InspectTarget | null
  readonly onSelect?: ((target: InspectTarget) => void) | undefined
  readonly busy?: boolean | undefined
  readonly onRemove?: ((target: Removable, name: string) => void) | undefined
}) {
  const target: InspectTarget = { kind: 'image', id: image.id }
  const chosen = isSelected(selected, target)
  return (
    <div
      className={chosen ? `${styles.row} ${styles.rowSelected}` : styles.row}
      data-audit="dockerImage"
      data-selected={chosen ? 'true' : undefined}
      onClick={onSelect === undefined ? undefined : () => onSelect(target)}
    >
      <span className={`${styles.dot} ${styles.toneNeutral}`} aria-hidden="true" />
      <span className={styles.rowText}>
        <span
          className={styles.rowName}
          title={image.tags.join('\n') || image.id}
          onDoubleClick={
            onInspect === undefined
              ? undefined
              : () => onInspect({ kind: 'image', id: image.id }, imageLabel(image))
          }
        >
          {imageLabel(image)}
        </span>
        <span className={styles.rowDetail}>
          {/* Docker's `-1` means the daemon did not count, which is not "no containers" — the
              difference decides whether an image is safe to remove. */}
          {image.containers < 0n
            ? 'in use by an unknown number of containers'
            : `${image.containers} container(s)`}
        </span>
      </span>
      <span className={styles.imageSize}>{sizeText(image.size)}</span>
      <RemoveButton
        what={{ kind: 'image', id: image.id }}
        name={imageLabel(image)}
        busy={busy}
        onRemove={onRemove}
      />
    </div>
  )
}
