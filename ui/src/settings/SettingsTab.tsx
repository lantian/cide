/**
 * Content mode C: the Settings tab.
 *
 * A nav on the left — a search field over the pages in four groups (M133) — and the page on the
 * right under a 20px/600 title (with an (i) when the page has more to say), a one-line
 * description, the page's tabs when it is long enough to have them, and a banner when something
 * makes the whole page moot.
 *
 * The selected section is local state seeded from the tab — and re-seeded whenever the tab is
 * re-pointed from outside, which issue #1 was about — and is *also* pushed back to Rust
 * so it survives a relaunch. Local first because a nav click that waits an IPC round trip
 * before moving reads as a stuck button; pushed back because `TabKind::Settings { section }`
 * is persisted with the rest of the workspace and a tab that always reopens on Appearance
 * would be quietly discarding state it claims to keep.
 *
 * Everything below the nav is presentational — `sections.tsx` takes settings and callbacks —
 * so this component is the only place in the screen that touches the store.
 */
import { useCallback, useEffect, useRef, useState } from 'react'
import {
  agentDefs,
  claudeTasks,
  codexCli,
  openLogDir,
  settings as settingsApi,
  type AgentModels,
  type ClaudeCliSupport,
  type CodexCliSupport,
  type ProjectId,
  type SettingsSection,
} from '@/ipc/client'
import { useWorkspace } from '@/store/workspace'
import { notifyFailure } from '@/chrome/notices'
import { Segmented } from './controls'
import type { Settings, SettingsPatch, ProjectHarnessSettings, ProjectHarnessEdit } from '@/ipc/generated'
import { SearchField } from '@/kit/components/Field'
import { InfoTip } from '@/kit/components/InfoTip'
import { Tabs } from '@/kit/components/Surface'
import { SettingsDefaults } from './controls'
import {
  SECTIONS,
  SECTION_GROUPS,
  SECTION_TABS,
  renderSection,
  sectionBanner,
  type SectionMeta,
  type SectionProps,
} from './sections'
import { searchSettings, type SettingsHit } from './settingsIndex'
import { useSettings, useSettingsActions, useSettingsDefaultsFetch } from './useSettings'
import styles from './SettingsTab.module.css'

/**
 * The section the projectless frame was last left on, for the length of this session. (M74)
 *
 * Module scope because the frame unmounts when it is closed and there is nowhere durable to put
 * this: `TabKind::Settings { section }` is the bookmark with a project, and with none there is
 * no tab to carry one. A `useState` in the frame would forget on every close, so reopening
 * would always land on Appearance — the exact behaviour the stored section exists to avoid.
 *
 * Deliberately *not* promoted to `Workspace`: a relaunch with no project open is a fresh start,
 * and a persisted section would be a new field, a new default and a new way for a hand-edited
 * document to be wrong, bought for a nav click.
 */
let frameSection: SettingsSection = 'appearance'

/** Where the projectless Settings frame should open. See [`frameSection`]. */
export function lastFrameSection(): SettingsSection {
  return frameSection
}

export interface SettingsTabProps {
  /**
   * The project owning this tab. Settings are global; the tab is not.
   *
   * `null` is the **projectless frame** (M74): with nothing open there is no tab to put this
   * screen in, so `App.tsx` draws it in the work area instead and the screen is otherwise
   * identical. Three things follow from it and nothing else does — the section is remembered in
   * [`frameSection`] rather than on a tab, and the two model probes run in the user's own
   * directory rather than a project's, which is what a global setting is about anyway.
   */
  project: ProjectId | null
  /** The section the tab was last left on. */
  section: SettingsSection
}

export function SettingsTab({ project, section }: SettingsTabProps) {
  const [active, setActive] = useState<SettingsSection>(section)
  /*
   * …and re-seeded whenever the tab is re-pointed. (Issue #1)
   *
   * Seeding only on mount was the bug: with the tab already open on Git, Agents → Configure
   * called `openTab(project, 'agents')`, Rust re-pointed the tab, the prop arrived as `'agents'`
   * — and the screen stayed on Git, because `useState` had read its argument once. Every "open
   * Settings on X" gesture did the same (`settings.keymap` too); only closing the tab and
   * letting the next one mount fresh made them work.
   *
   * Adjusted during render rather than in an effect, so there is no painted frame of the old
   * section. Not `key={section}` in `App.tsx`, the way `SettingsFrame` does it: `select` below
   * pushes every nav click through `openTab`, so the prop moves on each click and a key would
   * remount the whole screen per click — re-running the `claude --version` probe and dropping
   * the revealed log directory. Following the prop is safe for the same reason: it only moves
   * by an explicit request, or as the echo of a `select` that has already set `active` to it.
   * `tab_open_settings` keeps an open tab's section when none is named, so the rail's ⚙ does
   * not arrive here as a jump to Appearance.
   */
  const [seeded, setSeeded] = useState<SettingsSection>(section)
  if (seeded !== section) {
    setSeeded(section)
    setActive(section)
  }
  const globalSettings = useSettings()
  const [harnessScope, setHarnessScope] = useState<'global' | 'project'>('global')
  const [tabs, setTabs] = useState<Partial<Record<SettingsSection, string>>>({})
  const scopedProject = active === 'claudeSessions' && tabs.claudeSessions !== 'proxy' && harnessScope === 'project' ? project : null
  const projectName = useWorkspace((s) => project === null ? null : s.boot?.workspace.projects[project]?.name ?? null)
  const overridesKey = useWorkspace((s) => JSON.stringify(s.boot?.workspace.projectHarness ?? {}))
  const [projectValues, setProjectValues] = useState<{ project: ProjectId; settings: Settings; overrides: ProjectHarnessSettings } | null>(null)
  const globalsKey = JSON.stringify(globalSettings)
  useEffect(() => {
    if (scopedProject === null) return
    let live = true
    void Promise.all([settingsApi.effective(scopedProject), settingsApi.projectHarness(scopedProject)]).then(([settings, overrides]) => {
      if (live) setProjectValues({ project: scopedProject, settings, overrides })
    }).catch(notifyFailure)
    return () => { live = false }
  }, [scopedProject, overridesKey, globalsKey])
  const settings = scopedProject === null ? globalSettings : projectValues?.project === scopedProject ? projectValues.settings : null
  const editProjectHarness = useCallback((edit: ProjectHarnessEdit) => {
    if (scopedProject !== null) void settingsApi.setProjectHarness(scopedProject, edit).catch(notifyFailure)
  }, [scopedProject])
  const defaults = useSettingsDefaultsFetch()
  const version = useWorkspace((s) => s.boot?.capabilities.version ?? null)
  const claudeVersion = useWorkspace((s) => s.boot?.capabilities.claudeVersion ?? null)
  const { patch: globalPatch, setTheme, setWindowMode } = useSettingsActions()
  const patch = useCallback((next: SettingsPatch) => {
    if (scopedProject === null || next.proxy !== undefined) { globalPatch(next); return }
    if (next.claude) editProjectHarness({ field: 'claude', value: next.claude.cli })
    if (next.codex) editProjectHarness({ field: 'codex', value: next.codex.cli })
    if (next.opencode) editProjectHarness({ field: 'opencode', value: next.opencode.cli })
    if (next.consoleHarness) editProjectHarness({ field: 'consoleHarness', value: next.consoleHarness })
  }, [scopedProject, globalPatch, editProjectHarness])

  /**
   * The CLI verdict — can the configured binary be run, and is its version one this build's
   * protocol was checked against.
   *
   * # Re-fetched when the configured binary changes, and that is the whole reason for the dep
   *
   * The probe used to be latched in Rust for the life of the process, which was free and
   * correct while `claude` was a bare name nobody could change. It is a *setting* now: a user
   * who fixes a typo in the Binary field and reads back the verdict for the binary that
   * answered ten minutes ago has been told their correction did not work. So the Rust side is
   * unlatched and per-binary, and this effect re-runs on the value it is about.
   *
   * The cost is one `claude --version` — a Node boot, hundreds of milliseconds, on Rust's
   * blocking pool — per *distinct* binary, not per keystroke: the field commits on blur, so
   * `settings.claude.cli.binary` changes once per edit rather than once per character.
   *
   * # …and when the injections change, which is a second reason and a newer one
   *
   * `ClaudeCliSupport` stopped being a fact about a binary the moment the injection switches
   * existed. `argReasons` is now keyed by the flag cide will *actually* write — a refusal lifts
   * when an injection is switched off and moves when it is renamed — and `injectReasons` exists
   * only for the configuration that produced it. Keyed on the binary alone this effect never
   * re-ran for either, so a user who typed `sid` into a flag field got the input struck through
   * with **no sentence under it**, and a `--sid` of their own struck through with the sentence
   * for a flag nobody passes. Both are the exact state `CliReason` was added to end, and the
   * only cure was editing the binary or closing the tab.
   *
   * Serialised rather than depended on by identity: `settings` is a fresh object on every
   * workspace snapshot, so the reference changes constantly and the string does not. It changes
   * when a toggle or a spelling does, and both commit on blur or on click — the same order of
   * frequency the binary field already pays a probe for.
   *
   * `cliSupport` degrades to a verdict-free value rather than throwing, so a build without the
   * command still draws.
   */
  // `null` until bootstrap resolves. Read as `undefined` rather than defaulted to `'claude'`,
  // so the first probe waits for the real value instead of firing once for the default and
  // again for whatever the user actually configured.
  const configuredBinary = settings?.claude.cli.binary
  const configuredInjections = JSON.stringify(settings?.claude.cli.inject ?? null)
  const [cliSupport, setCliSupport] = useState<ClaudeCliSupport | null>(null)
  useEffect(() => {
    if (configuredBinary === undefined) return
    let live = true
    void claudeTasks.cliSupport(scopedProject).then((support) => {
      if (live) setCliSupport(support)
    })
    return () => {
      live = false
    }
  }, [scopedProject, configuredBinary, configuredInjections])

  /**
   * Settings → Harness → Codex's readout (M93), re-asked whenever the stored codex launch
   * configuration changes. Keyed on the whole configuration serialised, for `cliSupport`'s reason
   * above: every verdict on the codex screen comes from Rust, so any committed edit — a row, a
   * switch, the binary — is a question only this fetch answers.
   */
  const configuredCodex = settings === null ? undefined : JSON.stringify(settings.codex)
  const [codexSupport, setCodexSupport] = useState<CodexCliSupport | null>(null)
  useEffect(() => {
    if (configuredCodex === undefined) return
    let live = true
    void codexCli.support(scopedProject).then((support) => {
      if (live) setCodexSupport(support)
    })
    return () => {
      live = false
    }
  }, [scopedProject, configuredCodex])

  const configuredOpencode = JSON.stringify(settings?.opencode.cli)
  const [opencodeSupport, setOpencodeSupport] = useState<CodexCliSupport | null>(null)
  useEffect(() => {
    if (configuredOpencode === undefined) return
    let live = true
    void settingsApi.opencodeSupport(scopedProject).then((support) => { if (live) setOpencodeSupport(support) }).catch(notifyFailure)
    return () => { live = false }
  }, [scopedProject, configuredOpencode])

  /**
   * What `opencode models` reports, with cide's provider document injected. (M45)
   *
   * Refetched whenever the provider list changes — a key typed or an endpoint corrected is
   * precisely when somebody wants to know whether it worked — and keyed on the *providers* only,
   * so editing a pool (which changes nothing opencode can see) does not spawn a process.
   * `recheck` is the same fetch on demand.
   */
  const configuredProviders = JSON.stringify(settings?.llm.providers ?? null)
  const [opencodeModels, setOpencodeModels] = useState<AgentModels | null>(null)
  useEffect(() => {
    if (settings === undefined) return
    let live = true
    void agentDefs
      .models(project, 'opencode')
      // A machine with mimo and no opencode: the fork reads the same provider document, so its
      // list is the honest one for a pool. Asked only when opencode could not answer at all —
      // a *failing* opencode's sentence is still the one to show. (M81)
      .then((answer) =>
        answer !== null && answer.problem !== null && answer.models.length === 0
          ? agentDefs
              .models(project, 'mimo')
              .then((mimo) => (mimo !== null && mimo.problem === null ? mimo : answer))
              .catch(() => answer)
          : answer,
      )
      .then((answer) => {
        if (live) setOpencodeModels(answer)
      })
      // A build with no such handler, or a project that has gone away. The screen draws the
      // un-probed state, which is the same one it starts in.
      .catch(() => {
        if (live) setOpencodeModels(null)
      })
    return () => {
      live = false
    }
  }, [project, configuredProviders, settings === undefined])
  /**
   * One real turn against one model, for the Models screen's Test buttons.
   *
   * Not memoised on anything but `project`: it takes the model as an argument and reads nothing
   * else, so a provider edit must not give the buttons a new identity mid-test.
   */
  const testModel = useCallback(
    (model: string, variant?: string) => agentDefs.testModel(project, model, variant),
    [project],
  )
  /** A custom model's limits from its server, for the Models screen's auto-fill. (M88) */
  const probeLimits = useCallback(
    (provider: string, model: string) => agentDefs.probeLimits(provider, model),
    [],
  )

  /**
   * The log directory, resolved by opening it.
   *
   * Not fetched on mount: the path is only interesting once the user has asked for it, and
   * the command's whole job is the side effect. It is kept afterwards so the machine where no
   * file manager answered still ends up showing a path that can be copied — see
   * `PathReadout`.
   */
  const [logDir, setLogDir] = useState<string | null>(null)
  const revealLogDir = useCallback(() => {
    void openLogDir()
      .then(setLogDir)
      // Rejects only when the platform has no log directory or it could not be created,
      // neither of which the user can act on beyond reading the log — which is the thing they
      // cannot reach. The Rust side has already logged it.
      .catch(() => setLogDir(null))
  }, [])

  const select = useCallback(
    (next: SettingsSection) => {
      setActive(next)
      if (project === null) {
        // The projectless frame: no tab to record it on, so it is remembered for the session.
        frameSection = next
        return
      }
      // Fire and forget: the section is a bookmark, and failing to record it costs the user
      // one nav click after a relaunch. Blocking the click on it would cost every click.
      void settingsApi.openTab(project, next).catch(() => {})
    },
    [project],
  )

  /**
   * Each tabbed page's current tab (M133), for as long as this screen is mounted. Screen state,
   * not a setting: a relaunch lands on a page's first tab, which is where somebody who did not
   * come here for a specific row should land anyway.
   */
  const pageTabs = SECTION_TABS[active]
  const tab = tabs[active] ?? pageTabs?.[0]?.value ?? ''

  const pane = useRef<HTMLDivElement>(null)
  /** The row search is taking the user to, until it has been drawn and scrolled to. */
  const [landing, setLanding] = useState<string | null>(null)
  const jump = useCallback(
    (hit: SettingsHit) => {
      select(hit.section)
      if (hit.tab !== undefined) setTabs((t) => ({ ...t, [hit.section]: hit.tab }))
      setLanding(hit.label)
    },
    [select],
  )
  // After the page (and its tab) has rendered: scroll the row into the middle of the pane and
  // wash it in the accent for a moment. A row that is not there — the Agents page before its
  // roles have loaded, a row drawn only in some state — is simply not flashed; the page is
  // still the right page.
  useEffect(() => {
    if (landing === null) return
    const row = pane.current?.querySelector<HTMLElement>(`[data-setting="${CSS.escape(landing)}"]`)
    setLanding(null)
    if (row == null) return
    row.scrollIntoView({ block: 'center' })
    row.dataset.flash = ''
    // Not cleared on cleanup, deliberately: `setLanding(null)` above re-runs this effect at
    // once, and a cleanup that cancelled the timer would leave the row washed for good. A
    // timer that fires on a row that has since unmounted edits a detached node, which is free.
    window.setTimeout(() => delete row.dataset.flash, FLASH_MS)
  }, [landing, active, tab])

  // `SECTIONS` is non-empty and `active` always names one of its entries, but the compiler
  // has `noUncheckedIndexedAccess` and cannot know either — and a hand-written fallback here
  // is cheaper than the assertion that would silence it.
  const meta: SectionMeta = SECTIONS.find((s) => s.id === active) ?? {
    id: active,
    group: 'general',
    title: 'Settings',
    description: '',
  }

  const props: SectionProps | null =
    settings === null
      ? null
      : {
          settings,
          ...(scopedProject !== null && projectValues?.project === scopedProject ? { projectHarness: projectValues.overrides, editProjectHarness } : {}),
          opencodeSupport,
          tab,
          setTab: (next: string) => setTabs((t) => ({ ...t, [active]: next })),
          patch,
          setTheme,
          setWindowMode,
          version,
          claudeVersion,
          cliSupport,
          codexSupport,
          openLogDir: revealLogDir,
          logDir,
          opencodeModels,
          testModel,
          probeLimits,
        }

  return (
    <SettingsDefaults.Provider value={scopedProject === null ? defaults : globalSettings}>
      <div className={styles.tab}>
        <SettingsNav active={active} onSelect={select} onJump={jump} />

        <div ref={pane} className={styles.pane} role="tabpanel" aria-label={meta.title}>
          <header className={styles.header}>
            <h2 className={styles.title}>
              {meta.title}
              {meta.about !== undefined && (
                <InfoTip label={`About ${meta.title}`}>{meta.about}</InfoTip>
              )}
            </h2>
            <p className={styles.description}>{meta.description}</p>
            {active === 'claudeSessions' && tab !== 'proxy' && project !== null && (
              <Segmented label="Harness settings scope" value={harnessScope}
                options={[{ value: 'global', label: 'Global' }, { value: 'project', label: `Project: ${projectName ?? 'current project'}` }]}
                onChange={setHarnessScope} />
            )}
            {pageTabs !== undefined && (
              <div className={styles.pageTabs}>
                <Tabs
                  label={`${meta.title} pages`}
                  value={tab}
                  tabs={pageTabs}
                  onChange={(next) => setTabs((t) => ({ ...t, [active]: next }))}
                />
              </div>
            )}
          </header>
          <div className={styles.body}>
            {props === null ? (
              // Bootstrap has not resolved. Rendering the form against defaults would show
              // values that are not the user's and then swap them under the pointer.
              <div className={styles.loading}>Loading settings…</div>
            ) : (
              <>
                {sectionBanner(active, props)}
                {renderSection(active, props)}
              </>
            )}
          </div>
        </div>
      </div>
    </SettingsDefaults.Provider>
  )
}

/** How long a row found by search stays washed in the accent. */
const FLASH_MS = 1400

/**
 * The nav: a search field over the four groups of pages. (M133)
 *
 * With a query, the groups give way to the rows that match it — label first, then the words in
 * `settingsIndex.ts` — each naming its page, and choosing one opens that page (and tab) on the
 * row. The arrow keys move through the results from the field, Enter opens the current one and
 * Escape clears the query, so a keyboard user never leaves the field to use it.
 */
function SettingsNav({
  active,
  onSelect,
  onJump,
}: {
  active: SettingsSection
  onSelect: (next: SettingsSection) => void
  onJump: (hit: SettingsHit) => void
}) {
  const [query, setQuery] = useState('')
  const [cursor, setCursor] = useState(0)
  const hits = query.trim() === '' ? null : searchSettings(query)
  const titleOf = (id: SettingsSection) => SECTIONS.find((s) => s.id === id)?.title ?? id

  const choose = (hit: SettingsHit | undefined) => {
    if (hit === undefined) return
    onJump(hit)
  }

  return (
    <nav className={styles.nav} aria-label="Settings">
      <div className={styles.search}>
        <SearchField
          size="sm"
          aria-label="Search settings"
          placeholder="Search settings"
          value={query}
          onChange={(e) => {
            setQuery(e.target.value)
            setCursor(0)
          }}
          onKeyDown={(e) => {
            if (hits === null) return
            if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
              e.preventDefault()
              const step = e.key === 'ArrowDown' ? 1 : -1
              setCursor((c) => Math.max(0, Math.min(hits.length - 1, c + step)))
            } else if (e.key === 'Enter') {
              e.preventDefault()
              choose(hits[cursor])
            } else if (e.key === 'Escape') {
              e.preventDefault()
              e.stopPropagation()
              setQuery('')
            }
          }}
        />
      </div>

      {hits !== null ? (
        <div className={styles.results} role="listbox" aria-label="Matching settings">
          {hits.length === 0 && <div className={styles.noResults}>No setting matches.</div>}
          {hits.map((hit, i) => (
            <button
              key={`${hit.section}:${hit.label}`}
              type="button"
              role="option"
              aria-selected={i === cursor}
              className={styles.result}
              onMouseEnter={() => setCursor(i)}
              onClick={() => choose(hit)}
            >
              <span className={styles.resultLabel}>{hit.label}</span>
              <span className={styles.resultWhere}>
                {titleOf(hit.section)}
                {hit.tab !== undefined &&
                  ` › ${SECTION_TABS[hit.section]?.find((t) => t.value === hit.tab)?.label ?? hit.tab}`}
              </span>
            </button>
          ))}
        </div>
      ) : (
        <div role="tablist" aria-orientation="vertical" aria-label="Settings sections">
          {SECTION_GROUPS.map((group) => (
            <div key={group.id} className={styles.navGroup} role="presentation">
              <div className={styles.navCaption} role="presentation">
                {group.title}
              </div>
              {SECTIONS.filter((entry) => entry.group === group.id).map((entry) => {
                const selected = entry.id === active
                return (
                  <button
                    key={entry.id}
                    type="button"
                    role="tab"
                    aria-selected={selected}
                    className={selected ? `${styles.navItem} ${styles.navItemActive}` : styles.navItem}
                    onClick={() => onSelect(entry.id)}
                  >
                    {entry.title}
                  </button>
                )
              })}
            </div>
          ))}
        </div>
      )}
    </nav>
  )
}
