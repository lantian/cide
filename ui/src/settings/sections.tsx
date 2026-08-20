/**
 * The sections of the Settings tab, in `SettingsSection`'s order — which is the mock's for the
 * seven it drew, plus the ones later milestones added.
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
import type {
  ClaudeCliSupport,
  ClaudeSettings,
  EditorSettings,
  ExplorerSettings,
  Settings,
  SettingsPatch,
  SettingsSection,
  TerminalRenderer,
  TerminalSettings,
  Theme,
  WindowMode,
} from '@/ipc/client'
import {
  ActionButton,
  Group,
  NumberField,
  Note,
  PathReadout,
  Row,
  Segmented,
  ToggleRow,
} from './controls'
import { AgentsSection } from './AgentsSection'
// The band, from the module the arithmetic lives in, rather than two literals typed here.
// `check-ui-scale.mjs` pins that module against Rust's `MIN_UI_FONT_SIZE`/`MAX_UI_FONT_SIZE`,
// so importing it is what makes this input's clamp the same clamp `settings_set` applies —
// the editor's and terminal's controls below still spell 6 and 40 out, and that is the older
// shape rather than the better one.
import { MAX_UI_FONT_SIZE, MIN_UI_FONT_SIZE } from './fontScale'
import { badge, handshakeNote, sentence } from './cliHandshake'
import { ClaudeCliSection } from './ClaudeCliSection'
import { GraphicsLadder } from './GraphicsLadder'
import { KeymapSection } from './KeymapSection'
import { ProxySection } from './ProxySection'
import { WindowModeCards } from './WindowModeCards'

/** Nav order and headings. The order is the mock's and is not alphabetical. */
export const SECTIONS: readonly { id: SettingsSection; title: string; description: string }[] = [
  {
    id: 'appearance',
    title: 'Appearance',
    description: 'Theme, and the Linux rendering workarounds this machine may need.',
  },
  {
    id: 'projectsAndWindows',
    title: 'Projects & windows',
    description: 'How projects are laid out across windows, and what happens when one closes.',
  },
  {
    id: 'keymap',
    title: 'Keymap',
    description:
      'Defaults are compiled in; your overrides layer on top. Conflicts are reported, never resolved for you.',
  },
  {
    id: 'claudeSessions',
    title: 'Claude sessions',
    description: 'Which claude is launched, and the arguments and environment it is given.',
  },
  { id: 'editor', title: 'Editor', description: 'The code buffer.' },
  {
    id: 'files',
    title: 'Files',
    description: 'What the project tree walks, and therefore what it draws and what Ctrl+P finds.',
  },
  {
    id: 'inspections',
    title: 'Inspections',
    description: 'Which analysers run, and which of their findings you see.',
  },
  {
    // Between Inspections and Git because that is where `SettingsSection::Agents` sits in the
    // Rust enum, and the nav is that enum's order. It is also the honest place for it: it is
    // not a page of settings at all — nothing here rides `SettingsPatch` or `workspace.json` —
    // it is an editor for `.cide/agents/*.md` and their global twins, reached through the
    // Settings shell because that is where a person looks for *configure the thing*. The
    // description says so, because a section under Settings that writes files a `git status`
    // will show is not what the eight rows around it are.
    id: 'agents',
    title: 'Agents',
    description:
      'The subagent roles this project and you define. Each one is a file — a system prompt plus the switches a run is spawned with — not a cide setting, so saving one changes the project, not your preferences.',
  },
  { id: 'git', title: 'Git', description: 'Changelists, staging and the commit tool window.' },
  { id: 'terminal', title: 'Terminal', description: 'Every pane running a shell or a TUI.' },
]

export interface SectionProps {
  settings: Settings
  patch: (patch: SettingsPatch) => void
  setTheme: (theme: Theme) => void
  setWindowMode: (mode: WindowMode) => void
  /** `claude --version`, or null when the binary is not on PATH. */
  claudeVersion: string | null
  /**
   * Whether that version is one this build's IDE protocol was verified against, or null
   * before the answer arrives. Kept out of `claudeVersion` because it is a *verdict* and the
   * range it is measured against lives in Rust — see `cide_claude::version`.
   */
  cliSupport: ClaudeCliSupport | null
  /** Reveal the log directory. Resolves to the path, whether or not a file manager opened. */
  openLogDir: () => void
  /** The log directory once it has been resolved, so it can be shown and copied. */
  logDir: string | null
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

function Appearance({ settings, patch, setTheme, openLogDir, logDir }: SectionProps) {
  return (
    <>
      <Group>
        <Row
          label="Theme"
          // Worth saying out loud now that it is true: the header's toggle and this control
          // are the same write. They used not to be, and the header's one reached neither
          // this screen nor a second window nor the next launch.
          hint="Every colour in the app comes from a token, so live terminals repaint with it — no restart, no reload. The titlebar’s toggle sets the same setting."
          control={
            <Segmented label="Theme" value={settings.theme} options={THEMES} onChange={setTheme} />
          }
        />
        <Row
          label="UI font size"
          // Named for what it excludes, because the screen already carries two other font
          // sizes and "which one moves the file tree?" is the question this hint answers.
          hint="Everything except the editor and the terminal — the file tree, the git panel, Problems, the log, tabs, menus and this screen. The two settings below stay where you put them."
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
        <Row
          label="Log directory"
          hint="Where the app writes its own log. Opens in your file manager; the path is shown below either way."
          control={<ActionButton label="Open" onClick={openLogDir} />}
        />
        {logDir !== null && <PathReadout path={logDir} />}
      </Group>
    </>
  )
}

function ProjectsAndWindows({ settings, patch, setWindowMode }: SectionProps) {
  return (
    <>
      <WindowModeCards value={settings.windowMode} onChange={setWindowMode} />

      <ToggleRow
        label="Each project keeps its own Claude tab"
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
        checked={settings.reopenLastProject}
        onChange={(v) => patch({ reopenLastProject: v })}
      />
      <ToggleRow
        // The mock says "Keep a project running when its window is closed — live Claude
        // sessions survive in the background". That is a promise cide does not keep: there is
        // no background daemon, and quitting the app quits its sessions. Reworded so the
        // toggle means what it does — the window-close case, and nothing about quitting.
        label="Keep sessions running when a project window is closed"
        hint="Sessions end when cide quits. The workspace is restored on relaunch and conversations resume."
        checked={settings.keepSessionsOnWindowClose}
        onChange={(v) => patch({ keepSessionsOnWindowClose: v })}
      />
      <ToggleRow
        // The hint's second sentence is not decoration. This toggle governs *sessions* only:
        // unsaved editor buffers are confirmed whatever it says, because an interrupted turn
        // resumes and discarded edits do not come back. A user who turns this off and later
        // loses a buffer would rightly blame this row for saying nothing about the limit.
        label="Confirm before closing a project with a live session"
        hint="“Live” means a session is working or waiting for permission — not merely that a process exists. Unsaved files are always confirmed."
        checked={settings.confirmCloseWithLiveSession}
        onChange={(v) => patch({ confirmCloseWithLiveSession: v })}
      />
    </>
  )
}

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

function ClaudeSessionsSection({ settings, patch, claudeVersion, cliSupport }: SectionProps) {
  const claude = settings.claude
  const set = (next: Partial<ClaudeSettings>) => patch({ claude: { ...claude, ...next } })

  return (
    <>
      <Note title={claudeVersion ?? 'claude is not on PATH'}>
        {claudeVersion === null
          ? 'Panes will fail to spawn until the CLI is installed and on this app’s PATH.'
          : 'cide never sets ANTHROPIC_API_KEY and never reads ~/.claude/.credentials.json: a key outranks subscription OAuth, so injecting one would bill a Console organisation for a Claude Max user. Children inherit their authentication by inheriting the environment.'}
      </Note>

      {/* Rendered only when there is something to say. A note reading "everything is fine"
          on every launch is a note nobody reads on the launch it matters — which is the
          launch after the CLI updated itself out from under this build. The sentence is
          composed in Rust so the verified range has exactly one home. */}
      {cliSupport?.warning != null && (
        <Note title="Untested Claude Code version" tone="warn">
          {cliSupport.warning}. The IDE integration — inline diffs, @-mentions, the editor
          selection — is spoken over an undocumented protocol with no version field, so it can
          only be right or wrong, never negotiated. The terminal itself is unaffected.
        </Note>
      )}

      {/* A `Group` of readout weight, not a `Note`, and deliberately: this is the *good* news
          case as often as not, and a coloured panel that says "everything is fine" on every
          launch is one nobody reads on the launch it matters. The `Note tone="warn"` above
          stays reserved for the drift case. The wording is `cliHandshake.ts`'s, so it can be
          driven by a check script. */}
      {cliSupport != null && (
        <Group title="IDE protocol">
          <CliHandshakeRow support={cliSupport} />
        </Group>
      )}

      <Group title="Environment">
        <ToggleRow
          label="Disable mouse reporting"
          hint="CLAUDE_CODE_DISABLE_MOUSE=1. Worth having: xterm.js has no shift-to-bypass gesture, so a TUI that grabs the mouse takes text selection with it."
          checked={claude.disableMouse}
          onChange={(v) => set({ disableMouse: v })}
        />
        <ToggleRow
          label="Resume every Claude pane on launch"
          hint="On, a restored pane picks its conversation up by itself. Off, only the project's console does and the rest wait behind a Resume button. Resuming is not forking — it continues a conversation that already exists and sends no prompt — but it does start one claude process per pane. A pane whose transcript is gone always waits, either way."
          checked={claude.resumeAllOnLaunch}
          onChange={(v) => set({ resumeAllOnLaunch: v })}
        />
        <ToggleRow
          label="Full repaint on the alternate screen"
          hint="CLAUDE_CODE_ALT_SCREEN_FULL_REPAINT=1. Costs bandwidth; fixes a TUI that leaves debris behind after a resize."
          checked={claude.altScreenFullRepaint}
          onChange={(v) => set({ altScreenFullRepaint: v })}
        />
        <ToggleRow
          label="Disable the alternate screen"
          hint="CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN=1. The transcript stays in the scrollback, and the fullscreen renderer the design is drawn against is off."
          checked={claude.disableAlternateScreen}
          onChange={(v) => set({ disableAlternateScreen: v })}
        />
        {/* A number rather than a switch because there is no right value to default to: what a
            wheel notch is worth is this figure multiplied by how many reports the mouse
            produces per notch, and the second factor is a property of the pointer and the
            compositor that cide cannot read. Raising this is the one lever that works whichever
            way that lands. `max` is the CLI's own clamp and `min` is 1 because 0 is a value it
            discards — see `ClaudeSettings::SCROLL_SPEED`, which is where both come from. */}
        <Row
          label="Scroll speed"
          hint="CLAUDE_CODE_SCROLL_SPEED. Transcript lines a Claude pane moves per wheel report, 1 to 20. Raise it if the wheel scrolls too little; a high-resolution mouse can spend several reports on one notch. PageUp and PageDown scroll a Claude pane too, and are not affected."
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

      <Note title="Applied at spawn">
        These reach a pane's child process when it starts. A pane already running keeps the
        environment it was spawned with until it is restarted.
      </Note>

      {/* The launch configuration: which `claude`, and what beyond cide's own argv and
          environment. A component of its own rather than more rows here, for the same reason
          `ProxySection` is one — it has repeatable rows, a readout and a hard verdict, none of
          which is this file's `Row`/`Note` vocabulary.

          It also has to stay out of *this* file. `check-claude-env.mjs` asserts set-equality
          between the `CLAUDE_CODE_*` names in `child_env.rs` and the ones named here, so that a
          switch wired to nothing fails a gate; the refusal list names four of those variables
          for the opposite reason, and spelling them in this file would break that check with a
          message about a defect that is not there. */}
      <ClaudeCliSection
        cli={claude.cli}
        onChange={(cli) => set({ cli })}
        support={cliSupport}
      />
    </>
  )
}

function Editor({ settings, patch }: SectionProps) {
  const editor = settings.editor
  const set = (next: Partial<EditorSettings>) => patch({ editor: { ...editor, ...next } })

  return (
    <>
      <Group>
        <Row
          label="Font size"
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
        <Row
          label="Tab size"
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
          checked={editor.insertSpaces}
          onChange={(v) => set({ insertSpaces: v })}
        />
        <ToggleRow
          label="Show the minimap"
          hint="The 96px canvas strip down the right edge of a buffer."
          checked={editor.showMinimap}
          onChange={(v) => set({ showMinimap: v })}
        />
        <ToggleRow
          label="Wrap long lines"
          checked={editor.wordWrap}
          onChange={(v) => set({ wordWrap: v })}
        />
        <ToggleRow
          label="Trim trailing whitespace on save"
          checked={editor.trimTrailingWhitespaceOnSave}
          onChange={(v) => set({ trimTrailingWhitespaceOnSave: v })}
        />
        <ToggleRow
          label="Save automatically"
          hint="Writes a changed file when it loses focus, and after a minute with no edits. Never over a Claude diff, a conflict, or a read-only file."
          checked={editor.autosave}
          onChange={(v) => set({ autosave: v })}
        />
      </Group>
    </>
  )
}

/**
 * What the file tree walks. (M18)
 *
 * Two toggles and a long hint each, because both of them change what *opening a project costs*
 * rather than only what it looks like — flipping either one re-walks every open project, which
 * `cide_ipc::ExplorerSettings` explains and `cmd::settings::settings_set` performs. A toggle
 * with that consequence and a three-word label would be a trap.
 *
 * The wording is deliberately concrete about the cost of the second one. "Show ignored files" is
 * a reasonable-sounding request until it means `target/`, and a user who turns it on without
 * being told that has a slow Ctrl+P and no idea why.
 */
function Files({ settings, patch }: SectionProps) {
  const explorer = settings.explorer
  const set = (next: Partial<ExplorerSettings>) => patch({ explorer: { ...explorer, ...next } })

  return (
    <Group>
      <ToggleRow
        label="Show hidden files"
        hint="Dot-prefixed entries — .claude, .github, .env — in the file tree and in Ctrl+P. .git itself stays hidden: it is a database of one object per version of every file ever committed, and nothing in a tree can act on one."
        checked={explorer.showHiddenFiles}
        onChange={(v) => set({ showHiddenFiles: v })}
      />
      <ToggleRow
        label="Show ignored files"
        hint="Everything .gitignore covers — target/, node_modules/, dist/ — drawn in a muted olive, the way IDEA draws them. On by default, the way IDEA shows them. It is the expensive setting: on a Rust project it is hundreds of thousands of extra rows to walk, hold and offer to Ctrl+P, so turn it off if indexing a large project feels slow. One ignore decision serves every surface, so this widens Find in Files and the symbol index with the tree — a find-in-files over a built project will read your object files. They are shown but not watched, so changes inside an ignored directory appear when the project is indexed again rather than as they happen."
        checked={explorer.showIgnoredFiles}
        onChange={(v) => set({ showIgnoredFiles: v })}
      />
      <Note title="Changing either one re-walks every open project">
        The rows are not hidden, they are <em>unwalked</em> — there is nothing in the index to
        reveal — so the walk runs again. Large projects take a moment, and Ctrl+P fills as it
        goes.
      </Note>
    </Group>
  )
}

function Git() {
  return (
    <Note title="No git settings yet">
      The commit tool window and its changelists land with the git milestone. Changelists are
      cide's own model rather than the index — committing rewrites <code>.git/index</code> from
      the active changelist — so this section will grow the switch that hands staging back to
      git, and the guard for an index changed outside cide. See{' '}
      <code>docs/adr/0004-changelists-not-index.md</code>.
    </Note>
  )
}

function Terminal({ settings, patch }: SectionProps) {
  const terminal = settings.terminal
  const set = (next: Partial<TerminalSettings>) => patch({ terminal: { ...terminal, ...next } })

  return (
    <>
      <Group>
        <Row
          label="Font size"
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
          hint="Lines kept per pane, in the webview and in the Rust mirror both."
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
          hint="A setting rather than a probe: WebGL context creation succeeds even on a software rasteriser, and the debug renderer string is masked on Linux. Auto gives WebGL to the most recently focused panes and DOM to the rest, because WebKitGTK caps concurrent contexts at roughly 8-16."
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
      // The proxy belongs to *every* child, shells included, so on the merits it wants a nav
      // row of its own — "Network". It cannot have one yet: the nav is keyed by
      // `SettingsSection`, a Rust enum in `cide-ipc/src/workspace.rs`, and adding a variant
      // there is not this change's to make. It sits under the section that is already about
      // the environment children are spawned with, and `ProxySection` says in its own words
      // that shells get it too. Promoting it later is three lines: the enum variant, a
      // `SECTIONS` entry, and a `case` here.
      return (
        <>
          <ClaudeSessionsSection {...props} />
          <ProxySection proxy={props.settings.proxy} patch={props.patch} />
        </>
      )
    case 'editor':
      return <Editor {...props} />
    case 'files':
      return <Files {...props} />
    case 'inspections':
      return <Inspections {...props} />
    case 'agents':
      // No props: roles belong to a *project*, and `SectionProps` carries global settings.
      // The component takes the project from the store the way `KeymapSection` takes the
      // command registry — see its own header for why that is right here and nowhere else.
      return <AgentsSection />
    case 'git':
      return <Git />
    case 'terminal':
      return <Terminal {...props} />
  }
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

  return (
    <>
      <Group title="Severities">
        <ToggleRow
          label="Errors"
          checked={inspections.severities.error}
          onChange={(error) => severity({ error })}
        />
        <ToggleRow
          label="Warnings"
          checked={inspections.severities.warning}
          onChange={(warning) => severity({ warning })}
        />
        <ToggleRow
          label="Weak warnings"
          // Named for what the user sees in IDEA, explained for what the wire calls it. One
          // thing, three names — IDEA's *weak warning*, LSP's `Information`, our `info` — and a
          // row labelled only "Info" would leave nobody able to map it to either.
          hint="IDEA’s weak warning. Language servers report these as “information”."
          checked={inspections.severities.weakWarning}
          onChange={(weakWarning) => severity({ weakWarning })}
        />
        <ToggleRow
          label="Hints"
          hint="Suggestions rather than defects. Never counted in the status bar."
          checked={inspections.severities.hint}
          onChange={(hint) => severity({ hint })}
        />
      </Group>

      <Group title="Sources">
        <ToggleRow
          label="rust-analyzer"
          hint="Semantic diagnostics for Rust. Needs `rustup component add rust-analyzer`."
          checked={shows('rust-analyzer')}
          onChange={(on) => setSource('rust-analyzer', on)}
        />
        <ToggleRow
          label="gopls"
          hint="Semantic diagnostics for Go. Needs `go install golang.org/x/tools/gopls@latest`."
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
          hint="A headless one-shot review, run only when you ask for it. Costs tokens."
          checked={shows('claude')}
          onChange={(on) => setSource('claude', on)}
        />
        <Note title="A source not listed here is shown">
          The list is exceptions, not a registry. A language server may attribute a finding to a
          tool of its own — rust-analyzer reports clippy’s lints as <code>clippy</code> — and
          anything cide has not heard of is shown rather than silently hidden. Turn one of those
          off and it gains a row here.
        </Note>
      </Group>

      <Group title="Highlighting">
        <Row
          label="Default level"
          hint="What a newly opened editor starts at. Change it per file from the editor’s context menu."
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
        <Note title="The level is per editor, and the panel is per project">
          Turning highlighting down hides squiggles in <em>that buffer</em>. It does not change
          the Problems panel or the status bar, which count the whole project — a per-file switch
          cannot coherently redefine a project-wide total. IDEA behaves the same way. The severity
          and source toggles above <em>do</em> affect all three, because they are applied once,
          before any of them sees the list.
        </Note>
      </Group>
    </>
  )
}
