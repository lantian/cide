/**
 * What Settings search searches. (M133)
 *
 * # Why a written-down list, and not the rendered page
 *
 * The obvious index is the DOM: render every section, read every `data-setting`. It loses on
 * three counts. Most sections are not mounted — only the current page is — so the index would
 * need all thirteen rendered off-screen, and two of them (Agents, Keymap) fetch their own state
 * and spawn nothing useful doing it. A row's label is a poor search key on its own: nobody
 * looking for "proxy" types "Mode", and nobody looking for the wheel types "Scroll speed". And
 * a row that is only drawn in some state (a role's form, a provider's key) is exactly the one a
 * user searches for because they cannot see it.
 *
 * So the index is this list: the page, the tab, the row's label exactly as drawn (search scrolls
 * to `[data-setting="<label>"]`, which `controls.tsx`'s `Row` sets from the label), and the words
 * someone would type for it. `check:settings-copy` fails when a literal `Row`/`ToggleRow` label
 * in the settings sources has no entry here, so a new row cannot quietly be unfindable.
 */
import type { SettingsSection } from '@/ipc/client'

export interface SettingsHit {
  section: SettingsSection
  /** The page's tab the row is on, for a page in `SECTION_TABS`. */
  tab?: string
  /** The row's label, exactly as drawn — and its `data-setting`. */
  label: string
  /** Other words for it: the env var it sets, what it is about, what people call it. */
  keywords?: string
}

export const SETTINGS_INDEX: readonly SettingsHit[] = [
  { section: 'projectsAndWindows', label: 'New-task proposals', keywords: 'openspec propose task background tab run' },
  // --- Pages drawn in sections.tsx ---
  // Appearance
  { section: 'appearance', label: 'Theme', keywords: 'dark light mode colours' },
  { section: 'appearance', label: 'Waiting pane highlight', keywords: 'awaiting attention notification finished ready accent rail tinted header outline harness session' },
  { section: 'appearance', label: 'UI font size', keywords: 'zoom scale text size interface' },
  { section: 'appearance', label: 'Version', keywords: 'about build release check for updates' },
  { section: 'appearance', label: 'Log directory', keywords: 'logs diagnostics debug report' },
  // Projects & windows
  { section: 'projectsAndWindows', label: 'Each project keeps its own console tab', keywords: 'pinned console' },
  { section: 'projectsAndWindows', label: 'Reopen the last project on launch', keywords: 'restore startup session' },
  { section: 'projectsAndWindows', label: 'Check for updates on start', keywords: 'update release github auto' },
  { section: 'projectsAndWindows', label: 'Skipping a version', keywords: 'skip update release offer again' },
  { section: 'projectsAndWindows', label: 'Keep sessions running when a project window is closed', keywords: 'background close window' },
  { section: 'projectsAndWindows', label: 'Confirm before closing a project with a live session', keywords: 'close prompt warning dialog' },
  { section: 'projectsAndWindows', label: 'Open a run in', keywords: 'agent subagent run tab split view' },
  // Harness
  { section: 'claudeSessions', tab: 'launch', label: 'Override global launch', keywords: 'project local binary arguments environment inheritance' },
  { section: 'claudeSessions', tab: 'launch', label: 'Inherited launch', keywords: 'project global default binary' },
  { section: 'claudeSessions', tab: 'console', label: 'Harness', keywords: 'claude codex opencode cli console which default global project override' },
  { section: 'claudeSessions', tab: 'console', label: 'Last handshake', keywords: 'ide protocol claude verified' },
  { section: 'claudeSessions', tab: 'console', label: 'Disable mouse reporting', keywords: 'CLAUDE_CODE_DISABLE_MOUSE selection' },
  { section: 'claudeSessions', tab: 'console', label: 'Resume every Harness pane on launch', keywords: 'restore conversation startup' },
  { section: 'claudeSessions', tab: 'console', label: 'Full repaint on the alternate screen', keywords: 'CLAUDE_CODE_ALT_SCREEN_FULL_REPAINT debris resize' },
  { section: 'claudeSessions', tab: 'console', label: 'Disable the alternate screen', keywords: 'CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN fullscreen scrollback' },
  { section: 'claudeSessions', tab: 'console', label: 'Scroll speed', keywords: 'CLAUDE_CODE_SCROLL_SPEED wheel mouse' },
  // Editor
  { section: 'editor', label: 'Font size', keywords: 'editor text size' },
  { section: 'editor', label: 'Wrap long lines', keywords: 'word wrap soft wrap' },
  { section: 'editor', label: 'Show the minimap', keywords: 'overview scrollbar' },
  { section: 'editor', label: 'Tab size', keywords: 'indent width' },
  { section: 'editor', label: 'Insert spaces', keywords: 'tabs indentation' },
  { section: 'editor', label: 'Detect indentation from the file', keywords: 'tabs spaces auto indent' },
  { section: 'editor', label: 'Save automatically', keywords: 'autosave auto save focus' },
  { section: 'editor', label: 'Trim trailing whitespace on save', keywords: 'whitespace strip' },
  { section: 'editor', label: 'Suggest completions', keywords: 'autocomplete intellisense language server' },
  { section: 'editor', label: 'Open suggestions while typing', keywords: 'autocomplete popup ctrl+space' },
  // Files
  { section: 'files', label: 'Show hidden files', keywords: 'dotfiles dot .env tree explorer' },
  { section: 'files', label: 'Show ignored files', keywords: 'gitignore target node_modules tree explorer indexing slow' },
  // Inspections
  { section: 'inspections', label: 'Errors', keywords: 'severity diagnostics problems' },
  { section: 'inspections', label: 'Warnings', keywords: 'severity diagnostics problems' },
  { section: 'inspections', label: 'Weak warnings', keywords: 'severity information diagnostics' },
  { section: 'inspections', label: 'Hints', keywords: 'severity suggestions diagnostics' },
  { section: 'inspections', label: 'rust-analyzer', keywords: 'rust language server source diagnostics' },
  { section: 'inspections', label: 'gopls', keywords: 'go language server source diagnostics' },
  { section: 'inspections', label: 'tree-sitter', keywords: 'syntax errors source' },
  { section: 'inspections', label: 'Claude inspections', keywords: 'review ai tokens source' },
  { section: 'inspections', label: 'Which rust-analyzer runs', keywords: 'built-in system path binary language server' },
  { section: 'inspections', label: 'Which gopls runs', keywords: 'built-in system path binary language server' },
  { section: 'inspections', label: 'Memory limit (MiB)', keywords: 'watchdog restart ram GOMEMLIMIT language server' },
  { section: 'inspections', label: 'Index working set (%)', keywords: 'rust-analyzer memory disk index' },
  { section: 'inspections', label: 'Default level', keywords: 'highlighting squiggles' },
  // Git
  { section: 'git', label: 'When your branch has diverged', keywords: 'pull merge rebase fast-forward update project pull.rebase' },
  { section: 'git', label: 'Apply non-conflicting changes automatically', keywords: 'merge conflict resolver' },
  // Terminal
  { section: 'terminal', label: 'Font size', keywords: 'terminal text size' },
  { section: 'terminal', label: 'Scrollback', keywords: 'history lines buffer' },
  { section: 'terminal', label: 'Renderer', keywords: 'webgl dom gpu' },
  { section: 'terminal', label: 'Render JSON log lines', keywords: 'structured logs json' },
  { section: 'terminal', label: 'Announce a job after (seconds)', keywords: 'notification long running command threshold' },
  { section: 'claudeSessions', tab: 'console', label: 'Version', keywords: 'claude codex cli version untested' },
  // --- Harness → Launch and Proxy ---
  // Harness › Launch (ClaudeCliSection / CodexCliSection — one is drawn at a time, per consoleHarness)
  { section: 'claudeSessions', tab: 'launch', label: 'Program', keywords: 'binary claude codex path executable wrapper mise asdf shim cli' },
  { section: 'claudeSessions', tab: 'launch', label: 'Resolves to', keywords: 'binary path version which resolved' },
  { section: 'claudeSessions', tab: 'launch', label: 'Default permissions', keywords: 'codex permission permissions approve for me approval sandbox full access read only default' },
  { section: 'claudeSessions', tab: 'launch', label: 'Permission profile', keywords: 'codex custom named permissions profile config' },
  // Harness › Proxy (ProxySection)
  { section: 'claudeSessions', tab: 'proxy', label: 'Mode', keywords: 'proxy inherit manual direct no proxy network' },
  { section: 'claudeSessions', tab: 'proxy', label: 'HTTP proxy', keywords: 'HTTP_PROXY http_proxy address url network' },
  { section: 'claudeSessions', tab: 'proxy', label: 'HTTPS proxy', keywords: 'HTTPS_PROXY https_proxy address url network' },
  { section: 'claudeSessions', tab: 'proxy', label: 'ALL_PROXY', keywords: 'all_proxy socks proxy address url' },
  { section: 'claudeSessions', tab: 'proxy', label: 'Bypass', keywords: 'NO_PROXY no_proxy proxy exceptions loopback' },
  { section: 'claudeSessions', tab: 'proxy', label: 'Consoles and agents', keywords: 'proxy scope claude codex agent runs one-shots' },
  { section: 'claudeSessions', tab: 'proxy', label: 'Shell panes', keywords: 'proxy scope shell terminal curl npm' },
  { section: 'claudeSessions', tab: 'proxy', label: 'cide’s own Git', keywords: 'proxy scope git push fetch pull' },
  // --- Agents ---
  // Agents — This project
  { section: 'agents', tab: 'project', label: 'Task tracker & milestones', keywords: 'board tasks milestones openspec config.json tracker' },
  { section: 'agents', tab: 'project', label: 'Concurrent runs', keywords: 'parallel max concurrent subagents runs at once limit' },
  { section: 'agents', tab: 'project', label: 'Comment limit, characters', keywords: 'comment length report task subagent' },
  { section: 'agents', tab: 'project', label: 'Continue interrupted runs when cide starts', keywords: 'resume restart relaunch interrupted runs' },
  // Agents — Review & wake
  { section: 'agents', tab: 'spawn', label: 'Review finished runs in a new tab', keywords: 'reviewer review tab finished run console' },
  { section: 'agents', tab: 'spawn', label: 'Review finished work', keywords: 'batch one by one review mode reviewer' },
  { section: 'agents', tab: 'spawn', label: 'Tasks per review', keywords: 'batch size review' },
  { section: 'agents', tab: 'spawn', label: 'Review after, seconds', keywords: 'batch wait timeout review' },
  { section: 'agents', tab: 'spawn', label: 'Verify failures before the orchestrator takes over', keywords: 'verify retries red ci gate orchestrator' },
  { section: 'agents', tab: 'spawn', label: 'Wake this project when it goes quiet', keywords: 'auto spin idle wake orchestrator autonomy' },
  { section: 'agents', tab: 'spawn', label: 'Quiet for', keywords: 'idle minutes auto spin wake' },
  // Agents — Local overrides (the rows repeat per override card; search lands on the first)
  { section: 'agents', tab: 'overrides', label: 'Harness', keywords: 'override redirect cli opencode codex claude this machine' },
  { section: 'agents', tab: 'overrides', label: 'Model choice', keywords: 'override pool model' },
  { section: 'agents', tab: 'overrides', label: 'Pool', keywords: 'override model pool fallback' },
  { section: 'agents', tab: 'overrides', label: 'Permission mode', keywords: 'override auto bypassPermissions manual prompts' },
  { section: 'agents', tab: 'overrides', label: 'Concurrent runs of this role', keywords: 'override max concurrent parallel local model' },
  // --- Appearance parts, Keymap, Models, Extensions, Git (GitLab), Remote access ---
  // appearance (AccentRow, ColorSchemeRow, GraphicsLadder)
  { section: 'appearance', label: 'Accent colour', keywords: 'color brand red highlight focus' },
  { section: 'appearance', label: 'Colour scheme', keywords: 'color theme syntax vsix vs code import highlighting' },
  { section: 'appearance', label: 'Workaround ladder', keywords: 'graphics webkit rendering gpu nvidia dmabuf compositing CIDE_NO_GRAPHICS_WORKAROUNDS' },
  { section: 'appearance', label: 'Disable NVIDIA explicit sync', keywords: 'graphics __NV_DISABLE_EXPLICIT_SYNC hang driver' },
  { section: 'appearance', label: 'Disable the DMABUF renderer', keywords: 'graphics WEBKIT_DISABLE_DMABUF_RENDERER wayland' },
  { section: 'appearance', label: 'Disable accelerated compositing', keywords: 'graphics WEBKIT_DISABLE_COMPOSITING_MODE slow blank window' },
  // models (ModelsSection: rows inside a provider card, drawn while it is open)
  { section: 'models', label: 'Enabled', keywords: 'provider opencode off disable' },
  { section: 'models', label: 'API key', keywords: 'provider credential token secret opencode' },
  { section: 'models', label: 'Base URL', keywords: 'provider endpoint custom ollama openai-compatible' },
  { section: 'models', label: 'Setup steps', keywords: 'external codex plugin chatgpt provider' },
  // remote (RemoteSection)
  { section: 'remote', label: 'Let a phone connect to this cide', keywords: 'remote access enable phone mobile' },
  { section: 'remote', label: 'Reachable from', keywords: 'bind loopback network interface lan' },
  { section: 'remote', label: 'Port', keywords: 'listen socket remote' },
  { section: 'remote', label: 'Allow a public address', keywords: 'internet routable bind non-private' },
  { section: 'remote', label: 'Listener', keywords: 'status listening address connected devices' },
  { section: 'remote', label: 'Pair a device', keywords: 'pairing qr code phone' },
  // git (GitLabSettings)
  { section: 'git', label: 'Connect an account', keywords: 'gitlab token personal access host login' },
  { section: 'git', label: 'Hide matching files in all MR reviews', keywords: 'gitlab merge request exclude glob' },
  { section: 'git', label: 'Patterns', keywords: 'gitlab review excluded files glob' },
  { section: 'git', label: 'Default instructions', keywords: 'gitlab agent review prompt instructions' },
]

/** The entries whose label or keywords contain every word of the query, best first. */
export function searchSettings(query: string): SettingsHit[] {
  const words = query.toLowerCase().split(/\s+/).filter((w) => w !== '')
  if (words.length === 0) return []
  const scored: { hit: SettingsHit; score: number }[] = []
  for (const hit of SETTINGS_INDEX) {
    const label = hit.label.toLowerCase()
    const all = `${label} ${hit.section.toLowerCase()} ${(hit.keywords ?? '').toLowerCase()}`
    if (!words.every((w) => all.includes(w))) continue
    // A label that starts with the query outranks one that contains it, which outranks a
    // keyword-only match: "font" should put Font size above a row that merely mentions fonts.
    const first = words[0] ?? ''
    const score = label.startsWith(first) ? 0 : words.every((w) => label.includes(w)) ? 1 : 2
    scored.push({ hit, score })
  }
  return scored.sort((a, b) => a.score - b.score).map((s) => s.hit)
}
