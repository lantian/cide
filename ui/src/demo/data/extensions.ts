/**
 * The demo's extension registry: two marketplaces, a mix of installed, disabled, updatable and
 * not-yet-installed entries, and the Godot extension's own page.
 *
 * Contributions are kept to what draws without a running extension host: languages, a language
 * server, commands and settings. A contributed *panel* is left out on purpose — it would put an
 * icon on the rail whose click loads an extension bundle the demo does not have.
 */
import type {
  Capability,
  ExtensionPage,
  ExtensionSnapshot,
  InstalledExtension,
  LanguageBinding,
  Marketplace,
  MarketplaceEntry,
  ResolvedContributions,
  ServerBinding,
} from '../../ipc/generated'
import { wire } from '../world'

const COMMUNITY = 'community'
const TEAM = 'team'

function entry(
  id: string,
  name: string,
  version: string,
  description: string,
  capabilities: Capability[],
  extra: Partial<MarketplaceEntry> = {},
): MarketplaceEntry {
  return { id, name, version, description, capabilities, updateAvailable: false, ...extra }
}

const COMMUNITY_ENTRIES: MarketplaceEntry[] = [
  entry('godot', 'Godot', '1.4.0', 'GDScript highlighting and folding, and the Godot editor’s own language server over its TCP port.', ['lsp:connect', 'fs:read'], { installed: '1.4.0' }),
  entry('sql-tools', 'SQL Tools', '0.10.0', 'Dialect-aware SQL for Postgres and SQLite, with sqls for completion against a live schema.', ['process:spawn', 'lsp:connect'], { installed: '0.9.2', updateAvailable: true }),
  entry('todo-tree', 'TODO Tree', '2.0.1', 'Every TODO, FIXME and HACK in the project, grouped by file, with a command to jump through them.', ['fs:read', 'editor:read'], { installed: '2.0.1' }),
  entry('zig', 'Zig', '0.3.0', 'Zig and ZON files, and zls when it is on PATH.', ['process:spawn', 'lsp:connect'], { installed: '0.3.0' }),
  entry('terraform', 'Terraform', '0.6.1', 'HCL highlighting and terraform-ls: go-to-definition across modules, validation on save.', ['process:spawn', 'lsp:connect']),
  entry('protobuf', 'Protocol Buffers', '1.1.0', '.proto files, and buf’s language server for imports and lint.', ['process:spawn', 'lsp:connect']),
  entry('nix', 'Nix', '0.2.4', 'Nix expressions and flakes, with nil for completion and diagnostics.', ['process:spawn', 'lsp:connect']),
  entry('justfile', 'just', '0.1.3', 'Syntax for justfiles, and a command that runs a recipe in a new terminal pane.', ['fs:read']),
  entry('k8s', 'Kubernetes manifests', '0.4.0', 'Schema-checked Kubernetes YAML.', ['process:spawn'], { unavailable: 'needs cide 0.10 or newer' }),
]

const TEAM_ENTRIES: MarketplaceEntry[] = [
  entry('review-checklist', 'Review checklist', '0.4.0', 'The team’s review checklist beside every diff, ticked per file.', ['git:read', 'editor:read'], { installed: '0.4.0' }),
  entry('flamegraph', 'Flamegraph viewer', '0.2.0', 'Opens perf and cargo-flamegraph output as an interactive flame graph.', ['fs:read', 'process:spawn']),
]

export const MARKETPLACES: Marketplace[] = [
  {
    id: COMMUNITY,
    name: 'community',
    source: 'https://github.com/cide-ide/marketplace.git',
    authenticated: true,
    state: { kind: 'ready', head: '7c1e2f9a4b', fetchedAt: wire(1790260000000) },
    path: '/home/dev/.local/share/cide/marketplaces/community',
    entries: COMMUNITY_ENTRIES,
    problems: [],
  },
  {
    id: TEAM,
    name: 'team tools',
    source: '/home/dev/work/cide-extensions',
    authenticated: false,
    state: { kind: 'ready', head: 'e04b91d27c', fetchedAt: wire(1790261200000) },
    path: '/home/dev/work/cide-extensions',
    entries: TEAM_ENTRIES,
    problems: [],
  },
]

const NO_CONTRIBUTIONS = { languages: [], languageServers: [], panels: [], commands: [], settings: [] }

function installed(marketplace: string, e: MarketplaceEntry, enabled: boolean, extra: Partial<InstalledExtension> = {}): InstalledExtension {
  return {
    name: e.name,
    version: e.installed ?? e.version,
    description: e.description,
    enabled,
    railIcon: false,
    commit: marketplace === COMMUNITY ? '7c1e2f9a4b' : 'e04b91d27c',
    capabilities: e.capabilities,
    contributes: NO_CONTRIBUTIONS,
    settings: {},
    path: `/home/dev/.local/share/cide/extensions/${marketplace}/${e.id}`,
    problems: [],
    marketplace,
    extension: e.id,
    ...extra,
  }
}

const byId = (id: string): MarketplaceEntry => {
  const found = [...COMMUNITY_ENTRIES, ...TEAM_ENTRIES].find((e) => e.id === id)
  if (!found) throw new Error(`no demo extension ${id}`)
  return found
}

export const INSTALLED: InstalledExtension[] = [
  installed(COMMUNITY, byId('godot'), true, {
    contributes: {
      ...NO_CONTRIBUTIONS,
      commands: [
        { id: 'godot.runScene', title: 'Godot: Run current scene', keywords: ['play', 'f6'] },
        { id: 'godot.reconnect', title: 'Godot: Reconnect language server', keywords: ['lsp'] },
      ],
      settings: [
        { id: 'port', label: 'Language server port', description: 'The Godot editor’s network/language_server/remote_port.', kind: { type: 'number', default: 6005, min: 1024, max: 65535 } },
      ],
    },
    settings: { port: 6005 },
  }),
  installed(COMMUNITY, byId('sql-tools'), true),
  installed(COMMUNITY, byId('todo-tree'), true),
  installed(COMMUNITY, byId('zig'), false),
  installed(TEAM, byId('review-checklist'), true),
]

/**
 * The registry after layering: the builtins the bootstrap already carries, plus what the two
 * language extensions add. Each contributed language borrows a builtin's grammar and fold spec,
 * relabelled — close enough in shape for highlighting, and the panel only reads the label, the
 * file extensions and who contributed it.
 */
export function resolved(base: ResolvedContributions): ResolvedContributions {
  const like = (id: string) => base.languages.find((l) => l.def.id === id)
  const python = like('python')
  const sql = like('sql')
  const extra: LanguageBinding[] = []
  if (python) {
    extra.push({
      def: { ...python.def, id: 'gdscript', label: 'GDScript', extensions: [{ ext: 'gd' }], filenames: [], fenceAliases: ['gdscript', 'gd'], scratch: [] },
      source: { kind: 'extension', extension: { marketplace: COMMUNITY, extension: 'godot' } },
    })
  }
  const languages = base.languages.map((l): LanguageBinding =>
    l.def.id === 'sql' && sql
      ? { ...l, def: { ...l.def, label: 'SQL (Postgres, SQLite)', extensions: [{ ext: 'sql' }, { ext: 'psql', label: 'PostgreSQL' }] }, source: { kind: 'extension', extension: { marketplace: COMMUNITY, extension: 'sql-tools' } }, supersedes: { kind: 'builtin' } }
      : l,
  )
  const godotServer: ServerBinding = {
    def: {
      binary: '',
      args: [],
      languageIds: ['gdscript'],
      projectMarkers: ['project.godot'],
      projectKind: 'Godot project',
      installHint: 'Open the project in the Godot editor; it serves the language server itself.',
      declaresWatchedFiles: false,
      extraPathHints: [],
      connect: { host: '127.0.0.1', port: 6005 },
    },
    source: { kind: 'extension', extension: { marketplace: COMMUNITY, extension: 'godot' } },
  }
  return {
    ...base,
    languages: [...languages, ...extra],
    servers: [...base.servers, godotServer],
    commands: [
      ...base.commands,
      { id: 'godot.runScene', title: 'Godot: Run current scene', keywords: ['play'], extension: { marketplace: COMMUNITY, extension: 'godot' } },
      { id: 'todo-tree.next', title: 'TODO Tree: Next TODO', keywords: ['fixme'], extension: { marketplace: COMMUNITY, extension: 'todo-tree' } },
    ],
  }
}

export function snapshot(base: ResolvedContributions): ExtensionSnapshot {
  return { rev: wire(7), marketplaces: MARKETPLACES, extensions: INSTALLED, resolved: resolved(base), problems: [] }
}

const GODOT_README = `# Godot for cide

GDScript as a first-class language: highlighting, folding, and the **Godot editor's own language
server** — the one that already knows your scene tree, autoloads and signals.

## How it connects

Godot serves its language server on a TCP port while the editor is open. This extension does not
spawn anything: it connects to \`127.0.0.1:6005\` when a folder with a \`project.godot\` is open,
and reconnects when the editor restarts.

| Godot | Default port | Setting |
| --- | --- | --- |
| 4.x | 6005 | \`network/language_server/remote_port\` |
| 3.x | 6008 | same |

## What you get

- Go to definition across scripts, including autoloads and \`class_name\` globals
- Completion for nodes by path: \`$Player/AnimationTree\`
- Diagnostics from the editor's parser, in the Problems panel
- **Godot: Run current scene** — plays the scene of the open script in the editor

## Permissions

- *Connect to a language server* — the Godot editor's, on loopback only
- *Read files in the project* — to find \`project.godot\` and resolve \`res://\` paths

## Settings

\`port\` — change it if you moved the editor's language server off 6005.
`

export function godotPage(): ExtensionPage {
  const e = byId('godot')
  return {
    id: { marketplace: COMMUNITY, extension: 'godot' },
    entry: e,
    ...(INSTALLED[0] ? { installed: INSTALLED[0] } : {}),
    readme: GODOT_README,
    readmePath: '/home/dev/.local/share/cide/extensions/community/godot/README.md',
  }
}
