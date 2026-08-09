/**
 * M4's acceptance check, made runnable.
 *
 * The milestone states it as a sentence: *an instrumented counter shows `term.open()`
 * called exactly once per pane after 100 split/close/maximize/switch cycles*. A counter
 * read once at the end can only report that something went wrong, never which operation
 * did it, so this drives the real application — real `pane.split` round trips, real React
 * commits, real hosts moving between real slots — and re-asserts after every step. A
 * failure names the cycle, the step and the pane, because that is the difference between a
 * number and a bug report.
 *
 * The application is reached through {@link PaneAuditDriver} rather than through `@/ipc`
 * or the store. Partly so this file cannot drift into being a second copy of the wiring,
 * and partly because the thing under test is the *host registry's* response to layout
 * churn: whoever supplies the driver decides what churn means.
 *
 * What it cannot check: that a terminal still holds the right *bytes*. The registry can
 * prove an element was never destroyed and a renderer never re-opened; proving the
 * scrollback survived means reading the screen mirror back, which belongs to M2's PTY
 * tests and needs a session doing predictable work.
 */
import { evictionCounts, hostFaults, hostStats, liveHosts, peekHost, HOST_CAP } from './paneHosts'

/**
 * The pane tree, flattened to what the audit actually reasons about.
 *
 * `panes` is the leaf set, which `cide-core` asserts is exactly the key set of
 * `PaneTree.panes` — so an adapter is `Object.keys(tab.tree.panes)` and needs no walk of
 * `LayoutNode`. Keeping the generated shape out of this file means the audit does not have
 * to be revised every time the tree grows a field.
 */
export interface PaneAuditTree {
  panes: string[]
  focused: string
  maximized: string | null
}

/**
 * Everything the audit needs the host application to do. Every method acts on the tab that
 * `tree()` reports, except `activateTab`.
 *
 * Each mutating call must resolve only once the Rust core has accepted the operation; the
 * audit then waits for `tree()` to reflect it before asserting, so a driver that returns
 * early is caught as a settle timeout rather than as a phantom pane.
 */
export interface PaneAuditDriver {
  /** The active tab's tree, read fresh on every call. */
  tree(): PaneAuditTree
  /** Tab ids of the active project, `tabs()[0]` being the pinned console. */
  tabs(): string[]
  activeTab(): string
  activateTab(tab: string): Promise<void>
  /** Returns the new pane's id. */
  split(pane: string, axis: 'row' | 'col'): Promise<string>
  close(pane: string): Promise<void>
  focus(pane: string): Promise<void>
  /** `null` clears the flag. */
  maximize(pane: string | null): Promise<void>
  /** Resolve once React has committed and the browser has laid out against it. */
  settle(): Promise<void>
}

export interface PaneAuditResult {
  cycles: number
  /** Panes that opened a terminal at any point in this window, the run's own included. */
  panesSeen: number
  maxOpensPerPane: number
  hostsDestroyedWhileMounted: number
  /** Hosts that lost their terminal without being destroyed. */
  terminalsLost: number
  failures: string[]
  /** Things that weaken the run without failing it, such as a tab switch never exercised. */
  notes: string[]
  pass: boolean
}

const DEFAULT_CYCLES = 100

/**
 * The working set the audit holds the tab at.
 *
 * Deliberately below `HOST_CAP`: crossing it would make eviction, and therefore a second
 * legitimate `term.open()`, part of the headline number the milestone quotes. Eviction is
 * still checked — the allowance below accounts for it — but a run whose panes are evicted
 * for reasons of its own devising cannot answer the question that was asked.
 */
const WORKING_SET = 6

/** How long an operation has to appear in the tree before it counts as not having settled. */
const SETTLE_MS = 2000

const POLL_MS = 16

/** Enough of a UUID to identify a pane in a report, and short enough to read in a table. */
const short = (id: string) => id.slice(0, 8)

const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms))

export async function runPaneAudit(
  driver: PaneAuditDriver,
  cycles: number = DEFAULT_CYCLES,
): Promise<PaneAuditResult> {
  const failures: string[] = []
  const notes: string[] = []
  const seen = new Set<string>()
  const lostTerminals = new Set<string>()
  // Panes already reported, per finding. Without these, one bad pane files a failure on
  // every step of every remaining cycle and buries whatever else went wrong.
  const reportedOpens = new Set<string>()
  const reportedOrphan = new Set<string>()
  const reportedParked = new Set<string>()
  const reportedEvicted = new Set<string>()
  let reportedOverCap = false
  let reportedLeak = false
  /** Panes this run created, and so the only ones it is entitled to close. */
  const created: string[] = []

  let maxOpens = 0
  let completed = 0
  let tabSwitches = 0

  // Faults are counted since start-up, so only the movement during this run is ours. Both
  // counters need this: the app opens its first panes before the audit starts, and a note
  // blaming the run for a fault that happened during boot sends the reader to the wrong file.
  const baseline = hostFaults()
  // Evictions are cumulative too, and the checks below read movement rather than totals.
  const evictionBaseline = evictionCounts()
  let destroyedWhileMounted = 0

  const fail = (cycle: number, message: string) => failures.push(`cycle ${cycle}: ${message}`)

  /**
   * Every invariant the registry promises, re-read against the live DOM.
   *
   * Run after each step rather than each cycle: a split, a maximize and a tab switch break
   * hosts in different ways, and a check that only fires at the end of the cycle reports
   * all three as "something in cycle 37".
   */
  function assertHosts(cycle: number, step: string): void {
    const stats = hostStats()
    const evictions = evictionCounts()

    for (const [paneId, opens] of Object.entries(stats.opens)) {
      seen.add(paneId)
      maxOpens = Math.max(maxOpens, opens)
      // A pane that was evicted gave up its element, so its next mount has to open a new
      // one. That is the registry working, not failing — anything above the allowance is
      // a terminal that was re-opened while its host was still there to be reused.
      const allowance = 1 + (evictions[paneId] ?? 0)
      if (opens <= allowance || reportedOpens.has(paneId)) continue
      reportedOpens.add(paneId)
      const why =
        allowance > 1 ? ` (allowed ${allowance} after ${allowance - 1} eviction(s))` : ' (allowed 1)'
      fail(cycle, `pane ${short(paneId)} called term.open() ${opens} times${why} after ${step}`)
    }

    const faults = hostFaults().destroyedWhileMounted - baseline.destroyedWhileMounted
    if (faults > destroyedWhileMounted) {
      fail(cycle, `${faults - destroyedWhileMounted} host(s) destroyed while mounted during ${step}`)
      destroyedWhileMounted = faults
    }

    const mounted = new Map<string, boolean>()
    for (const host of liveHosts()) {
      mounted.set(host.paneId, host.mounted)
      if (!host.opened) continue

      // The host is still in the registry, so nothing declared it finished — yet its
      // terminal is gone. Either someone disposed it behind the registry's back or the
      // slot's children were replaced, which is rule 2.
      const empty = host.terminal === undefined || host.el.childElementCount === 0
      if (empty && !lostTerminals.has(host.paneId)) {
        lostTerminals.add(host.paneId)
        fail(cycle, `pane ${short(host.paneId)} lost its terminal without being destroyed, at ${step}`)
      }

      // An unmounted host belongs in parking; a parentless one has been removed from the
      // document by something that should have parked it instead.
      if (host.el.parentElement === null && !reportedOrphan.has(host.paneId)) {
        reportedOrphan.add(host.paneId)
        fail(cycle, `pane ${short(host.paneId)} host element was removed from the DOM, at ${step}`)
      }
    }

    const tree = driver.tree()

    // The eviction allowance above is what stops a legitimate rehydration reading as a bug,
    // and it is also the one place a real terminal loss could hide: a pane whose visible
    // terminal was disposed and re-created shows `opens` 2 against an allowance of 2, and
    // its host was transiently parked at the time so `destroyedWhileMounted` never moved.
    // The tab is showing these panes, so evicting one is never legitimate however the
    // counters read, and the working set is held below the cap so it should not arise.
    for (const paneId of tree.panes) {
      if ((evictions[paneId] ?? 0) <= (evictionBaseline[paneId] ?? 0)) continue
      if (reportedEvicted.has(paneId)) continue
      reportedEvicted.add(paneId)
      fail(cycle, `pane ${short(paneId)} was evicted while the tab was showing it, at ${step}`)
    }

    if (tree.maximized === null) {
      // Skipped while a pane is maximized: hiding the siblings by parking them loses no
      // state and is a legitimate way to render the flag.
      for (const paneId of tree.panes) {
        // A pane with no host at all is not a finding: hosts are created on first mount,
        // and a leaf the renderer has not reached yet has nothing to park.
        if (mounted.get(paneId) !== false || reportedParked.has(paneId)) continue
        reportedParked.add(paneId)
        fail(cycle, `pane ${short(paneId)} is in the tree but its host is parked, at ${step}`)
      }
    }

    const resident = stats.live + stats.parked
    if (resident > HOST_CAP && !reportedOverCap) {
      // Eviction declines to take a mounted or mid-turn host, so the only way to stay above
      // the cap is that every resident host claims to be one or the other — a `mounted` flag
      // left set by a slot that unmounted without parking, or a `busy` flag never cleared.
      // It does not detect leaked hosts for closed panes: those park, so eviction quietly
      // reclaims them and residency stays legal. `closed but still registered` below is the
      // check for that. Reported once: it stays true for the rest of the run.
      reportedOverCap = true
      fail(cycle, `${resident} hosts resident, cap is ${HOST_CAP}; ${stats.live} of them mounted, at ${step}`)
    }
  }

  /** Poll until the tree reflects an operation, so the next step is not asserted mid-flight. */
  async function settled(ok: () => boolean): Promise<boolean> {
    const deadline = performance.now() + SETTLE_MS
    for (;;) {
      await driver.settle()
      if (ok()) return true
      if (performance.now() >= deadline) return false
      await sleep(POLL_MS)
    }
  }

  /** `settled`, with the timeout reported. An operation the tree never shows is a finding. */
  async function reach(cycle: number, label: string, ok: () => boolean): Promise<boolean> {
    if (await settled(ok)) return true
    fail(cycle, `${label} had not reached the tree after ${SETTLE_MS}ms`)
    return false
  }

  const startingTab = driver.activeTab()

  // A pane that was already over its allowance when the run began is reported once, here,
  // and then excluded. It is still a failure — the registry is broken either way — but
  // filing it against "cycle 1, after a split" would send the reader looking in the wrong
  // place, and it would be re-filed on every step after that.
  {
    const before = hostStats().opens
    const evictions = evictionCounts()
    for (const [paneId, opens] of Object.entries(before)) {
      if (opens <= 1 + (evictions[paneId] ?? 0)) continue
      reportedOpens.add(paneId)
      failures.push(`before the run: pane ${short(paneId)} had already called term.open() ${opens} times`)
    }
  }

  try {
    for (let cycle = 1; cycle <= cycles; cycle++) {
      const before = driver.tree()
      const target = before.panes[cycle % before.panes.length]
      if (target === undefined) {
        fail(cycle, 'the tree has no panes')
        break
      }

      // Alternating so both axes, and therefore both splitter orientations and both resize
      // directions, are exercised. A row-only run never reflows a terminal's rows.
      const axis = cycle % 2 === 0 ? 'row' : 'col'

      let born: string
      try {
        born = await driver.split(target, axis)
      } catch (e) {
        fail(cycle, `split of ${short(target)} on ${axis} was rejected — ${String(e)}`)
        break
      }
      created.push(born)
      const split = `split of ${short(target)} → ${short(born)}`
      // The only settle failure that ends the run: every later step names a pane that does
      // not exist, so continuing would report the same absence a dozen ways.
      if (!(await reach(cycle, split, () => driver.tree().panes.includes(born)))) break
      assertHosts(cycle, split)

      try {
        await driver.focus(born)
        await reach(cycle, `focus of ${short(born)}`, () => driver.tree().focused === born)
        assertHosts(cycle, `focus of ${short(born)}`)

        await driver.maximize(born)
        await reach(cycle, `maximize of ${short(born)}`, () => driver.tree().maximized === born)
        assertHosts(cycle, `maximize of ${short(born)}`)

        await driver.maximize(null)
        await reach(cycle, 'restore from maximize', () => driver.tree().maximized === null)
        assertHosts(cycle, 'restore from maximize')
      } catch (e) {
        fail(cycle, `focus/maximize of ${short(born)} threw — ${String(e)}`)
        break
      }

      const other = driver.tabs().find((id) => id !== startingTab)
      if (other !== undefined) {
        try {
          await driver.activateTab(other)
          await reach(cycle, `switch to tab ${short(other)}`, () => driver.activeTab() === other)
          assertHosts(cycle, `switch to tab ${short(other)}`)

          await driver.activateTab(startingTab)
          await reach(cycle, 'switch back', () => driver.activeTab() === startingTab)
          assertHosts(cycle, 'switch back to the console tab')
          tabSwitches += 2
        } catch (e) {
          fail(cycle, `tab switch threw — ${String(e)}`)
          break
        }
      }

      // Closed oldest-first, and only panes this run made: the console's primary pane and a
      // tab's last pane are refused by Rust, and a close the audit knew would fail proves
      // nothing about the registry.
      let stuck = false
      while (driver.tree().panes.length > WORKING_SET && created.length > 0) {
        const victim = created.shift()
        if (victim === undefined) break
        if (!driver.tree().panes.includes(victim)) continue
        try {
          await driver.close(victim)
        } catch (e) {
          fail(cycle, `close of ${short(victim)} was rejected — ${String(e)}`)
          stuck = true
          break
        }
        if (!(await reach(cycle, `close of ${short(victim)}`, () => !driver.tree().panes.includes(victim)))) {
          stuck = true
          break
        }
        assertHosts(cycle, `close of ${short(victim)}`)

        // Rust has accepted the close and React has unmounted the slot, so nothing will ever
        // mount this pane again. A host still in the registry means the renderer never called
        // `destroyHost`, and the cost is invisible in every other number here: the host parks,
        // eviction reclaims it under pressure, and residency stays inside the cap while each
        // close leaks an xterm instance and its WebGL context until something else needs the
        // room. Reported once — it is one missing call site, not a hundred findings.
        if (!reportedLeak && peekHost(victim) !== undefined) {
          reportedLeak = true
          fail(cycle, `pane ${short(victim)} was closed but its host is still registered; nothing called destroyHost`)
        }
      }
      if (stuck) break

      completed = cycle
    }
  } catch (e) {
    // A driver that throws where the audit does not expect it — a `tree()` with no active
    // project, say — still has to yield the findings collected so far. Losing thirty cycles
    // of evidence to the exception that ended the run defeats the point of asserting per step.
    fail(completed + 1, `the run aborted — ${String(e)}`)
  }

  // Leave the tab as it was found. The audit runs inside a live application, and a run that
  // ends with forty panes open has broken the thing it was measuring.
  try {
    await driver.maximize(null)
    if (driver.activeTab() !== startingTab) await driver.activateTab(startingTab)
    for (const paneId of created) {
      if (!driver.tree().panes.includes(paneId)) continue
      await driver.close(paneId)
      await settled(() => !driver.tree().panes.includes(paneId))
    }
  } catch (e) {
    notes.push(`could not restore the tab after the run — ${String(e)}`)
  }

  if (completed < cycles) notes.push(`stopped after ${completed} of ${cycles} cycles`)
  if (tabSwitches === 0) {
    notes.push('no second tab existed, so the tab-switch half of the cycle never ran')
  }
  const parked = hostFaults().openedWhileParked - baseline.openedWhileParked
  if (parked > 0) notes.push(`${parked} terminal(s) were opened into a parked host`)

  return {
    cycles: completed,
    panesSeen: seen.size,
    maxOpensPerPane: maxOpens,
    hostsDestroyedWhileMounted: destroyedWhileMounted,
    terminalsLost: lostTerminals.size,
    failures,
    notes,
    pass: failures.length === 0,
  }
}

export function formatPaneAudit(r: PaneAuditResult): string {
  const rows: [string, string][] = [
    ['cycles completed', String(r.cycles)],
    ['panes with a terminal', String(r.panesSeen)],
    ['max term.open() per pane', String(r.maxOpensPerPane)],
    ['hosts destroyed while mounted', String(r.hostsDestroyedWhileMounted)],
    ['terminals lost', String(r.terminalsLost)],
  ]
  const width = Math.max(...rows.map(([label]) => label.length))

  const lines: string[] = [`pane audit — host registry under ${r.cycles} split/close/maximize/switch cycles`, '']
  for (const [label, value] of rows) lines.push(`  ${label.padEnd(width)}  ${value}`)
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
      ? `PASS — ${r.panesSeen} panes, no terminal re-opened beyond its eviction allowance, no host destroyed while mounted.`
      : `FAIL — ${r.failures.length} violation(s) across ${r.cycles} cycles.`,
  )
  return lines.join('\n')
}

/** True when the app was started with `?panes=1`, i.e. run the M4 host-registry audit. */
export function auditPanesMode(): boolean {
  return new URLSearchParams(location.search).get('panes') === '1'
}
