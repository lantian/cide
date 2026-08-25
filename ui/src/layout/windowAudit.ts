/**
 * M5's acceptance check, made runnable.
 *
 * The milestone states it as two sentences: *detach a Claude pane mid-response — not one
 * byte dropped, `/proc` shows the same pid, transcript unbroken including alt-screen state;
 * re-dock restores its tree position*, and *flip window mode with 3 projects and 6 live
 * sessions: zero deaths*.
 *
 * Half of that cannot be seen from a webview. `/proc` is not readable here, and a detached
 * pane's terminal lives in a different JavaScript realm — a second webview with its own
 * xterm — so nothing in this window can assert on the bytes the new window painted. The
 * check is therefore split. This file asserts what the frontend genuinely observes: that the
 * set of session ids is identical either side of every detach, re-dock and mode flip, that
 * no registered session's child has exited, that the pane comes back out of the project's
 * `detached` map and into *the split it left* — same nesting, same divider ids, same ratios,
 * which is the milestone's "restores its tree position" taken literally — and that a flip
 * moves no project, tab or pane. It
 * then prints every session id it saw, one per line behind a fixed marker, so a shell script
 * can take the other half: a Claude session's id is the value passed to `claude
 * --session-id`, so it appears verbatim in `/proc/<pid>/cmdline`, and the pid either side of
 * the run can be compared without a browser.
 *
 * What it deliberately never asserts: that any window is focused, raised, mapped or on
 * screen. KDE Wayland will not raise a shell-launched window — see ADR 0006 — so a check
 * that waited for one would fail for a reason unrelated to the code under test, which is the
 * worst kind of red.
 *
 * The application is reached through {@link WindowAuditDriver} rather than through `@/ipc`
 * or the store, for the same reason {@link './paneAudit'} does it: the audit must not become
 * a second copy of the wiring it is measuring, and whoever supplies the driver decides what
 * a detach really means.
 */
import { evictionCounts, hostFaults, peekHost } from './paneHosts'

export type WindowAuditMode = 'stacked' | 'perProject'

/** One pane of a project, wherever it currently lives. */
export interface WindowAuditPane {
  id: string
  /** The tab whose tree holds this pane, or `null` while it waits in `Project::detached`. */
  tab: string | null
  /** `null` for a pane with no process, such as a diff view. */
  session: string | null
}

/**
 * A project, flattened to what the audit reasons about.
 *
 * `panes` covers both halves of the domain's split — the tabs' trees and the `detached`
 * holding map — because the question asked of a detached pane is precisely which of the two
 * it is in. An adapter is `Object.keys(tab.tree.panes)` per tab plus `project.detached`.
 */
export interface WindowAuditProject {
  id: string
  /** Named in failures: "project 3f2a1b0c (cide) is shown by no window" beats an index. */
  name: string
  /** Tab ids in strip order; `tabs[0]` is the pinned console. */
  tabs: string[]
  panes: WindowAuditPane[]
  /**
   * Each tab's split tree, flattened to one comparable string, keyed by tab id.
   *
   * The milestone says *re-dock restores its tree position*, and `panes` cannot answer that:
   * it is a flat list, so a pane torn out of a 70/30 nested split and put back beside
   * whatever held focus looks identical to one put back where it was. The string therefore
   * has to carry what "position" is made of — the nesting, each divider's id and axis, and
   * its ratio — and it is compared to itself either side of a round trip, never parsed.
   *
   * Built with {@link renderTree}, which is exported so the adapter cannot invent its own
   * notion of "position" and quietly leave the ratio out of it.
   */
  trees: Record<string, string>
}

/**
 * A pane tree, structurally — the audit's own shape, not `LayoutNode` from `@/ipc`.
 *
 * Declared here rather than imported for the same reason the whole file avoids `@/ipc`: the
 * audit must be drivable from a fixture, and a fixture should not have to construct wire
 * types to be asserted over.
 */
export type WindowAuditNode =
  | { leaf: string }
  | { split: string; axis: string; ratio: number; a: WindowAuditNode; b: WindowAuditNode }

/**
 * One tab's tree as a single string, for comparing a tree against itself over time.
 *
 * Every part of what a re-dock has to reinstate is in it and nothing else is: the nesting,
 * which child is `a`, each divider's id and axis, and the ratio. `focused` and `maximized`
 * are deliberately absent — a re-dock is *meant* to move focus to the pane it just put back,
 * so including them would make the exact case fail for doing the right thing.
 *
 * The ratio is fixed to four places because it crosses the wire as an `f32` widened to an
 * `f64`, so `0.73` prints as `0.7300000190734863`; four places still separates any two
 * divider positions a person could distinguish, and 70/30 coming back 50/50 — the regression
 * this exists to catch — differs in the first.
 *
 * Ids are written in full and shortened only when a failure prints them: an eight-character
 * prefix is what the rest of this file shows a human, but two dividers are the same divider
 * or they are not, and an equality test should not be the place that decides.
 */
export function renderTree(node: WindowAuditNode): string {
  if ('leaf' in node) return node.leaf
  return `${node.split}:${node.axis}@${node.ratio.toFixed(4)}(${renderTree(node.a)},${renderTree(node.b)})`
}

/** A rendered tree with every uuid cut to the prefix the rest of the report uses. */
const shortTree = (rendered: string) =>
  rendered.replace(/[0-9a-f]{8}-[0-9a-f-]{27}/g, (id) => short(id))

/** A window as the *workspace* records it. Nothing here says whether it is on screen. */
export interface WindowAuditWindow {
  label: string
  kind: 'shell' | 'detachedPane' | 'detachedTab'
  /** Projects this window shows; exactly one for a detached window. */
  projects: string[]
}

export interface WindowAuditSnapshot {
  mode: WindowAuditMode
  projects: WindowAuditProject[]
  windows: WindowAuditWindow[]
}

/**
 * Everything the audit needs the host application to do.
 *
 * Each mutating call must resolve only once Rust has accepted the operation *and* the mirror
 * this window reads has been refreshed; the audit then polls `snapshot()` before asserting,
 * so a driver that returns early is caught as a settle timeout rather than as a phantom
 * pane.
 */
export interface WindowAuditDriver {
  /** The whole workspace, flattened, read fresh on every call. */
  snapshot(): WindowAuditSnapshot
  /** The label of the window the audit is running in. Checked for survival, never for focus. */
  windowLabel(): string
  /**
   * Every session id the process-global registry currently holds a child for.
   *
   * The registry, not the tree: a pane that lost its session still names it, and the whole
   * claim of M5 is about what Rust owns rather than what the layout says.
   */
  sessions(): Promise<string[]>
  /** True when a registered session's child has exited. */
  exited(session: string): Promise<boolean>
  /** Tear a pane out into its own window. Returns the new window's label. */
  detach(project: string, tab: string, pane: string): Promise<string>
  /** Put back the window a `detach` returned. */
  redock(window: string): Promise<void>
  setMode(mode: WindowAuditMode): Promise<void>
  /**
   * Bytes in the Rust screen mirror for a session, if the driver can read it.
   *
   * Optional because it is the one frontend approximation of "not one byte dropped": the
   * mirror is fed whether or not anything is attached, so a mirror that is still there after
   * a detach is a transcript that survived the window. A driver that cannot supply it loses
   * only that signal.
   */
  mirrorBytes?(session: string): Promise<number>
  /**
   * Whether a session is on the alternate screen, if the driver can read it.
   *
   * The milestone names alt-screen state because it is the part of a transcript that lives
   * in the parser rather than in the bytes: a detach that rebuilt the vt100 mirror would
   * come back on the primary screen with the pager the user was reading gone.
   *
   * Used for two claims, and the second is the one that caught a real bug: that the *session*
   * comes back on the buffer it left, and that the re-docked pane's own terminal is on the
   * same buffer as its session. The pair is the point — the session's half always passed.
   */
  inAlternateScreen?(session: string): Promise<boolean>
  /** Resolve once React has committed and the browser has laid out against it. */
  settle(): Promise<void>
}

export interface WindowAuditResult {
  detachedAndRedocked: number
  sessionsBefore: string[]
  sessionsAfter: string[]
  sessionsLost: string[]
  modeFlips: number
  failures: string[]
  /** Things that weaken the run without failing it, such as only two projects being open. */
  notes: string[]
  pass: boolean
}

/** How many detach/re-dock round trips a run makes, rotating over eligible panes. */
const DEFAULT_DETACHES = 3

/** Stacked → PerProject → Stacked, twice: state that only breaks on the second pass exists. */
const FLIP_ROUNDS = 2

/** The scale the milestone quotes. Below it the run is reported as weakened, not failed. */
const MILESTONE_PROJECTS = 3
const MILESTONE_SESSIONS = 6

/**
 * How long an operation has to appear in the snapshot before it counts as not having
 * settled. Longer than the pane audit's allowance because a detach builds an OS window and a
 * webview, and on a cold Wayland compositor that is not instant.
 */
const SETTLE_MS = 5000

const POLL_MS = 16

/** The marker a shell script greps for. Changing it breaks `scripts/`; see the file docs. */
const MARK = 'CIDE-AUDIT-SESSION'

/** Enough of a UUID to identify a thing in a report, and short enough to read in a table. */
const short = (id: string) => id.slice(0, 8)

/** Shorten every id in a composite key such as `project/tab`. */
const shortPath = (key: string) => key.split('/').map(short).join('/')

const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms))

const sorted = (ids: Iterable<string>) => [...new Set(ids)].sort()

const other = (mode: WindowAuditMode): WindowAuditMode =>
  mode === 'stacked' ? 'perProject' : 'stacked'

/** Session ids the layout claims, which must always be a subset of what the registry holds. */
function treeSessions(snapshot: WindowAuditSnapshot): string[] {
  const out: string[] = []
  for (const project of snapshot.projects) {
    for (const pane of project.panes) if (pane.session !== null) out.push(pane.session)
  }
  return sorted(out)
}

/**
 * The workspace reduced to identity, which is what a mode flip must not change.
 *
 * Keys are qualified by project so a failure names where the missing tab was, and so two
 * projects cannot cancel each other out in the comparison.
 */
interface WorkspaceShape {
  projects: string[]
  tabs: string[]
  panes: string[]
}

function shapeOf(snapshot: WindowAuditSnapshot): WorkspaceShape {
  return {
    projects: sorted(snapshot.projects.map((p) => p.id)),
    tabs: sorted(snapshot.projects.flatMap((p) => p.tabs.map((t) => `${p.id}/${t}`))),
    panes: sorted(snapshot.projects.flatMap((p) => p.panes.map((pane) => `${p.id}/${pane.id}`))),
  }
}

interface Candidate {
  project: string
  tab: string
  pane: string
  session: string | null
}

/**
 * Panes the domain will actually let go of.
 *
 * `detach_pane` refuses a tab's last pane — the tree has no way to represent an empty tab —
 * and refuses the console's primary pane while it is alone, which is the same condition. So
 * eligibility is "its tab holds another pane", and a run that asked for anything else would
 * be reporting Rust's refusals rather than the mechanism.
 */
function candidates(snapshot: WindowAuditSnapshot): Candidate[] {
  const out: Candidate[] = []
  for (const project of snapshot.projects) {
    const perTab = new Map<string, number>()
    for (const pane of project.panes) {
      if (pane.tab === null) continue
      perTab.set(pane.tab, (perTab.get(pane.tab) ?? 0) + 1)
    }
    for (const pane of project.panes) {
      if (pane.tab === null) continue
      if ((perTab.get(pane.tab) ?? 0) < 2) continue
      out.push({ project: project.id, tab: pane.tab, pane: pane.id, session: pane.session })
    }
  }
  // A pane with a session is the case the milestone is about; one without proves only that
  // the tree moved. Preferred rather than required, so a session-less workspace still runs.
  return [...out.filter((c) => c.session !== null), ...out.filter((c) => c.session === null)]
}

function findProject(
  snapshot: WindowAuditSnapshot,
  project: string,
): WindowAuditProject | undefined {
  return snapshot.projects.find((p) => p.id === project)
}

function findPane(
  snapshot: WindowAuditSnapshot,
  project: string,
  pane: string,
): WindowAuditPane | undefined {
  return findProject(snapshot, project)?.panes.find((p) => p.id === pane)
}

function panesOfTab(snapshot: WindowAuditSnapshot, project: string, tab: string): string[] {
  const found = findProject(snapshot, project)
  if (!found) return []
  return sorted(found.panes.filter((p) => p.tab === tab).map((p) => p.id))
}

/** A tab's rendered tree, or `undefined` for a tab the snapshot does not describe. */
function treeOfTab(
  snapshot: WindowAuditSnapshot,
  project: string,
  tab: string,
): string | undefined {
  return findProject(snapshot, project)?.trees[tab]
}

export async function runWindowAudit(
  driver: WindowAuditDriver,
  detaches: number = DEFAULT_DETACHES,
): Promise<WindowAuditResult> {
  const failures: string[] = []
  const notes: string[] = []
  let detachedAndRedocked = 0
  let modeFlips = 0

  const fail = (where: string, message: string) => failures.push(`${where}: ${message}`)

  // Reported once per session, per finding. Without this a session killed in the first
  // detach files the same failure on every later step and buries whatever else went wrong.
  const reportedGone = new Set<string>()
  const reportedNew = new Set<string>()
  const reportedDead = new Set<string>()
  const reportedUnheld = new Set<string>()
  const reportedShape = new Set<string>()
  let reportedMirrorFault = false

  // Host faults and evictions are counted since start-up, so only the movement during this
  // run belongs to it. Blaming the run for a fault that happened during boot sends the
  // reader to the wrong file.
  const faultBaseline = hostFaults().destroyedWhileMounted
  let faultsSeen = 0
  const evictionBaseline = evictionCounts()

  /** Poll until the snapshot reflects an operation, so the next step is not asserted mid-flight. */
  async function settled(ok: () => boolean): Promise<boolean> {
    const deadline = performance.now() + SETTLE_MS
    for (;;) {
      await driver.settle()
      if (ok()) return true
      if (performance.now() >= deadline) return false
      await sleep(POLL_MS)
    }
  }

  /** `settled`, with the timeout reported. An operation the snapshot never shows is a finding. */
  async function reach(where: string, label: string, ok: () => boolean): Promise<boolean> {
    if (await settled(ok)) return true
    fail(where, `${label} had not reached the workspace after ${SETTLE_MS}ms`)
    return false
  }

  /**
   * The check the whole milestone turns on, run after every single operation.
   *
   * Set equality against the baseline, not a count: a detach that respawned would keep the
   * count and change an id, which is exactly the failure that matters most. The liveness
   * half matters as much — a session can stay registered with a dead child, and "zero
   * deaths" is a claim about children rather than about map entries.
   */
  async function checkSessions(where: string, expected: Set<string>): Promise<string[]> {
    const now = await driver.sessions()
    const held = new Set(now)

    for (const id of expected) {
      if (held.has(id) || reportedGone.has(id)) continue
      reportedGone.add(id)
      fail(where, `session ${short(id)} is no longer in the registry`)
    }
    for (const id of now) {
      if (expected.has(id) || reportedNew.has(id)) continue
      reportedNew.add(id)
      fail(where, `session ${short(id)} appeared; a respawn keeps the pane and changes the id`)
    }
    for (const id of now) {
      if (reportedDead.has(id)) continue
      let dead = false
      try {
        dead = await driver.exited(id)
      } catch (e) {
        reportedDead.add(id)
        fail(where, `session ${short(id)} could not be polled for liveness — ${String(e)}`)
        continue
      }
      if (!dead) continue
      reportedDead.add(id)
      fail(where, `session ${short(id)} is still registered but its child has exited`)
    }

    // A pane naming a session the registry does not hold is the same death seen from the
    // other side, and it is the form the user notices: a terminal that has stopped.
    for (const id of treeSessions(driver.snapshot())) {
      if (held.has(id) || reportedUnheld.has(id)) continue
      reportedUnheld.add(id)
      fail(where, `a pane shows session ${short(id)}, which the registry does not hold`)
    }

    return now
  }

  async function mirrorOf(session: string | null): Promise<number | null> {
    if (session === null) return null
    const pending = driver.mirrorBytes?.(session)
    if (pending === undefined) return null
    try {
      return await pending
    } catch (e) {
      if (!reportedMirrorFault) {
        reportedMirrorFault = true
        notes.push(`the screen mirror could not be read, so byte continuity is unchecked — ${String(e)}`)
      }
      return null
    }
  }

  async function altScreenOf(session: string | null): Promise<boolean | null> {
    if (session === null) return null
    const pending = driver.inAlternateScreen?.(session)
    if (pending === undefined) return null
    try {
      return await pending
    } catch {
      return null
    }
  }

  function checkHostFaults(where: string): void {
    const moved = hostFaults().destroyedWhileMounted - faultBaseline
    if (moved <= faultsSeen) return
    fail(where, `${moved - faultsSeen} host(s) were destroyed while mounted`)
    faultsSeen = moved
  }

  /**
   * What the workspace says about windows, which is not what the compositor says.
   *
   * Only domain facts: how many shells there are for the mode, that every open project is
   * shown by one of them, and that the window this code is running in is still known. That
   * last one is not pedantry — `app_get_bootstrap` for an unknown label falls back to an
   * empty shell, so a flip that mints fresh labels instead of adopting the live window
   * leaves the user looking at a window with no project in it.
   */
  function checkWindows(where: string, snapshot: WindowAuditSnapshot): void {
    const shells = snapshot.windows.filter((w) => w.kind === 'shell')
    if (snapshot.mode === 'stacked') {
      if (shells.length !== 1) {
        fail(where, `${shells.length} shell window(s) in stacked mode, where there is exactly one`)
      }
    } else if (shells.length !== snapshot.projects.length) {
      fail(
        where,
        `${shells.length} shell window(s) for ${snapshot.projects.length} project(s) in perProject mode`,
      )
    }

    const shown = new Set(shells.flatMap((w) => w.projects))
    for (const project of snapshot.projects) {
      if (shown.has(project.id)) continue
      fail(where, `project ${short(project.id)} (${project.name}) is shown by no window`)
    }

    const self = driver.windowLabel()
    if (snapshot.windows.some((w) => w.label === self)) return
    fail(where, `this window (${self}) is no longer in the workspace; its bootstrap now resolves to an empty shell`)
  }

  /**
   * Set equality, reported per element: a count says a project went, never which one.
   *
   * Once per element, because the comparison is always against the shape before the first
   * flip — so a project lost in flip 1 is still missing in flips 2, 3 and 4, and re-filing it
   * each time buries whatever the later flips broke on their own.
   */
  function compare(where: string, what: string, before: string[], now: string[]): void {
    const held = new Set(now)
    const had = new Set(before)
    for (const key of before) {
      if (held.has(key) || reportedShape.has(`gone ${what} ${key}`)) continue
      reportedShape.add(`gone ${what} ${key}`)
      fail(where, `${what} ${shortPath(key)} did not survive the flip`)
    }
    for (const key of now) {
      if (had.has(key) || reportedShape.has(`new ${what} ${key}`)) continue
      reportedShape.add(`new ${what} ${key}`)
      fail(where, `${what} ${shortPath(key)} appeared during the flip`)
    }
  }

  const startSnapshot = driver.snapshot()
  const startMode = startSnapshot.mode

  // The baseline is read twice, a frame apart. Panes spawn their children after their slot
  // has been laid out, so a run that started while the app was still booting would see a
  // legitimate spawn as "a session appeared" and fail for its own impatience.
  let sessionsBefore = await driver.sessions()
  await driver.settle()
  {
    const second = await driver.sessions()
    if (second.length !== sessionsBefore.length) {
      notes.push('the workspace was still spawning when the run began; the later reading was taken as the baseline')
    }
    sessionsBefore = second
  }
  // The buffer state gets the same courtesy, held longer. A freshly spawned fullscreen TUI
  // starts on the primary screen and enters the alternate one when its first real frame
  // lands — seconds later on a machine that is also building a project's dependencies. The
  // round-trip assertion compares each session's buffer before its detach and after its
  // re-dock, so a run that starts mid-boot reads "primary", watches the child finish
  // booting *inside* the round trip, and reports the child's own transition as a mirror
  // fault. Settled means two readings two seconds apart agree for every session; the cap
  // keeps a genuinely oscillating child from wedging the audit, and says so.
  {
    const deadline = performance.now() + 30_000
    let previous: Map<string, boolean | null> | null = null
    for (;;) {
      const current = new Map<string, boolean | null>()
      for (const id of sessionsBefore) current.set(id, await altScreenOf(id))
      const settled =
        previous !== null && sessionsBefore.every((id) => current.get(id) === previous?.get(id))
      if (settled) break
      if (performance.now() > deadline) {
        notes.push('alt-screen states were still moving after 30s; the run proceeded anyway')
        break
      }
      previous = current
      await sleep(2000)
    }
  }
  const baseline = new Set(sessionsBefore)
  let sessionsAfter = sessionsBefore

  if (startSnapshot.projects.length < MILESTONE_PROJECTS) {
    notes.push(
      `${startSnapshot.projects.length} project(s) were open; the milestone flips the mode with ${MILESTONE_PROJECTS}`,
    )
  }
  if (baseline.size < MILESTONE_SESSIONS) {
    notes.push(
      `${baseline.size} live session(s); the milestone flips the mode with ${MILESTONE_SESSIONS}`,
    )
  }

  /** Windows detached but not yet put back, so the run can restore the workspace it broke. */
  const outstanding: string[] = []

  try {
    sessionsAfter = await checkSessions('before the run', baseline)

    for (let round = 1; round <= detaches; round++) {
      const eligible = candidates(driver.snapshot())
      const target = eligible[(round - 1) % Math.max(eligible.length, 1)]
      if (target === undefined) {
        fail(
          `detach ${round}`,
          'no tab held a second pane, so there was nothing the domain would let go of',
        )
        break
      }

      const { project, tab, pane, session } = target
      const where = `detach ${round} (pane ${short(pane)} of tab ${short(tab)})`
      const tabPanesBefore = panesOfTab(driver.snapshot(), project, tab)
      // The tree, not only the pane list: the milestone's words are "re-dock restores its
      // tree position", and a pane list is the same list wherever in the tab the pane came
      // back to. Read before the detach, since the detach is what collapses the split.
      const tabTreeBefore = treeOfTab(driver.snapshot(), project, tab)
      // Whether this window was ever showing the pane. Hosts are created on first mount and
      // only the active project's tabs are rendered, so a pane belonging to any other project
      // has no host here — and its absence afterwards is the normal state, not a destroyed
      // terminal. Read before the detach, because afterwards the two cases look identical.
      const hadHost = peekHost(pane) !== undefined
      const mirrorBefore = await mirrorOf(session)
      const altBefore = await altScreenOf(session)

      let label: string
      try {
        label = await driver.detach(project, tab, pane)
      } catch (e) {
        fail(where, `the detach was rejected — ${String(e)}`)
        break
      }
      outstanding.push(label)

      // The pane leaves the tree and lands in `detached`, which the flattened snapshot
      // reports as a pane with no tab. Waiting on that rather than on the window, because a
      // window this code cannot see is not evidence of anything.
      if (!(await reach(where, 'the detach', () => findPane(driver.snapshot(), project, pane)?.tab === null))) {
        // The pane is in neither half of the domain's split, so it was not moved into the
        // holding map — it was destroyed, which loses the binding to its conversation.
        if (findPane(driver.snapshot(), project, pane) === undefined) {
          fail(where, "the pane is in no tab and not in the project's detached map either")
        }
        break
      }

      const afterDetach = driver.snapshot()
      const detachedSession = findPane(afterDetach, project, pane)?.session ?? null
      if (detachedSession !== session) {
        fail(
          where,
          `the pane's session changed from ${session === null ? 'none' : short(session)} to ${detachedSession === null ? 'none' : short(detachedSession)} across the detach`,
        )
      }

      const detachedWindow = afterDetach.windows.find((w) => w.label === label)
      if (detachedWindow === undefined) {
        fail(where, `the workspace lists no window ${label} for the detached pane`)
      } else if (detachedWindow.kind !== 'detachedPane') {
        fail(where, `window ${label} is recorded as ${detachedWindow.kind}, not a detached pane`)
      }

      // The host in *this* window must have been released, not destroyed. `releaseHost`
      // keeps the terminal, its buffer and its session id, which is what makes the re-dock a
      // re-mount; `destroyHost` throws away a live turn and the id it would re-attach with.
      // Absence is the symptom of the latter — unless eviction took it, or this window never
      // had one to begin with, both of which are legitimate and excluded above.
      const host = peekHost(pane)
      const evicted = (evictionCounts()[pane] ?? 0) > (evictionBaseline[pane] ?? 0)
      if (host === undefined && hadHost && !evicted) {
        fail(where, `the pane's host is no longer registered; the detach destroyed it where it should have released it`)
      } else if (host !== undefined && !host.released) {
        fail(
          where,
          `the pane's host is still marked ${host.mounted ? 'mounted' : 'parked'}; nothing called releaseHost`,
        )
      }
      checkHostFaults(where)
      sessionsAfter = await checkSessions(where, baseline)

      const backWhere = `re-dock ${round} (pane ${short(pane)})`
      try {
        await driver.redock(label)
      } catch (e) {
        fail(backWhere, `the re-dock was rejected — ${String(e)}`)
        break
      }

      // Spelled out rather than `?.tab !== null`, which is also true of a pane that has
      // vanished from the snapshot entirely and would end the wait on the wrong evidence.
      const docked = () => {
        const found = findPane(driver.snapshot(), project, pane)
        return found !== undefined && found.tab !== null
      }
      if (!(await reach(backWhere, 'the re-dock', docked))) {
        if (findPane(driver.snapshot(), project, pane) === undefined) {
          fail(backWhere, 'the pane is gone from the project altogether')
        }
        break
      }
      outstanding.pop()
      detachedAndRedocked += 1

      const afterRedock = driver.snapshot()
      const home = findPane(afterRedock, project, pane)
      if (home === undefined || home.tab === null) {
        fail(backWhere, 'the pane came back into no tab at all')
        break
      }
      // `detach_pane` records the split the pane was torn out of — the sibling node, the
      // axis, the side, the divider id and the ratio — and `redock_pane` rebuilds exactly
      // that split whenever the sibling is still there. Nothing here closes a pane between
      // the detach and the re-dock, so "still there" always holds and the tab is owed its
      // tree back unchanged, divider ids and ratios included. Coming back into a fresh 50/50
      // is not a rounding error: it resizes the terminal, and a resized `claude` TUI repaints
      // its whole transcript, which is the failure the milestone's "restores its tree
      // position" is really about.
      if (home.tab !== tab) {
        const stillThere = findProject(afterRedock, project)?.tabs.includes(tab) ?? false
        if (stillThere) {
          fail(backWhere, `the pane came back into tab ${short(home.tab)}, not its home tab ${short(tab)}`)
        } else {
          notes.push(`${backWhere}: its home tab had gone, so the pane landed in the project console`)
        }
      } else {
        const restored = panesOfTab(afterRedock, project, tab)
        if (restored.join(',') !== tabPanesBefore.join(',')) {
          fail(
            backWhere,
            `tab ${short(tab)} holds ${restored.map(short).join(', ')} where it held ${tabPanesBefore.map(short).join(', ')}`,
          )
        }
        const tabTreeAfter = treeOfTab(afterRedock, project, tab)
        if (tabTreeBefore === undefined || tabTreeAfter === undefined) {
          // Reported rather than skipped: a driver that supplies no tree turns the sharpest
          // assertion in this file into a silent no-op, which is how a check stops checking.
          fail(backWhere, `the driver described no tree for tab ${short(tab)}, so its position could not be checked`)
        } else if (tabTreeAfter !== tabTreeBefore) {
          fail(
            backWhere,
            `tab ${short(tab)} came back as ${shortTree(tabTreeAfter)} where it was ${shortTree(tabTreeBefore)}`,
          )
        }
      }

      if (home.session !== session) {
        fail(
          backWhere,
          `the pane's session changed from ${session === null ? 'none' : short(session)} to ${home.session === null ? 'none' : short(home.session)} across the round trip`,
        )
      }
      if (afterRedock.windows.some((w) => w.label === label)) {
        fail(backWhere, `window ${label} is still in the workspace after its pane went home`)
      }

      // The mirror is fed whether or not anything is attached, so it is the one thing this
      // side of the realm boundary can say about bytes. An empty mirror where there were
      // bytes is a session that was rebuilt; a merely smaller one may be a screen the child
      // legitimately cleared, which is a note rather than a verdict.
      const mirrorAfter = await mirrorOf(session)
      if (session !== null && mirrorBefore !== null && mirrorAfter !== null && mirrorAfter < mirrorBefore) {
        if (mirrorAfter === 0) {
          fail(backWhere, `session ${short(session)}'s screen mirror is empty where it held ${mirrorBefore} bytes`)
        } else {
          notes.push(
            `${backWhere}: the screen mirror went from ${mirrorBefore} to ${mirrorAfter} bytes, which a cleared screen also does`,
          )
        }
      }

      const altAfter = await altScreenOf(session)
      if (session !== null && altBefore !== null && altAfter !== null && altAfter !== altBefore) {
        fail(
          backWhere,
          `session ${short(session)} came back on the ${altAfter ? 'alternate' : 'primary'} screen having been on the ${altBefore ? 'alternate' : 'primary'} one`,
        )
      }

      /*
       * The other half of the alt-screen claim, and the half that was never checked.
       *
       * The assertion above is session-to-session: it proves the *mirror* kept the alternate
       * screen across the round trip, which it always did. What broke was the pane — the
       * terminal that re-docked came back on whatever buffer it happened to be on, because
       * the reattach snapshot did not say which buffer it was a picture of and nothing else
       * did either. Measuring only the session's side of that pair is why a permanently
       * desynced pane could ship: the pane lost its scrollbar, xterm turned the wheel into
       * cursor keys aimed at the agent's prompt, and the agent's repaints landed in rows that
       * had scrolled away — all with this audit green.
       *
       * Waited for, not sampled: a re-docked pane re-attaches asynchronously (a spawn check,
       * an attach, a snapshot), and `hydrated` going true is the moment the snapshot has been
       * written. A pane that never gets there is not judged — it may be an evicted host, or a
       * pane belonging to a project this window is not showing.
       */
      if (session !== null && altAfter !== null && hadHost) {
        const rehydrated = await settled(() => peekHost(pane)?.hydrated === true)
        const terminal = peekHost(pane)?.terminal
        if (rehydrated && terminal !== undefined) {
          const onAlt = terminal.term.buffer.active.type === 'alternate'
          if (onAlt !== altAfter) {
            fail(
              backWhere,
              `the pane's terminal came back on the ${onAlt ? 'alternate' : 'primary'} buffer while session ${short(session)} is on the ${altAfter ? 'alternate' : 'primary'} one`,
            )
          }
        }
      }

      checkHostFaults(backWhere)
      sessionsAfter = await checkSessions(backWhere, baseline)
    }

    // Everything below is about the flip, and the shape it must not change is read here —
    // after the detaches have put every pane back, so a pane still out would not read as a
    // pane the flip lost.
    const shapeBefore = shapeOf(driver.snapshot())
    checkWindows('before the flips', driver.snapshot())

    for (let round = 1; round <= FLIP_ROUNDS; round++) {
      let broke = false
      for (const mode of [other(startMode), startMode]) {
        const where = `flip ${round} → ${mode}`
        try {
          await driver.setMode(mode)
        } catch (e) {
          fail(where, `the flip was rejected — ${String(e)}`)
          broke = true
          break
        }
        // Positive proof the flip happened: without it every check below would pass by the
        // mode never having changed at all.
        if (!(await reach(where, 'the mode change', () => driver.snapshot().mode === mode))) {
          broke = true
          break
        }
        modeFlips += 1

        const now = driver.snapshot()
        const shape = shapeOf(now)
        compare(where, 'project', shapeBefore.projects, shape.projects)
        compare(where, 'tab', shapeBefore.tabs, shape.tabs)
        compare(where, 'pane', shapeBefore.panes, shape.panes)
        checkWindows(where, now)
        checkHostFaults(where)
        sessionsAfter = await checkSessions(where, baseline)
      }
      if (broke) break
    }
  } catch (e) {
    // A driver that throws where the audit does not expect it still has to yield the
    // findings collected so far; losing them to the exception that ended the run defeats the
    // point of asserting after every step.
    fail('the run', `aborted — ${String(e)}`)
  }

  // Leave the workspace as it was found. The audit runs inside a live application, and a run
  // that ends with a pane stranded in a window of its own has broken what it was measuring.
  try {
    for (const label of outstanding) await driver.redock(label)
    if (driver.snapshot().mode !== startMode) {
      await driver.setMode(startMode)
      await settled(() => driver.snapshot().mode === startMode)
    }
  } catch (e) {
    notes.push(`the workspace could not be restored after the run — ${String(e)}`)
  }

  try {
    sessionsAfter = await driver.sessions()
  } catch (e) {
    notes.push(`the final session census failed — ${String(e)}`)
  }

  if (detachedAndRedocked === 0) {
    failures.push(
      'nothing was detached, so the criterion was never exercised; seed a tab with a second pane before running',
    )
  }
  if (modeFlips === 0) {
    failures.push('the window mode was never flipped, so half the criterion was never exercised')
  }

  const after = new Set(sessionsAfter)
  const lost = sessionsBefore.filter((id) => !after.has(id))

  // The final census is the one reading no `checkSessions` covers: it is taken after the run
  // has put back whatever it detached and returned the mode. Without these two loops a
  // session that died during the restore is printed as "sessions lost 1" directly above a
  // PASS claiming the same ids throughout.
  for (const id of lost) {
    if (reportedGone.has(id)) continue
    reportedGone.add(id)
    fail('after the run', `session ${short(id)} is no longer in the registry`)
  }
  for (const id of after) {
    if (baseline.has(id) || reportedNew.has(id)) continue
    reportedNew.add(id)
    fail('after the run', `session ${short(id)} appeared; a respawn keeps the pane and changes the id`)
  }

  return {
    detachedAndRedocked,
    sessionsBefore: sorted(sessionsBefore),
    sessionsAfter: sorted(sessionsAfter),
    sessionsLost: sorted(lost),
    modeFlips,
    failures,
    notes,
    pass: failures.length === 0,
  }
}

export function formatWindowAudit(r: WindowAuditResult): string {
  const rows: [string, string][] = [
    ['detach/re-dock round trips', String(r.detachedAndRedocked)],
    ['window mode flips', String(r.modeFlips)],
    ['sessions before', String(r.sessionsBefore.length)],
    ['sessions after', String(r.sessionsAfter.length)],
    ['sessions lost', String(r.sessionsLost.length)],
  ]
  const width = Math.max(...rows.map(([label]) => label.length))

  const lines: string[] = [
    'window audit — sessions across detach, re-dock and window-mode flips',
    '',
  ]
  for (const [label, value] of rows) lines.push(`  ${label.padEnd(width)}  ${value}`)
  lines.push('')

  // Full ids, one per line, behind a fixed marker: this is the half of the criterion that
  // leaves the browser. A shell script greps these and looks each id up in /proc/<pid>/cmdline,
  // where it appears because it is the argument to `claude --session-id`.
  lines.push('session ids:')
  for (const id of r.sessionsBefore) lines.push(`  ${MARK} before ${id}`)
  for (const id of r.sessionsAfter) lines.push(`  ${MARK} after ${id}`)
  for (const id of r.sessionsLost) lines.push(`  ${MARK} lost ${id}`)
  lines.push('')

  if (r.notes.length > 0) {
    lines.push('notes:')
    for (const note of r.notes) lines.push(`  ${note}`)
    lines.push('')
  }

  if (r.failures.length > 0) {
    lines.push('failures:')
    for (const failure of r.failures) lines.push(`  ${failure}`)
    lines.push('')
  }

  lines.push(
    r.pass
      ? `PASS — ${r.detachedAndRedocked} round trip(s) and ${r.modeFlips} flip(s), ${r.sessionsAfter.length} session(s), same ids throughout.`
      : `FAIL — ${r.failures.length} violation(s).`,
  )
  return lines.join('\n')
}

/** True when the app was started with `?windows=1`, i.e. run the M5 window audit. */
export function auditWindowsMode(): boolean {
  return new URLSearchParams(location.search).get('windows') === '1'
}
