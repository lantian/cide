/**
 * The SSR entry `ui/scripts/check-ext-render.mjs` bundles and runs. (M22)
 *
 * Both halves of the extension UI in one entry, so one Vite SSR build covers both: the renderer
 * that draws whatever a worker posted, and the manager panel that installs the thing that posted
 * it. They are checked together because they fail together — a contributed panel nobody can
 * install is as useless as one that installs and draws nothing.
 *
 * The fixtures are here rather than in the check script so they are type-checked against the
 * shapes they claim to be. One `console.log` of a JSON array at the end, which is the contract
 * every other smoke entry in this repository follows.
 */
import { renderToStaticMarkup } from 'react-dom/server'

import type { Capability, InstalledExtension } from '@/ipc/client'
import { ExtPanelView } from './ExtPanelView'
import { ExtensionTab } from './ExtensionTab'
import { ExtensionSettingsView } from '@/settings/ExtensionSettings'
import type { PanelView } from './viewModel'
import { ExtensionsPanelView } from '@/sidebar/ExtensionsPanel/ExtensionsPanel'
import {
  buildModel,
  type InstalledIn,
  type LanguageIn,
  type MarketRowIn,
} from '@/sidebar/ExtensionsPanel/model'

/** One view per `ViewBody` kind. The check asserts each one draws something. */
const VIEWS: Readonly<Record<string, PanelView>> = {
  empty: { body: { kind: 'empty', message: 'Open a .sql file to see its statements.' } },
  loading: { body: { kind: 'loading', what: 'Parsing' } },
  failed: { body: { kind: 'failed', message: 'sql: it threw an error' } },
  list: {
    title: 'Statements',
    actions: [
      { id: 'refresh', label: 'Refresh' },
      { id: 'run', label: 'Run', disabled: 'no connection configured' },
    ],
    body: {
      kind: 'list',
      rows: [
        { id: '1', label: 'SELECT', detail: 'line 1', icon: 'symbol' },
        { id: '2', label: 'CREATE TABLE users', detail: 'line 9', tone: 'accent' },
      ],
    },
  },
  tree: {
    body: {
      kind: 'tree',
      rows: [
        {
          id: 'root',
          label: 'services',
          expanded: true,
          children: [
            { id: 'a', label: 'web', detail: 'line 2' },
            { id: 'b', label: 'db', detail: 'line 7' },
          ],
        },
        { id: 'closed', label: 'volumes', children: [{ id: 'v', label: 'data' }] },
      ],
    },
  },
  table: {
    body: {
      kind: 'table',
      columns: ['name', 'type'],
      rows: [
        { id: '1', cells: ['id', 'integer'] },
        // Deliberately short: the renderer pads rather than throwing, because a worker that got
        // its own column count wrong should cost a blank cell and not the panel.
        { id: '2', cells: ['email'] },
      ],
    },
  },
  markdown: { body: { kind: 'markdown', text: '# Heading\n\nSome prose.' } },
}

/**
 * The capability list the fixtures use, typed as the **wire** enum.
 *
 * `EntryIn.capabilities` is `readonly string[]`, because `model.ts` imports nothing — so a fixture
 * could hold any string at all, and for a while it held the manifest spelling while the wire
 * carried a different one. Binding it to `Capability` here means a rename on either side is a
 * compile error in this file rather than an extension that installs and never receives an editor
 * note.
 */
const ASKS: readonly Capability[] = ['editor:read', 'editor:write', 'process:spawn']

const MARKET: MarketRowIn = {
  id: 'cide-marketplace',
  name: 'cide extensions',
  source: '/home/someone/work/cide-marketplace',
  authenticated: false,
  state: { kind: 'ready' },
  entries: [
    {
      id: 'sql',
      name: 'SQL',
      version: '0.1.0',
      description: 'SQL syntax, an outline of statements, and sqls.',
      capabilities: [...ASKS],
      updateAvailable: false,
    },
    {
      id: 'yaml',
      name: 'YAML',
      version: '0.1.0',
      description: 'YAML syntax, a document outline, and yaml-language-server.',
      capabilities: [...ASKS],
      installed: '0.0.9',
      updateAvailable: true,
    },
  ],
  problems: [],
}

const INSTALLED: InstalledIn[] = [
  {
    marketplace: 'cide-marketplace',
    extension: 'yaml',
    name: 'YAML',
    version: '0.0.9',
    enabled: true,
    capabilities: ASKS,
    problems: [],
  },
]

/**
 * The resolved language table, as the panel sees it.
 *
 * A builtin that nothing displaced (`markdown`), a builtin a contribution displaced (`yaml`), and
 * a contribution that displaced nothing. The first must not be listed and the other two must be —
 * that is the whole rule this readout implements.
 */
const LANGUAGES: LanguageIn[] = [
  { id: 'markdown', label: 'Markdown', extensions: ['md'], source: null },
  {
    id: 'yaml',
    label: 'YAML',
    extensions: ['yaml', 'yml'],
    source: 'cide-marketplace.yaml',
    supersedes: null,
  },
  {
    id: 'sql',
    label: 'SQL',
    extensions: ['sql', 'ddl'],
    source: 'cide-marketplace.sql',
    supersedes: 'cide',
  },
]

const stories: { story: string; html: string }[] = []

for (const [story, view] of Object.entries(VIEWS)) {
  stories.push({
    story: `panel-${story}`,
    html: renderToStaticMarkup(
      <ExtPanelView label="SQL" view={view} onAction={() => {}} onActivate={() => {}} />,
    ),
  })
}

// The same list view with no handlers at all: every control must be *absent*, not present and
// dead. That is the panel convention, and this is the fixture that proves this renderer follows it.
stories.push({
  story: 'panel-read-only',
  html: renderToStaticMarkup(<ExtPanelView label="SQL" view={VIEWS.list as PanelView} />),
})

stories.push({
  story: 'manager-none',
  html: renderToStaticMarkup(
    <ExtensionsPanelView model={buildModel([], [], [])} onConnect={() => {}} />,
  ),
})
stories.push({
  story: 'manager-ready',
  html: renderToStaticMarkup(
    <ExtensionsPanelView
      model={buildModel([MARKET], INSTALLED, [], LANGUAGES)}
      onConnect={() => {}}
      onRefresh={() => {}}
      onDisconnect={() => {}}
      onInstall={() => {}}
      onUninstall={() => {}}
      onSetEnabled={() => {}}
      onQuery={() => {}}
      onOpen={() => {}}
    />,
  ),
})
stories.push({
  story: 'manager-filtered',
  html: renderToStaticMarkup(
    <ExtensionsPanelView
      model={buildModel([MARKET], INSTALLED, [], LANGUAGES, { text: 'sql', filter: 'enabled' })}
      onConnect={() => {}}
      onQuery={() => {}}
      onOpen={() => {}}
      onInstall={() => {}}
    />,
  ),
})
stories.push({
  story: 'manager-read-only',
  html: renderToStaticMarkup(<ExtensionsPanelView model={buildModel([MARKET], INSTALLED, [])} />),
})
stories.push({
  story: 'manager-failed',
  html: renderToStaticMarkup(
    <ExtensionsPanelView
      model={buildModel(
        [
          {
            ...MARKET,
            state: { kind: 'failed', error: 'fatal: could not read from remote repository.' },
            entries: [],
          },
        ],
        [],
        [
          {
            path: '/home/someone/.config/cide/extensions.json',
            line: 4,
            severity: 'error',
            message: 'marketplace `x` has no source and is ignored.',
          },
        ],
      )}
      onRefresh={() => {}}
    />,
  ),
})

// The extension page, in the two states that differ: one that is installed and one that is only
// listed. `useEffect` does not run under SSR, so `page` is null and the body draws its Reading
// notice — which is exactly the state worth pinning, because it is what a user sees first and it
// is the one a `null` guard omitted would render as a blank tab.
stories.push({
  story: 'page',
  html: renderToStaticMarkup(
    <ExtensionTab id={{ marketplace: 'cide-marketplace', extension: 'sql' }} name="SQL" />,
  ),
})

/*
 * The Settings ▸ Extensions page, with one setting of every kind.
 *
 * The *view*, with props, and not the store-reading host: zustand's `useSyncExternalStore` reads
 * `getInitialState` on a server pass, so a component that took its rows from the store would
 * render the empty screen here whatever this fixture put in it. `ExtensionSettings.tsx`'s header
 * says the same from the other end.
 */
const SHOWCASE: InstalledExtension = {
  marketplace: 'cide-marketplace',
  extension: 'showcase',
  name: 'Showcase',
  version: '0.1.0',
  description: '',
  enabled: true,
  commit: 'abc',
  capabilities: [...ASKS],
  main: 'main.js',
  path: '/tmp/showcase',
  problems: [],
  // Resolved values, which is the shape Rust actually sends: complete, and already coerced.
  settings: { greeting: 'Hello', rows: 6, density: 'detailed', log: false },
  contributes: {
    languages: [],
    languageServers: [],
    panels: [],
    commands: [],
    settings: [
      {
        id: 'greeting',
        label: 'Greeting',
        description: 'A text setting.',
        kind: { type: 'text', default: 'Hello', placeholder: 'Say something' },
      },
      { id: 'rows', label: 'Rows in the list', kind: { type: 'number', default: 6, min: 1, max: 20 } },
      {
        id: 'density',
        label: 'Detail',
        kind: {
          type: 'choice',
          default: 'detailed',
          choices: [
            { value: 'plain', label: 'Plain' },
            { value: 'detailed', label: 'Detailed' },
          ],
        },
      },
      { id: 'log', label: 'Log every note', kind: { type: 'toggle', default: false } },
    ],
  },
}

stories.push({
  story: 'settings',
  html: renderToStaticMarkup(
    <ExtensionSettingsView extensions={[SHOWCASE]} onChange={() => {}} />,
  ),
})
// Nothing installed at all, and nothing installed *with settings*, are two different sentences
// and only one of them suggests installing something.
stories.push({
  story: 'settings-none',
  html: renderToStaticMarkup(<ExtensionSettingsView extensions={[]} />),
})
stories.push({
  story: 'settings-nothing-to-configure',
  html: renderToStaticMarkup(
    <ExtensionSettingsView
      extensions={[{ ...SHOWCASE, contributes: { ...SHOWCASE.contributes, settings: [] } }]}
    />,
  ),
})

console.log(JSON.stringify(stories))
