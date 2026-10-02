import { GitLabSettings } from '@/gitlab/GitLabSettings'
/**
 * The sections of the Settings tab, grouped for the nav (see [`SECTIONS`]).
 *
 * # How a row talks (M133)
 *
 * The screen was reported hard to read: every row said everything it knew under its label, and
 * the blue and amber notes between rows could not be tied to any one of them. So a row has one
 * line of hint (≤ 90 characters, `check:settings-copy`), and the rest is `info`, behind an (i)
 * after the label. What the whole group is about is its caption's `info`; a problem with one
 * setting's current state is that row's `status`; a problem that makes the page moot is
 * [`sectionBanner`]. A row bound to a `Settings` field spreads `resetTo(value, default, set)` for
 * its changed-from-default dot and Reset.
 *
 * Most sections are a pure function of the settings they render plus a `patch` callback:
 * `SettingsTab` does the wiring, which is what lets the whole screen be driven from a fixture and
 * keeps every section's props visible in one place.
 *
 * **Two are not, and both are the same exception.** `KeymapSection` and `AgentsSection` render
 * something that is not in `Settings` at all — the command registry, and this project's role
 * definition files — so they take no props and fetch their own state. They are the sections whose
 * subject is not a global preference, and each says so in its own header. Nothing else here may
 * quietly join them: a section that reads the store is a section the fixture cannot drive.
 *
 * The wording of the toggles is the mock's, with one deliberate exception noted on
 * `keepSessionsOnWindowClose` below.
 */
import type { ReactNode } from 'react'
import { checkForUpdates } from '@/chrome/updates'
import { notifyFailure } from '@/chrome/notices'
import type {
  ClaudeCliSupport,
  ClaudeSettings,
  CodexCliSupport,
  AwaitingHighlight,
  EditorSettings,
  ExplorerSettings,
  GitSettings,
  OpenRunIn,
  TaskProposalRunMode,
  Settings,
  SettingsPatch,
  SettingsSection,
  AgentModels,
  LlmLimitsProbe,
  LlmModelTest,
  TerminalRenderer,
  TerminalSettings,
  Theme,
  WindowMode,
} from '@/ipc/client'
import {
  ActionButton,
  Group,
  InlineControls,
  NumberField,
  PathReadout,
  Readout,
  Row,
  Segmented,
  ToggleRow,
  resetTo,
  useSettingsDefaults,
} from './controls'
import { Banner } from '@/kit/components/Feedback'
import { InfoPara } from '@/kit/components/InfoTip'
import { AgentsSection } from './AgentsSection'
import { ExtensionSettings } from './ExtensionSettings'
import { FormattersSection } from './FormattersSection'
// The band, from the module the arithmetic lives in, rather than two literals typed here.
// `check-ui-scale.mjs` pins that module against Rust's `MIN_UI_FONT_SIZE`/`MAX_UI_FONT_SIZE`,
// so importing it is what makes this input's clamp the same clamp `settings_set` applies —
// the editor's and terminal's controls below still spell 6 and 40 out, and that is the older
// shape rather than the better one.
import { MAX_UI_FONT_SIZE, MIN_UI_FONT_SIZE } from './fontScale'
import { badge, handshakeNote, sentence } from './cliHandshake'
import { HarnessChoice, HarnessLaunch, HARNESSES } from './HarnessSection'
import type { ProjectHarnessSettings, ProjectHarnessEdit } from '@/ipc/generated'
import { ModelsSection } from './ModelsSection'
import { AccentRow } from './AccentRow'
import { ColorSchemeRow } from './ColorSchemeRow'
import { GraphicsLadder } from './GraphicsLadder'
import { KeymapSection } from './KeymapSection'
import { ProxySection } from './ProxySection'
import { RemoteSection } from './RemoteSection'
import { WindowModeCards } from './WindowModeCards'

/** The nav's four groups, in order. (M133) */
export const SECTION_GROUPS: readonly { id: SectionGroup; title: string }[] = [
  { id: 'general', title: 'General' },
  { id: 'editing', title: 'Editing' },
  { id: 'ai', title: 'AI' },
  { id: 'tools', title: 'Tools' },
]

export type SectionGroup = 'general' | 'editing' | 'ai' | 'tools'

export interface SectionMeta {
  id: SettingsSection
  group: SectionGroup
  title: string
  /** One line under the title. */
  description: string
  /** What the page is, when a line was not enough — behind the title's (i). */
  about?: ReactNode
}

/**
 * Nav order and headings.
 *
 * **Not** `SettingsSection`'s order, since M133, and this comment used to claim it was while the
 * array had already drifted from it (Models and Agents sat before Extensions here, after it in
 * the enum). The nav is grouped now — General, Editing, AI, Tools — because thirteen rows with
 * no structure were one of the things the user reported as making the screen hard to find
 * anything on; the enum's ids are API and keep their order, this array decides what is drawn.
 * `check:settings-copy` holds every id to exactly one group.
 */
export const SECTIONS: readonly SectionMeta[] = [
  {
    id: 'appearance',
    group: 'general',
    title: 'Appearance',
    description: 'Theme, colours, sizes, and the rendering workarounds a machine may need.',
  },
  {
    id: 'projectsAndWindows',
    group: 'general',
    title: 'Projects & windows',
    description: 'How projects are laid out across windows, and what closing one does.',
  },
  {
    id: 'keymap',
    group: 'general',
    title: 'Keymap',
    description: 'Your overrides on top of the built-in shortcuts.',
    about: 'Defaults are compiled in; your overrides layer on top. Conflicts are reported, never resolved for you.',
  },
  {
    id: 'editor',
    group: 'editing',
    title: 'Editor',
    description: 'Indentation, completion, saving and formatters for the code buffer.',
  },
  {
    id: 'files',
    group: 'editing',
    title: 'Files',
    description: 'What the project tree walks — and so what it draws and Ctrl+P finds.',
  },
  {
    id: 'inspections',
    group: 'editing',
    title: 'Inspections',
    description: 'Which analysers run, and which of their findings you see.',
  },
  {
    // The id is API and keeps its M16 spelling; the section has been about more than claude
    // since M93, when codex became a console too.
    id: 'claudeSessions',
    group: 'ai',
    title: 'Harness',
    description: 'Which CLI a console runs, and how it is launched.',
    about: 'Claude Code or Codex: the binary, its arguments and environment, and the proxy every child process gets.',
  },
  {
    id: 'models',
    group: 'ai',
    title: 'Models',
    description: 'Providers and the ordered model pools opencode runs fall down.',
    about: 'The providers your agents may reach, and the ordered pools a run falls down when a model is busy or failing. Only opencode runs use them.',
  },
  {
    // Not a page of settings at all — nothing here rides `SettingsPatch` or `workspace.json` —
    // it is an editor for `.cide/agents/*.md` and their global twins, reached through the
    // Settings shell because that is where a person looks for *configure the thing*. The
    // `about` says so, because a section under Settings that writes files a `git status` will
    // show is not what the pages around it are.
    id: 'agents',
    group: 'ai',
    title: 'Agents',
    description: 'This project’s tracker switches, and the roles its runs are spawned as.',
    about: 'Each role is a file — a system prompt plus the switches a run is spawned with — not a cide setting, so saving one changes the project, not your preferences.',
  },
  {
    id: 'git',
    group: 'tools',
    title: 'Git',
    description: 'GitLab, what Update project does on a diverged branch, and conflicts.',
  },
  {
    id: 'terminal',
    group: 'tools',
    title: 'Terminal',
    description: 'Every pane running a shell or a TUI.',
  },
  {
    // Like `agents`, none of these rows is a cide setting: they are declared by third-party
    // manifests and stored in `extensions.json`. The `about` says so.
    id: 'extensions',
    group: 'tools',
    title: 'Extensions',
    description: 'Settings the installed extensions declare.',
    about: 'Each belongs to the extension that declared it and is stored with your extension list, not your cide preferences — so it follows you between projects and goes if you remove the extension.',
  },
  {
    id: 'remote',
    group: 'tools',
    title: 'Remote access',
    description: 'Whether a phone may reach this cide, and which devices have.',
  },
]

/**
 * The pages long enough to be split into tabs. (M133)
 *
 * The tab is screen state, not a setting: it lives in `SettingsTab` for as long as the tab is
 * open and is not persisted, so a relaunch lands on a page's first tab. Search sets it when the
 * row it found is on another one.
 */
export const SECTION_TABS: Partial<Record<SettingsSection, readonly { value: string; label: string }[]>> = {
  claudeSessions: [
    { value: 'console', label: 'Console' },
    { value: 'launch', label: 'Launch' },
    { value: 'proxy', label: 'Proxy' },
  ],
  agents: [
    { value: 'project', label: 'This project' },
    { value: 'roles', label: 'Roles' },
    { value: 'overrides', label: 'Local overrides' },
    { value: 'spawn', label: 'Review & wake' },
  ],
}

export interface SectionProps {
  settings: Settings
  projectHarness?: ProjectHarnessSettings
  editProjectHarness?: (edit: ProjectHarnessEdit) => void
  opencodeSupport?: CodexCliSupport | null
  /**
   * The page's current tab, for a page in [`SECTION_TABS`]; the first tab's value otherwise
   * (and `''` for a page with none).
   */
  tab: string
  /**
   * Switch this page's tab — for a page that is sent somewhere from outside: Agents → Configure
   * on a role has to land on the Roles tab, whatever tab the page was left on.
   */
  setTab: (next: string) => void
  patch: (patch: SettingsPatch) => void
  setTheme: (theme: Theme) => void
  setWindowMode: (mode: WindowMode) => void
  /** This build's own version, as bootstrap reported it; null until bootstrap resolves. */
  version: string | null
  /** `claude --version`, or null when the binary is not on PATH. */
  claudeVersion: string | null
  /**
   * Whether that version is one this build's IDE protocol was verified against, or null
   * before the answer arrives. Kept out of `claudeVersion` because it is a *verdict* and the
   * range it is measured against lives in Rust — see `cide_claude::version`.
   */
  cliSupport: ClaudeCliSupport | null
  /** Settings → Harness → Codex's readout, computed in Rust. (M93) `null` before it answers. */
  codexSupport: CodexCliSupport | null
  /** Reveal the log directory. Resolves to the path, whether or not a file manager opened. */
  openLogDir: () => void
  /** The log directory once it has been resolved, so it can be shown and copied. */
  logDir: string | null
  /**
   * What `opencode models` reported, with cide's own provider document injected. (M45)
   *
   * Threaded through here rather than fetched in the section, on `cliSupport`'s precedent one
   * field up: a section that reads the IPC surface is a section the fixture cannot drive, and
   * `sections.tsx`'s header calls the props-less exception closed. `null` before the probe lands,
   * and on a build with no such handler.
   */
  opencodeModels: AgentModels | null
  /**
   * Try one `provider/model` for real, for the Models screen's Test buttons. (M45)
   *
   * Spends a very small amount of quota, so it is only ever wired to a button. `null` on a build
   * with no such handler, exactly as `opencodeModels` above.
   */
  testModel: (model: string, variant?: string) => Promise<LlmModelTest | null>
  /**
   * A custom model's limits, read off its provider's server. (M88) Free, so the screen calls it
   * when a model id is entered as well as from its button.
   */
  probeLimits: (provider: string, model: string) => Promise<LlmLimitsProbe | null>
}

const THEMES: readonly { value: Theme; label: string }[] = [
  { value: 'dark', label: 'Dark' },
  { value: 'light', label: 'Light' },
]

const RENDERERS: readonly { value: TerminalRenderer; label: string }[] = [
  { value: 'auto', label: 'Auto' },
  { value: 'webgl', label: 'WebGL' },
  { value: 'dom', label: 'DOM' },
]

const AWAITING_HIGHLIGHTS: readonly { value: AwaitingHighlight; label: string }[] = [
  { value: 'accentRail', label: 'Accent rail' },
  { value: 'tintedHeader', label: 'Tinted header' },
  { value: 'paneOutline', label: 'Pane outline' },
]

function Appearance({ settings, patch, setTheme, openLogDir, logDir, version }: SectionProps) {
  const def = useSettingsDefaults()
  return (
    <>
      <Group>
        <Row
          label="Theme"
          hint="Applies at once, live terminals included."
          // Worth saying out loud now that it is true: the header's toggle and this control
          // are the same write. They used not to be, and the header's one reached neither
          // this screen nor a second window nor the next launch.
          info="Every colour in the app comes from a token, so live terminals repaint with it — no restart, no reload. The titlebar’s toggle sets the same setting."
          {...resetTo(settings.theme, def?.theme, setTheme)}
          control={
            <Segmented label="Theme" value={settings.theme} options={THEMES} onChange={setTheme} />
          }
        />
        {/*
          Directly under Theme, because it is *keyed by* Theme: this row sets the scheme for
          whichever polarity the control above is showing. Two rows apart and the pairing stops
          being visible at all.
        */}
        <ColorSchemeRow theme={settings.theme} editor={settings.editor} patch={patch} />
        {/* After the scheme rather than between it and Theme, which are a pair (above). */}
        <AccentRow accent={settings.accent} />
        <Row
          label="Waiting pane highlight"
          hint="Use the accent colour to show which pane is waiting for you. Applies to all projects."
          info="Accent rail marks the left edge; tinted header adds a wash across the top; pane outline marks every edge. Each style includes a Waiting badge and applies immediately in every window. The highlight clears when you interact with the pane."
          {...resetTo(settings.awaitingHighlight, def?.awaitingHighlight, (awaitingHighlight) =>
            patch({ awaitingHighlight }),
          )}
          control={
            <Segmented
              label="Waiting pane highlight"
              value={settings.awaitingHighlight}
              options={AWAITING_HIGHLIGHTS}
              onChange={(awaitingHighlight) => patch({ awaitingHighlight })}
            />
          }
        />
        <Row
          label="UI font size"
          hint="Everything but the editor and the terminal."
          // Named for what it excludes, because the screen already carries two other font
          // sizes and "which one moves the file tree?" is the question this answers.
          info="The file tree, the git panel, Problems, the log, tabs, menus and this screen. The editor’s and the terminal’s font sizes stay where you put them."
          {...resetTo(settings.uiFontSize, def?.uiFontSize, (uiFontSize) => patch({ uiFontSize }))}
          control={
            <NumberField
              label="UI font size"
              value={settings.uiFontSize}
              // Half steps like the two code sizes, even though this default is a whole
              // number: the scale it drives is fractional (13 → 15 moves a 10.5px label to
              // 12.1px), so a user chasing a particular size wants the finer notch.
              step={0.5}
              min={MIN_UI_FONT_SIZE}
              max={MAX_UI_FONT_SIZE}
              onChange={(uiFontSize) => patch({ uiFontSize })}
            />
          }
        />
      </Group>

      <Group title="Graphics workarounds">
        <GraphicsLadder
          graphics={settings.graphics}
          onChange={(graphics) => patch({ graphics })}
        />
      </Group>

      {/* Beside the graphics ladder rather than in a section of its own: somebody walking
          down that ladder because the app will not paint is exactly who needs the log, and
          until now there was no way to reach it from inside the app at all. */}
      <Group title="Diagnostics">
        {/* First in Diagnostics rather than in a section of its own: the person who opens the
            log directory is the person about to file a report, and a report without a version
            is a report that gets asked for one. The string is spelled exactly as
            `cide --version` prints it, so the two never disagree in a bug thread. */}
        {/* The check sits beside the version it compares against — the same `checkForUpdates`
            as the command and the About card, answered by a toast. Enabled on a -dev build
            too: the answer there is "development builds never check", which is true and says
            why, where a disabled button would say nothing. */}
        <Row
          label="Version"
          hint="This build of cide."
          info="A -dev suffix is a tree built by run.sh, not a release, and never checks for updates."
          control={
            <InlineControls>
              <Readout text={version === null ? '…' : `cide ${version}`} />
              <ActionButton
                label="Check for updates"
                onClick={() => {
                  checkForUpdates().catch((reason: unknown) => notifyFailure(reason, { project: null }))
                }}
              />
            </InlineControls>
          }
        />
        <Row
          label="Log directory"
          hint="Where cide writes its own log."
          info="Opens in your file manager; the path is shown under the row either way, for a machine where no file manager answers."
          control={<ActionButton label="Open" onClick={openLogDir} />}
        />
        {logDir !== null && <PathReadout path={logDir} />}
      </Group>
    </>
  )
}

function ProjectsAndWindows({ settings, patch, setWindowMode }: SectionProps) {
  const def = useSettingsDefaults()
  return (
    <>
      <WindowModeCards value={settings.windowMode} onChange={setWindowMode} />

      <Group title="Projects">
        <ToggleRow
          label="Each project keeps its own console tab"
          hint="Pinned, cannot be closed — only its panes can."
          checked={settings.eachProjectKeepsClaudeTab}
          // Disabled rather than hidden. The mock draws the toggle, and the rest of the model
          // does not currently allow a project without a console tab: `tabs[0]` being the
          // pinned Claude tab is an invariant `cide-core::workspace::validate` enforces. A
          // switch that moved and then sprang back would be worse than one that says why.
          disabled
          onChange={(v) => patch({ eachProjectKeepsClaudeTab: v })}
        />
        <ToggleRow
          label="Reopen the last project on launch"
          hint="Restores its tab strip and pane layout."
          {...resetTo(settings.reopenLastProject, def?.reopenLastProject, (v) =>
            patch({ reopenLastProject: v }),
          )}
          checked={settings.reopenLastProject}
          onChange={(v) => patch({ reopenLastProject: v })}
        />
        {/* Here beside *Reopen the last project*: both are about what a launch does. The patch
            sends the whole `update` group, `SettingsPatch`'s per-group rule. */}
        <ToggleRow
          label="Check for updates on start"
          hint="Offers a newer release a few seconds after launch."
          info="Asks GitHub for a newer release a few seconds after launch, and offers to install it. Development builds never check."
          {...resetTo(settings.update.checkOnStart, def?.update.checkOnStart, (v) =>
            patch({ update: { ...settings.update, checkOnStart: v } }),
          )}
          checked={settings.update.checkOnStart}
          onChange={(v) => patch({ update: { ...settings.update, checkOnStart: v } })}
        />
        {settings.update.skippedVersion !== null && (
          <Row
            label={`Skipping cide ${settings.update.skippedVersion}`}
            anchor="Skipping a version"
            hint="You chose Skip this version. Newer releases are still offered."
            control={
              <ActionButton
                label="Offer it again"
                onClick={() => patch({ update: { ...settings.update, skippedVersion: null } })}
              />
            }
          />
        )}
      </Group>

      <Group title="Closing">
        <ToggleRow
          // The mock says "Keep a project running when its window is closed — live Claude
          // sessions survive in the background". That is a promise cide does not keep: there
          // is no background daemon, and quitting the app quits its sessions. Reworded so the
          // toggle means what it does — the window-close case, and nothing about quitting.
          label="Keep sessions running when a project window is closed"
          hint="Sessions still end when cide quits."
          info="The workspace is restored on relaunch and conversations resume."
          {...resetTo(settings.keepSessionsOnWindowClose, def?.keepSessionsOnWindowClose, (v) =>
            patch({ keepSessionsOnWindowClose: v }),
          )}
          checked={settings.keepSessionsOnWindowClose}
          onChange={(v) => patch({ keepSessionsOnWindowClose: v })}
        />
        <ToggleRow
          // The info's second sentence is not decoration. This toggle governs *sessions* only:
          // unsaved editor buffers are confirmed whatever it says, because an interrupted turn
          // resumes and discarded edits do not come back. A user who turns this off and later
          // loses a buffer would rightly blame this row for saying nothing about the limit —
          // which is why that half is also the visible hint.
          label="Confirm before closing a project with a live session"
          hint="Unsaved files are always confirmed, whatever this says."
          info="“Live” means a session is working or waiting for permission — not merely that a process exists."
          {...resetTo(settings.confirmCloseWithLiveSession, def?.confirmCloseWithLiveSession, (v) =>
            patch({ confirmCloseWithLiveSession: v }),
          )}
          checked={settings.confirmCloseWithLiveSession}
          onChange={(v) => patch({ confirmCloseWithLiveSession: v })}
        />
      </Group>

      <Group title="Agent runs">
        <Row
          label="New-task proposals"
          hint="Where a proposal requested during task creation runs. Applies to all projects."
          info="Background keeps your focus. New tab opens the proposal run when it starts. You can open background proposals from OpenSpec."
          {...resetTo(settings.taskProposalRunMode, def?.taskProposalRunMode, (taskProposalRunMode) => patch({ taskProposalRunMode }))}
          control={
            <Segmented
              label="New-task proposals"
              value={settings.taskProposalRunMode}
              options={TASK_PROPOSAL_RUN_MODES}
              onChange={(taskProposalRunMode) => patch({ taskProposalRunMode })}
            />
          }
        />
        {/* M108. Here rather than under Agents, whose page writes role files and no setting at
            all: this is about where a view lands, which is this page's whole subject. */}
        <Row
          label="Open a run in"
          hint="Where the Agents panel’s Open shows a subagent."
          info="A tab of its own, or a row under the project console’s conversation. Only the button opens one — a run never opens a view by itself."
          {...resetTo(settings.openRunIn, def?.openRunIn, (openRunIn) => patch({ openRunIn }))}
          control={
            <Segmented
              label="Open a run in"
              value={settings.openRunIn}
              options={OPEN_RUN_IN}
              onChange={(openRunIn) => patch({ openRunIn })}
            />
          }
        />
      </Group>
    </>
  )
}

const OPEN_RUN_IN: readonly { value: OpenRunIn; label: string }[] = [
  { value: 'tab', label: 'Tab' },
  { value: 'split', label: 'Split' },
]

const TASK_PROPOSAL_RUN_MODES: readonly { value: TaskProposalRunMode; label: string }[] = [
  { value: 'background', label: 'Background' },
  { value: 'tab', label: 'New tab' },
]

/**
 * What the last `claude` to complete the IDE handshake on this machine was.
 *
 * # Why this row is worth a control's space
 *
 * `SUPPORTED_CLI` is a property of the source tree: it says a human recorded a version, and
 * `cargo xtask verify-cli` is what produces that record. Nothing in it is about the machine
 * the app is running on. This row is the other half — a per-machine observation that a real
 * CLI got through the whole discovery chain here — and it is the only thing on this screen
 * that is evidence rather than a claim about the world.
 *
 * Every sentence comes from `handshakeNote`, in `./cliHandshake`, which imports nothing and is
 * compiled and driven standalone by `ui/scripts/check-handshake.mjs`. This component is a
 * `switch` over the answer and holds no rule: five states, two of which look identical from a
 * distance and mean opposite things, is exactly the shape that ships inverted when it lives in
 * JSX.
 */
function CliHandshakeRow({ support }: { support: ClaudeCliSupport }) {
  const note = handshakeNote(support, Date.now())
  return (
    <Row label="Last handshake" hint={sentence(note)} control={<span>{badge(support)}</span>} />
  )
}

/**
 * A problem with the whole page, drawn as a `Banner` under its title. (M133)
 *
 * Kept to what makes the page's settings moot — a CLI that cannot run — rather than anything a
 * single row can carry: a row-sized problem is that row's `status`, so it sits on the thing it
 * is about instead of above everything.
 */
export function sectionBanner(id: SettingsSection, props: SectionProps): ReactNode {
  if (id !== 'claudeSessions') return null
  if (props.settings.consoleHarness === 'opencode') {
    const problem = props.opencodeSupport?.problem
    return problem ? <Banner tone="bad">OpenCode cannot be run: {problem}</Banner> : null
  }
  if (props.settings.consoleHarness === 'codex') {
    const problem = props.codexSupport?.problem
    return problem != null ? <Banner tone="bad">codex cannot be run: {problem}</Banner> : null
  }
  return (props.projectHarness === undefined ? props.claudeVersion === null : props.cliSupport?.problem != null) ? (
    <Banner tone="bad">
      claude is not on PATH. Panes will fail to spawn until the CLI is installed and on this
      app’s PATH.
    </Banner>
  ) : null
}

function CodexSessionsSection({ codexSupport }: SectionProps) {
  return (
    <Group
      title="Codex"
      info={
        <>
          <InfoPara>
            cide never sets <code>OPENAI_API_KEY</code> or <code>CODEX_API_KEY</code>: a key
            outranks a ChatGPT login and moves billing to API credits.
          </InfoPara>
          <InfoPara>
            A codex console gets cide’s hooks, task tools and roster through <code>-c</code>{' '}
            overrides on its own command line — nothing is written to your{' '}
            <code>~/.codex/config.toml</code> or the project’s <code>.codex/</code>.
          </InfoPara>
          <InfoPara>
            What codex has no equivalent for: the inline diffs and diagnostics of Claude Code’s IDE
            protocol, and the token and cost readout of its statusline.
          </InfoPara>
        </>
      }
    >
      <Row
        label="Version"
        hint="Launch options are on the Launch tab."
        control={<Readout text={codexSupport?.version ?? (codexSupport?.problem != null ? 'cannot be run' : '…')} />}
      />
    </Group>
  )
}

function ClaudeSessionsSection({ settings, patch, claudeVersion, cliSupport }: SectionProps) {
  const claude = settings.claude
  const set = (next: Partial<ClaudeSettings>) => patch({ claude: { ...claude, ...next } })
  const def = useSettingsDefaults()?.claude

  return (
    <>
      <Group
        title="Claude Code"
        info={
          <>
            <InfoPara>
              cide never sets <code>ANTHROPIC_API_KEY</code> and never reads{' '}
              <code>~/.claude/.credentials.json</code>: a key outranks subscription OAuth, so
              injecting one would bill a Console organisation for a Claude Max user.
            </InfoPara>
            <InfoPara>Children inherit their authentication by inheriting the environment.</InfoPara>
          </>
        }
      >
        {/* The drift case is the row's status, not a note above the page: it is a fact about
            *this* binary, and a coloured panel saying "everything is fine" on every launch is
            one nobody reads on the launch it matters. The sentence is composed in Rust so the
            verified range has exactly one home. */}
        <Row
          label="Version"
          hint="Launch options are on the Launch tab."
          status={
            cliSupport?.warning != null
              ? { tone: 'warn', text: `${cliSupport.warning}.` }
              : undefined
          }
          info={
            cliSupport?.warning != null
              ? 'The IDE integration — inline diffs, @-mentions, the editor selection — is spoken over an undocumented protocol with no version field, so it can only be right or wrong, never negotiated. The terminal itself is unaffected.'
              : undefined
          }
          control={<Readout text={claudeVersion ?? 'not found'} />}
        />
        {/* Readout weight, not a note, and deliberately: this is the *good* news case as often
            as not. The wording is `cliHandshake.ts`'s, so it can be driven by a check script. */}
        {cliSupport != null && <CliHandshakeRow support={cliSupport} />}
      </Group>

      <Group
        title="Environment"
        info="These reach a pane’s child process when it starts. A pane already running keeps the environment it was spawned with until it is restarted."
      >
        <ToggleRow
          label="Disable mouse reporting"
          hint="Keeps text selection working in the terminal."
          info={
            <>
              <code>CLAUDE_CODE_DISABLE_MOUSE=1</code>. Worth having: xterm.js has no
              shift-to-bypass gesture, so a TUI that grabs the mouse takes text selection with it.
            </>
          }
          {...resetTo(claude.disableMouse, def?.disableMouse, (v) => set({ disableMouse: v }))}
          checked={claude.disableMouse}
          onChange={(v) => set({ disableMouse: v })}
        />
        <ToggleRow
          label="Resume every Harness pane on launch"
          hint="Off: only the project’s console resumes; the rest wait behind a button."
          info="On, a restored pane picks its conversation up by itself. Resuming is not forking — it continues a conversation that already exists and sends no prompt — but it does start one harness process per pane. A pane whose transcript is gone always waits, either way."
          {...resetTo(claude.resumeAllOnLaunch, def?.resumeAllOnLaunch, (v) => set({ resumeAllOnLaunch: v }))}
          checked={claude.resumeAllOnLaunch}
          onChange={(v) => set({ resumeAllOnLaunch: v })}
        />
        <ToggleRow
          label="Full repaint on the alternate screen"
          hint="Fixes debris after a resize, at the cost of bandwidth."
          info={<code>CLAUDE_CODE_ALT_SCREEN_FULL_REPAINT=1</code>}
          {...resetTo(claude.altScreenFullRepaint, def?.altScreenFullRepaint, (v) => set({ altScreenFullRepaint: v }))}
          checked={claude.altScreenFullRepaint}
          onChange={(v) => set({ altScreenFullRepaint: v })}
        />
        <ToggleRow
          label="Disable the alternate screen"
          hint="Keeps the transcript in the scrollback instead of fullscreen."
          info={
            <>
              <code>CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN=1</code>. The transcript stays in the
              scrollback, and the fullscreen renderer the design is drawn against is off.
            </>
          }
          {...resetTo(claude.disableAlternateScreen, def?.disableAlternateScreen, (v) => set({ disableAlternateScreen: v }))}
          checked={claude.disableAlternateScreen}
          onChange={(v) => set({ disableAlternateScreen: v })}
        />
        {/* A number rather than a switch because there is no right value to default to: what a
            wheel notch is worth is this figure multiplied by how many reports the mouse
            produces per notch, and the second factor is a property of the pointer and the
            compositor that cide cannot read. `max` is the CLI's own clamp and `min` is 1
            because 0 is a value it discards — see `ClaudeSettings::SCROLL_SPEED`. */}
        <Row
          label="Scroll speed"
          hint="Transcript lines per wheel report, 1 to 20."
          info={
            <>
              <InfoPara>
                <code>CLAUDE_CODE_SCROLL_SPEED</code>. Raise it if the wheel scrolls too little;
                a high-resolution mouse can spend several reports on one notch.
              </InfoPara>
              <InfoPara>PageUp and PageDown scroll a Claude pane too, and are not affected.</InfoPara>
            </>
          }
          {...resetTo(claude.scrollSpeed, def?.scrollSpeed, (scrollSpeed) => set({ scrollSpeed }))}
          control={
            <NumberField
              label="Claude scroll speed"
              value={claude.scrollSpeed}
              min={1}
              max={20}
              onChange={(scrollSpeed) => set({ scrollSpeed })}
            />
          }
        />
      </Group>
    </>
  )
}

function Editor({ settings, patch }: SectionProps) {
  const editor = settings.editor
  const set = (next: Partial<EditorSettings>) => patch({ editor: { ...editor, ...next } })
  const def = useSettingsDefaults()?.editor
  /** `resetTo` for one boolean field of the editor group. */
  const flag = (key: EditorFlag) => resetTo(editor[key], def?.[key], (v) => set({ [key]: v }))

  return (
    <>
      <Group title="Text">
        <Row
          label="Font size"
          {...resetTo(editor.fontSize, def?.fontSize, (fontSize) => set({ fontSize }))}
          control={
            <NumberField
              label="Editor font size"
              value={editor.fontSize}
              // Half steps, because the default *is* a half: the mock's mono size is 12.5px
              // and an integer-only input snaps it to 12 the first time anyone touches it.
              step={0.5}
              min={6}
              max={40}
              onChange={(fontSize) => set({ fontSize })}
            />
          }
        />
        <ToggleRow
          label="Wrap long lines"
          {...flag('wordWrap')}
          checked={editor.wordWrap}
          onChange={(v) => set({ wordWrap: v })}
        />
        <ToggleRow
          label="Show the minimap"
          hint="The canvas strip down the right edge of a buffer."
          {...flag('showMinimap')}
          checked={editor.showMinimap}
          onChange={(v) => set({ showMinimap: v })}
        />
      </Group>

      <Group title="Indentation">
        <Row
          label="Tab size"
          {...resetTo(editor.tabSize, def?.tabSize, (tabSize) => set({ tabSize }))}
          control={
            <NumberField
              label="Tab size"
              value={editor.tabSize}
              min={1}
              max={16}
              onChange={(tabSize) => set({ tabSize })}
            />
          }
        />
        <ToggleRow
          label="Insert spaces"
          {...flag('insertSpaces')}
          checked={editor.insertSpaces}
          onChange={(v) => set({ insertSpaces: v })}
        />
        <ToggleRow
          label="Detect indentation from the file"
          hint="A file that already indents keeps its own style."
          info="A buffer that already indents with tabs, or with two spaces, keeps doing so. Tab size and Insert spaces decide only a file with no indentation of its own."
          {...flag('detectIndentation')}
          checked={editor.detectIndentation}
          onChange={(v) => set({ detectIndentation: v })}
        />
      </Group>

      <Group title="Saving">
        <ToggleRow
          label="Save automatically"
          hint="On focus loss, and after a minute with no edits."
          info="Never over a Claude diff, a conflict, or a read-only file."
          {...flag('autosave')}
          checked={editor.autosave}
          onChange={(v) => set({ autosave: v })}
        />
        <ToggleRow
          label="Trim trailing whitespace on save"
          {...flag('trimTrailingWhitespaceOnSave')}
          checked={editor.trimTrailingWhitespaceOnSave}
          onChange={(v) => set({ trimTrailingWhitespaceOnSave: v })}
        />
      </Group>

      <Group title="Completion">
        <ToggleRow
          label="Suggest completions"
          hint="What the language server offers under the caret."
          info="Tab or Enter accepts, Escape dismisses. Needs a server for the language — Rust and Go have one built in."
          {...flag('completion')}
          checked={editor.completion}
          onChange={(v) => set({ completion: v })}
        />
        <ToggleRow
          label="Open suggestions while typing"
          hint="Off: the popup opens only on Ctrl+Space."
          {...flag('completionOnTyping')}
          checked={editor.completionOnTyping}
          onChange={(v) => set({ completionOnTyping: v })}
          disabled={!editor.completion}
        />
      </Group>

      {/* What were two long notes around the list — one before it, one after — are the
          caption's (i) now: they are about the whole list, not about any one row of it. */}
      <Group
        title="Formatters"
        info={
          <>
            <InfoPara>
              Ctrl+Alt+F asks the language server first. Rust and Go format out of the box — cide
              ships rust-analyzer and gopls, so <code>rustfmt</code> and <code>gofmt</code> need no
              row here. Add one for a language with no server, or to override the one a server
              would use.
            </InfoPara>
            <InfoPara>
              One box per argument, and no shell: cide runs the program directly, so{' '}
              <code>$HOME</code>, <code>*</code>, <code>|</code> and <code>&amp;&amp;</code> are
              ordinary text. Only <code>{'${file}'}</code> and <code>{'${dir}'}</code> are
              substituted.
            </InfoPara>
            <InfoPara>
              The program must be a filter: it reads the buffer on standard input and writes the
              result to standard output — <code>prettier --stdin-filepath {'${file}'}</code>,{' '}
              <code>black -</code>, <code>gofmt</code>. A tool that rewrites files in place, such
              as <code>cargo fmt</code>, is refused rather than allowed to empty the buffer.
              Formatting never writes the file; the tab goes dirty like any other edit.
            </InfoPara>
          </>
        }
      >
        <FormattersSection editor={editor} patch={set} />
      </Group>
    </>
  )
}

/** The editor settings that are plain switches — what `Editor`'s `flag` resets. */
type EditorFlag = {
  [K in keyof EditorSettings]: EditorSettings[K] extends boolean ? K : never
}[keyof EditorSettings]

/**
 * What the file tree walks. (M18)
 *
 * Two toggles and a long explanation each, because both of them change what *opening a project costs*
 * rather than only what it looks like — flipping either one re-walks every open project, which
 * `cide_ipc::ExplorerSettings` explains and `cmd::settings::settings_set` performs. A toggle
 * with that consequence and a three-word label would be a trap.
 *
 * The wording is deliberately concrete about the cost of the second one. "Show ignored files" is
 * a reasonable-sounding request until it means `target/`, and a user who turns it on without
 * being told that has a slow Ctrl+P and no idea why — which is why "slows indexing" stayed in
 * its visible hint when M133 moved the rest behind the (i).
 */
function Files({ settings, patch }: SectionProps) {
  const explorer = settings.explorer
  const set = (next: Partial<ExplorerSettings>) => patch({ explorer: { ...explorer, ...next } })
  const def = useSettingsDefaults()?.explorer

  return (
    <Group>
      <ToggleRow
        label="Show hidden files"
        hint="Dot-prefixed entries — .claude, .github, .env — in the tree and Ctrl+P."
        info={
          <>
            <InfoPara>
              <code>.git</code> itself stays hidden: it is a database of one object per version of
              every file ever committed, and nothing in a tree can act on one.
            </InfoPara>
            <InfoPara>{REWALK}</InfoPara>
          </>
        }
        {...resetTo(explorer.showHiddenFiles, def?.showHiddenFiles, (v) => set({ showHiddenFiles: v }))}
        checked={explorer.showHiddenFiles}
        onChange={(v) => set({ showHiddenFiles: v })}
      />
      <ToggleRow
        label="Show ignored files"
        hint="target/, node_modules/ and the rest, drawn muted. Slows indexing a big project."
        info={
          <>
            <InfoPara>
              Everything <code>.gitignore</code> covers, drawn in a muted olive, the way IDEA draws
              them — and on by default, the way IDEA shows them.
            </InfoPara>
            <InfoPara>
              It is the expensive setting: on a Rust project it is hundreds of thousands of extra
              rows to walk, hold and offer to Ctrl+P. One ignore decision serves every surface, so
              this widens Find in Files and the symbol index with the tree — a find-in-files over a
              built project will read your object files.
            </InfoPara>
            <InfoPara>
              They are shown but not watched: changes inside an ignored directory appear when the
              project is indexed again, or on demand with Refresh in the folder’s context menu.
            </InfoPara>
            <InfoPara>{REWALK}</InfoPara>
          </>
        }
        {...resetTo(explorer.showIgnoredFiles, def?.showIgnoredFiles, (v) => set({ showIgnoredFiles: v }))}
        checked={explorer.showIgnoredFiles}
        onChange={(v) => set({ showIgnoredFiles: v })}
      />
    </Group>
  )
}

/**
 * The cost both Files switches share, said in each one's (i) rather than in a note under the
 * pair: the note was the thing the user could not tie to a row, and here it is about both.
 */
const REWALK =
  'Changing it re-walks every open project: the rows are not hidden, they are unwalked, so there is nothing in the index to reveal. Large projects take a moment, and Ctrl+P fills as it goes.'

/**
 * The Git screen. (M20)
 *
 * A pure function of `settings`, deliberately: `sections.tsx`'s header is explicit that
 * *"a section that reads the store is a section the fixture cannot drive"*, and every row here
 * is a plain settings value. The one thing that would need the store — what each repository's
 * own `pull.rebase` currently resolves to — is not here for that reason; the diverged row's (i)
 * names the command that clears it instead, which is what
 * `terminal/outsideOpen.ts`'s *"listed in Settings where it can be revoked"* rule asks for.
 */
function Git({ settings, patch }: SectionProps) {
  const git = settings.git
  const set = (next: Partial<GitSettings>) => patch({ git: { ...git, ...next } })
  const def = useSettingsDefaults()?.git

  return (
    <>
      <GitLabSettings />
      <Group title="Update project">
        <Row
          label="When your branch has diverged"
          hint="What Update project does when a fast-forward is impossible."
          info={
            <>
              <InfoPara>
                cide fast-forwards whenever it can. This is what happens when it cannot — when you
                have commits the remote does not and the remote has commits you do not.
              </InfoPara>
              <InfoPara>
                Ask puts the choice in front of you and offers to write your answer into that
                repository. Fast-forward only is what cide did before this setting existed: it
                refuses, and says how far apart the two have got.
              </InfoPara>
              <InfoPara>
                A repository’s own <code>pull.rebase</code> always wins over this. Ticking{' '}
                <em>remember this choice</em> in the dialog writes it into that repository’s{' '}
                <code>.git/config</code> — the key <code>git pull</code> reads in a terminal. To be
                asked again, run <code>git config --unset pull.rebase</code> there. Your global{' '}
                <code>~/.gitconfig</code> is never edited.
              </InfoPara>
            </>
          }
          {...resetTo(git.pullStrategy, def?.pullStrategy, (pullStrategy) => set({ pullStrategy }))}
          control={
            <Segmented
              label="Diverged pull"
              value={git.pullStrategy}
              options={[
                { value: 'ask', label: 'Ask' },
                { value: 'merge', label: 'Merge' },
                { value: 'rebase', label: 'Rebase' },
                { value: 'fastForward', label: 'Fast-forward only' },
              ]}
              onChange={(pullStrategy) => set({ pullStrategy })}
            />
          }
        />
      </Group>

      <Group
        title="Resolving a conflict"
        info={
          <>
            Conflicts are git’s, not cide’s. A merge or rebase that stops leaves real git state —{' '}
            <code>MERGE_HEAD</code>, an index with all three versions, markers in the files — so
            the same conflict is visible to <code>git status</code> in a terminal, survives closing
            cide, and can be abandoned with <code>git merge --abort</code>.
          </>
        }
      >
        <ToggleRow
          label="Apply non-conflicting changes automatically"
          hint="Take the changes only one side made when the resolver opens."
          info={
            <>
              <InfoPara>
                Off by default, as it is in IDEA: the middle pane opens as the version both
                branches came from, and every difference either side made is a block you take or
                reject — applying two thirds of them before you have looked undoes the reason for
                showing them.
              </InfoPara>
              <InfoPara>The toolbar’s Apply non-conflicting is the same action when you want it.</InfoPara>
            </>
          }
          {...resetTo(git.autoApplyNonConflicting, def?.autoApplyNonConflicting, (v) =>
            set({ autoApplyNonConflicting: v }),
          )}
          checked={git.autoApplyNonConflicting}
          onChange={(v) => set({ autoApplyNonConflicting: v })}
        />
      </Group>
    </>
  )
}

function Terminal({ settings, patch }: SectionProps) {
  const terminal = settings.terminal
  const set = (next: Partial<TerminalSettings>) => patch({ terminal: { ...terminal, ...next } })
  const def = useSettingsDefaults()?.terminal

  return (
    <>
      <Group>
        <Row
          label="Font size"
          {...resetTo(terminal.fontSize, def?.fontSize, (fontSize) => set({ fontSize }))}
          control={
            <NumberField
              label="Terminal font size"
              value={terminal.fontSize}
              step={0.5}
              min={6}
              max={40}
              onChange={(fontSize) => set({ fontSize })}
            />
          }
        />
        <Row
          label="Scrollback"
          hint="Lines kept per pane."
          info="Kept twice — in the webview and in the Rust mirror — so a large figure costs memory on both sides for every pane."
          {...resetTo(terminal.scrollback, def?.scrollback, (scrollback) => set({ scrollback }))}
          control={
            <NumberField
              label="Scrollback lines"
              value={terminal.scrollback}
              min={200}
              max={100000}
              onChange={(scrollback) => set({ scrollback })}
            />
          }
        />
        <Row
          label="Renderer"
          hint="Auto: WebGL for the most recent panes, DOM for the rest."
          info={
            <>
              <InfoPara>
                A setting rather than a probe: WebGL context creation succeeds even on a software
                rasteriser, and the debug renderer string is masked on Linux.
              </InfoPara>
              <InfoPara>
                Auto gives WebGL to the most recently focused panes and DOM to the rest, because
                WebKitGTK caps concurrent contexts at roughly 8–16.
              </InfoPara>
            </>
          }
          {...resetTo(terminal.renderer, def?.renderer, (renderer) => set({ renderer }))}
          control={
            <Segmented
              label="Terminal renderer"
              value={terminal.renderer}
              options={RENDERERS}
              onChange={(renderer) => set({ renderer })}
            />
          }
        />
      </Group>

      <Group title="Output">
        <ToggleRow
          label="Render JSON log lines"
          hint="Structured log lines as time, level, message and fields."
          info={
            <>
              <InfoPara>
                A shell pane that prints one JSON object per line shows them as time, level,
                message and fields instead of raw braces. Only lines that carry two of those three
                are touched; a minified file or a jq result prints exactly as it always did.
              </InfoPara>
              <InfoPara>
                The rendering replaces the line in the scrollback, so turn this off when you need to
                copy the original JSON back out.
              </InfoPara>
            </>
          }
          {...resetTo(terminal.jsonLogs, def?.jsonLogs, (jsonLogs) => set({ jsonLogs }))}
          checked={terminal.jsonLogs}
          onChange={(jsonLogs) => set({ jsonLogs })}
        />
      </Group>

      <Group title="Notifications">
        <Row
          label="Announce a job after (seconds)"
          hint="How long a command runs before its pane reports finishing."
          info={
            <>
              <InfoPara>
                A job past this lights the pane dot, the tab and project badges and the window title
                when it finishes. Shorter jobs come and go silently: the threshold is the answer to
                “did I walk away from this?”, so set it around where you stop watching.
              </InfoPara>
              <InfoPara>Applies to running shells immediately, jobs already under way included.</InfoPara>
            </>
          }
          {...resetTo(terminal.jobNotifyAfterSecs, def?.jobNotifyAfterSecs, (jobNotifyAfterSecs) =>
            set({ jobNotifyAfterSecs }),
          )}
          control={
            <NumberField
              label="Job announce threshold, seconds"
              value={terminal.jobNotifyAfterSecs}
              min={0}
              max={3600}
              // Rounded because the wire type is an integer: a typed 90.5 would not be
              // clamped by the input, and the rejected patch would silently snap back.
              onChange={(secs) => set({ jobNotifyAfterSecs: Math.round(secs) })}
            />
          }
        />
      </Group>
    </>
  )
}

/** The body for one section. */
export function renderSection(id: SettingsSection, props: SectionProps): ReactNode {
  switch (id) {
    case 'appearance':
      return <Appearance {...props} />
    case 'projectsAndWindows':
      return <ProjectsAndWindows {...props} />
    case 'keymap':
      return <KeymapSection />
    case 'claudeSessions':
      // Three tabs since M133 — Console, Launch, Proxy — where it was one page that ran to
      // five groups, a dozen notes and two launch configurations' worth of rows.
      //
      // The proxy belongs to *every* child, shells included, so on the merits it wants a nav
      // row of its own — "Network". It cannot have one without a new `SettingsSection` variant
      // in `cide-ipc/src/workspace.rs`; it sits under the section that is already about the
      // environment children are spawned with, and `ProxySection` says in its own words that
      // shells get it too.
      //
      // Only the chosen CLI's options (M93): two full launch configurations one under the
      // other read as one long form whose half is inert. The other half is still stored and
      // still used — a role whose harness is codex runs Settings → Harness → Codex whatever the
      // console is — and flipping the switch shows it.
      if (props.tab === 'proxy') return <ProxySection proxy={props.settings.proxy} patch={props.patch} />
      if (props.tab === 'launch') return <HarnessLaunch {...props} />
      return (
        <>
          <HarnessChoice {...props} />
          {props.settings.consoleHarness === 'opencode' || props.projectHarness !== undefined ? (
            <Group title={HARNESSES.find((h) => h.value === props.settings.consoleHarness)?.label}><Row label="Version" hint="Launch settings apply to new processes." control={<Readout text={props.settings.consoleHarness === 'opencode' ? props.opencodeSupport?.version ?? '…' : props.settings.consoleHarness === 'codex' ? props.codexSupport?.version ?? '…' : props.cliSupport?.version ?? props.claudeVersion ?? '…'} />} /></Group>
          ) : props.settings.consoleHarness === 'codex' ? (
            <CodexSessionsSection {...props} />
          ) : (
            <ClaudeSessionsSection {...props} />
          )}
        </>
      )
    case 'editor':
      return <Editor {...props} />
    case 'files':
      return <Files {...props} />
    case 'inspections':
      return <Inspections {...props} />
    case 'extensions':
      // No props, for `agents`' reason one case down and one of its own: nothing here rides
      // `SettingsPatch` or `workspace.json`. These values are declared by third-party manifests
      // and stored in `extensions.json`, so the component reads the extension store directly.
      return <ExtensionSettings />
    case 'models':
      return (
        <ModelsSection
          settings={props.settings.llm}
          patch={props.patch}
          models={props.opencodeModels}
          testModel={props.testModel}
          probeLimits={props.probeLimits}
        />
      )
    case 'agents':
      // No props: roles belong to a *project*, and `SectionProps` carries global settings.
      // The component takes the project from the store the way `KeymapSection` takes the
      // command registry — see its own header for why that is right here and nowhere else.
      return <AgentsSection tab={props.tab} setTab={props.setTab} />
    case 'git':
      return <Git {...props} />
    case 'terminal':
      return <Terminal {...props} />
    case 'remote':
      // Takes the group and the patch like most sections, and reads the listener's live state
      // itself — that part is not in `Settings` and cannot be: how many devices are connected
      // is a property of a socket, not of a stored value.
      return <RemoteSection settings={props.settings.remote} patch={props.patch} />
    default:
      // A `SettingsSection` variant with no case here.
      //
      // Without this the switch simply falls off the end and returns `undefined`, which React
      // renders as nothing at all — a nav row that opens a blank page, with no error anywhere.
      // That is exactly what happened when `extensions` was added, and `tsc` was silent because
      // `ReactNode` includes `undefined`. `never` makes the next one a compile error, which is
      // the same guard `chrome/TabStrip.tsx` puts on `TabKind` and for the same reason.
      return unhandled(id)
  }
}

/** Turns a future `SettingsSection` variant into a compile error rather than a blank page. */
function unhandled(section: never): never {
  throw new Error(`unhandled settings section: ${JSON.stringify(section)}`)
}

/**
 * Which analysers run, and which of their findings are shown. (M12)
 *
 * IDEA's three axes, and they are not one axis said three times:
 *
 *  * **Sources** decide which *processes run*. Turning rust-analyzer off turns off a 1–4 GB
 *    indexer, so nothing is computed and nothing can be revealed by turning a severity back on.
 *    It is also the only axis Claude sees — a severity a user hid is a statement about their own
 *    screen, not about what an agent should be told.
 *  * **Severities** decide what is *shown*, of what was computed. Applied once in Rust, so the
 *    panel, the status bar and the rail badge cannot disagree.
 *  * The **highlighting level** here is only the *default* a newly opened editor starts at. The
 *    per-editor override lives in the code pane's context menu and is deliberately not persisted:
 *    a `none` set three weeks ago, restored silently, is a user concluding their language server
 *    is broken while the app shows a confidently clean gutter.
 */
function Inspections({ settings, patch }: SectionProps) {
  const inspections = settings.inspections
  const set = (next: Partial<typeof inspections>) =>
    patch({ inspections: { ...inspections, ...next } })
  const severity = (next: Partial<typeof inspections.severities>) =>
    set({ severities: { ...inspections.severities, ...next } })
  /** Absent means shown — see `InspectionSettings::sources`. */
  const shows = (source: string) => inspections.sources[source] !== false
  const setSource = (source: string, on: boolean) =>
    set({ sources: { ...inspections.sources, [source]: on } })
  const def = useSettingsDefaults()?.inspections
  const sev = (key: keyof typeof inspections.severities) =>
    resetTo(inspections.severities[key], def?.severities[key], (v) => severity({ [key]: v }))
  const binary = (server: 'rust-analyzer' | 'gopls') =>
    resetTo(
      inspections.serverBinaries[server] ?? 'builtin',
      def === undefined ? undefined : (def.serverBinaries[server] ?? 'builtin'),
      (choice) => set({ serverBinaries: { ...inspections.serverBinaries, [server]: choice } }),
    )

  return (
    <>
      <Group
        title="Severities"
        info="Applied once, before the editor, the Problems panel and the status bar see the list — so all three agree."
      >
        <ToggleRow
          label="Errors"
          {...sev('error')}
          checked={inspections.severities.error}
          onChange={(error) => severity({ error })}
        />
        <ToggleRow
          label="Warnings"
          {...sev('warning')}
          checked={inspections.severities.warning}
          onChange={(warning) => severity({ warning })}
        />
        <ToggleRow
          label="Weak warnings"
          // Named for what the user sees in IDEA, explained for what the wire calls it. One
          // thing, three names — IDEA's *weak warning*, LSP's `Information`, our `info` — and a
          // row labelled only "Info" would leave nobody able to map it to either.
          hint="IDEA’s weak warning. Language servers report these as “information”."
          {...sev('weakWarning')}
          checked={inspections.severities.weakWarning}
          onChange={(weakWarning) => severity({ weakWarning })}
        />
        <ToggleRow
          label="Hints"
          hint="Suggestions rather than defects. Never counted in the status bar."
          {...sev('hint')}
          checked={inspections.severities.hint}
          onChange={(hint) => severity({ hint })}
        />
      </Group>

      <Group
        title="Sources"
        info={
          <>
            <InfoPara>
              A source decides which processes run: turning rust-analyzer off turns off its indexer,
              so nothing is computed for a severity to reveal.
            </InfoPara>
            <InfoPara>
              A source not listed here is shown. The list is exceptions, not a registry: a language
              server may attribute a finding to a tool of its own — rust-analyzer reports clippy’s
              lints as <code>clippy</code> — and anything cide has not heard of is shown rather
              than silently hidden. Turn one of those off and it gains a row here.
            </InfoPara>
          </>
        }
      >
        <ToggleRow
          label="rust-analyzer"
          hint="Semantic diagnostics for Rust."
          info={<code>rustup component add rust-analyzer</code>}
          checked={shows('rust-analyzer')}
          onChange={(on) => setSource('rust-analyzer', on)}
        />
        <ToggleRow
          label="gopls"
          hint="Semantic diagnostics for Go."
          info={<code>go install golang.org/x/tools/gopls@latest</code>}
          checked={shows('gopls')}
          onChange={(on) => setSource('gopls', on)}
        />
        <ToggleRow
          label="tree-sitter"
          hint="Syntax errors, in-process and instant. Only sees files you have open."
          checked={shows('tree-sitter')}
          onChange={(on) => setSource('tree-sitter', on)}
        />
        <ToggleRow
          label="Claude inspections"
          hint="A headless one-shot review, run only when you ask. Costs tokens."
          checked={shows('claude')}
          onChange={(on) => setSource('claude', on)}
        />
      </Group>

      <Group
        title="Language server builds"
        info={
          <>
            <InfoPara>
              Built-in is the build cide ships: its own rust-analyzer — extended to keep its index
              on disk instead of only in memory — and a pinned gopls, newer than most distributions
              carry.
            </InfoPara>
            <InfoPara>
              When no bundled build is present — every build run from source — Built-in quietly
              behaves like System. System always runs whatever your PATH resolves, and is never
              sent cide’s configuration.
            </InfoPara>
          </>
        }
      >
        <Row
          label="Which rust-analyzer runs"
          hint="Changing it restarts rust-analyzer and re-indexes open projects."
          {...binary('rust-analyzer')}
          control={
            <Segmented
              label="Which rust-analyzer build runs"
              value={inspections.serverBinaries['rust-analyzer'] ?? 'builtin'}
              onChange={(choice) =>
                set({
                  serverBinaries: { ...inspections.serverBinaries, 'rust-analyzer': choice },
                })
              }
              options={[
                { value: 'builtin', label: 'Built-in' },
                { value: 'system', label: 'System (PATH)' },
              ]}
            />
          }
        />
        <Row
          label="Which gopls runs"
          hint="Changing it restarts gopls."
          info="The built-in gopls keeps its file cache inside cide’s per-profile cache directory; a System gopls keeps its own machine-global one."
          {...binary('gopls')}
          control={
            <Segmented
              label="Which gopls build runs"
              value={inspections.serverBinaries['gopls'] ?? 'builtin'}
              onChange={(choice) =>
                set({
                  serverBinaries: { ...inspections.serverBinaries, 'gopls': choice },
                })
              }
              options={[
                { value: 'builtin', label: 'Built-in' },
                { value: 'system', label: 'System (PATH)' },
              ]}
            />
          }
        />
        <Row
          label="Memory limit (MiB)"
          hint="Restart a language server that grows past this. 0 is off."
          info={
            <>
              <InfoPara>
                0 is the default, because a restart re-indexes until the disk index makes it a warm
                load. Values under 256 are raised to 256.
              </InfoPara>
              <InfoPara>
                The built-in gopls also gets this as a soft limit (<code>GOMEMLIMIT</code>, at
                75%), so its GC compresses the heap before the watchdog would restart it; changing
                the value restarts gopls to apply that.
              </InfoPara>
            </>
          }
          {...resetTo(inspections.serverMemoryLimitMb, def?.serverMemoryLimitMb, (serverMemoryLimitMb) =>
            set({ serverMemoryLimitMb }),
          )}
          control={
            <NumberField
              label="Language server memory limit in MiB"
              value={inspections.serverMemoryLimitMb}
              min={0}
              max={65536}
              step={256}
              onChange={(serverMemoryLimitMb) => set({ serverMemoryLimitMb })}
            />
          }
        />
        <Row
          label="Index working set (%)"
          hint="How much of the built-in rust-analyzer’s index stays in memory."
          info={
            <>
              <InfoPara>
                A percentage of its defaults — the rest lives on disk and reloads on demand. 0 keeps
                the shipped tuning; values are kept between 25 and 400.
              </InfoPara>
              <InfoPara>
                Changing it restarts rust-analyzer (a warm load from the disk index). Only the
                built-in build listens; a System build has no disk index.
              </InfoPara>
            </>
          }
          {...resetTo(
            inspections.serverIndexWorkingSetPct,
            def?.serverIndexWorkingSetPct,
            (serverIndexWorkingSetPct) => set({ serverIndexWorkingSetPct }),
          )}
          control={
            <NumberField
              label="Index working set as a percentage of the built-in defaults"
              value={inspections.serverIndexWorkingSetPct}
              min={0}
              max={400}
              step={25}
              onChange={(serverIndexWorkingSetPct) => set({ serverIndexWorkingSetPct })}
            />
          }
        />
      </Group>

      <Group title="Highlighting">
        <Row
          label="Default level"
          hint="Where a new editor starts. Change it per file from its context menu."
          info="The level is per editor, and the panel is per project: turning highlighting down hides squiggles in that buffer only. The Problems panel and the status bar count the whole project — a per-file switch cannot coherently redefine a project-wide total. IDEA behaves the same way."
          {...resetTo(inspections.defaultHighlightLevel, def?.defaultHighlightLevel, (defaultHighlightLevel) =>
            set({ defaultHighlightLevel }),
          )}
          control={
            <Segmented
              label="Default highlighting level"
              value={inspections.defaultHighlightLevel}
              onChange={(defaultHighlightLevel) => set({ defaultHighlightLevel })}
              options={[
                { value: 'none', label: 'None' },
                { value: 'syntaxOnly', label: 'Syntax' },
                { value: 'allProblems', label: 'All problems' },
              ]}
            />
          }
        />
      </Group>
    </>
  )
}
