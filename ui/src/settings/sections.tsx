/**
 * The seven sections of the Settings tab, exactly the mock's list and in its order.
 *
 * Each section is a pure function of the settings it renders plus a `patch` callback. None of
 * them reads the store: `SettingsTab` does the wiring, which is what lets the whole screen be
 * driven from a fixture and keeps every section's props visible in one place.
 *
 * The wording of the toggles is the mock's, with one deliberate exception noted on
 * `keepSessionsOnWindowClose` below.
 */
import type { ReactNode } from 'react'
import type {
  ClaudeCliSupport,
  ClaudeSettings,
  EditorSettings,
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
    description: 'The environment every claude pane is spawned with.',
  },
  { id: 'editor', title: 'Editor', description: 'The code buffer.' },
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
      </Group>

      <Note title="Applied at spawn">
        These reach a pane's child process when it starts. A pane already running keeps the
        environment it was spawned with until it is restarted.
      </Note>
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
      </Group>
    </>
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
    case 'git':
      return <Git />
    case 'terminal':
      return <Terminal {...props} />
  }
}
