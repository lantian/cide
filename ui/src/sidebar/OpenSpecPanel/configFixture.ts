/**
 * Stories for the OpenSpec configuration form and its set-up wizard. (M28)
 *
 * Fixture data only — `fixture.ts` next door is the panel's and is not this change's to edit, so
 * this is its twin for the config surface. Driven by `ConfigSmokeEntry.tsx` under
 * `check-openspec-config.mjs`; nothing in the app imports it, so it is tree-shaken out of the
 * real bundle.
 *
 * Every story is a *state the screen can really be in*, and three of them are states that only
 * exist because something failed: a project whose file states a schema the CLI did not list, a
 * project where the CLI could not be asked at all, and a save that was rejected. Those are the
 * ones a fixture is for — the happy path is visible every time somebody opens the tab.
 */
import type { ConfigFormProps, ConfigWizardProps } from './ConfigForm'
import { draftFrom, type ConfigLike, type SchemaLike } from './configModel'

const noop = () => {}

export const SPEC_DRIVEN: SchemaLike = {
  name: 'spec-driven',
  description: 'Default OpenSpec workflow - proposal → specs → design → tasks',
  artifacts: ['proposal', 'specs', 'design', 'tasks'],
  source: 'package',
}

const LEAN: SchemaLike = {
  name: 'lean',
  description: 'Proposal and tasks only.',
  artifacts: ['proposal', 'tasks'],
  source: 'project',
}

/** What `openspec init` leaves behind: a schema line and three commented-out examples. */
export const BARE: ConfigLike = {
  schema: null,
  defaultSchema: 'spec-driven',
  context: null,
  rules: [],
  operations: [],
  path: '/home/dev/work/thing/openspec/config.yaml',
  exists: true,
}

/** A project somebody has actually configured. */
export const CONFIGURED: ConfigLike = {
  schema: 'spec-driven',
  defaultSchema: 'spec-driven',
  context: 'Tech stack: Rust + Tauri 2, React 19.\nLinux-first.',
  rules: [
    { artifact: 'proposal', rules: ['Keep it under one page.'] },
    { artifact: 'design', rules: ['Name the option that lost.', 'No new dependencies.'] },
  ],
  operations: [{ operation: 'archive', guidance: ['Re-run validate --all first.'] }],
  path: '/home/dev/work/thing/openspec/config.yaml',
  exists: true,
}

/** A schema the CLI did not list — the select must still be able to show it. */
const UNLISTED: ConfigLike = {
  ...CONFIGURED,
  schema: 'house-style',
  rules: [{ artifact: 'brief', rules: ['One paragraph.'] }],
}

function form(over: Partial<ConfigFormProps> & { config: ConfigLike }): ConfigFormProps {
  return {
    schemas: [SPEC_DRIVEN, LEAN],
    draft: draftFrom(over.config),
    onDraft: noop,
    dirty: false,
    status: 'idle',
    error: null,
    cliNote: null,
    onSave: noop,
    onRevert: noop,
    onOpenFile: noop,
    ...over,
  }
}

export const CONFIG_STORIES = {
  /** The file `init` wrote and nobody has touched. */
  bare: form({ config: BARE }),
  /** A configured project, clean. */
  configured: form({ config: CONFIGURED }),
  /** Edited but not yet saved. */
  dirty: form({
    config: CONFIGURED,
    draft: { ...draftFrom(CONFIGURED), context: 'Tech stack: Rust.' },
    dirty: true,
  }),
  /** A Save in flight: everything inert, nothing that can be pressed twice. */
  saving: form({ config: CONFIGURED, dirty: true, status: 'saving' }),
  /** A Save that found the file already saying exactly this. */
  unchanged: form({ config: CONFIGURED, status: 'unchanged' }),
  /** A rejection, as a sentence from Rust rather than `String(e)`. */
  rejected: form({
    config: CONFIGURED,
    dirty: true,
    error: '/home/dev/work/thing/openspec/config.yaml could not be written: Permission denied',
  }),
  /** A schema the CLI never listed. The select must still show the project's own value. */
  unlisted: form({ config: UNLISTED }),
  /**
   * The CLI answered, and there is one schema — which is almost every project, because upstream
   * ships exactly one. Its pair is `configured`, where a second schema exists and the sentence
   * about forking one must not appear; and `noCli` below, where the single row is cide's fallback
   * rather than the CLI's answer and the sentence would be a claim about a list nobody read.
   */
  onlySchema: form({ config: CONFIGURED, schemas: [SPEC_DRIVEN] }),
  /** No CLI at all: the form still works, and says what is degraded. */
  noCli: form({
    config: CONFIGURED,
    schemas: [SPEC_DRIVEN],
    cliNote: 'openspec is not on the PATH this app was launched with.',
  }),
} satisfies Record<string, ConfigFormProps>

export type ConfigStoryName = keyof typeof CONFIG_STORIES

function wizard(over: Partial<ConfigWizardProps> = {}): ConfigWizardProps {
  return {
    path: '/home/dev/work/thing/openspec',
    hint: 'OpenSpec keeps proposed work in openspec/changes/ and settled behaviour in openspec/specs/.',
    context: '',
    onContext: noop,
    busy: false,
    error: null,
    onSetUp: noop,
    ...over,
  }
}

export const WIZARD_STORIES = {
  /** Nothing typed: the button says it is about to skip. */
  empty: wizard(),
  /** Something typed: the button says it is about to write it. */
  filled: wizard({ context: 'Tech stack: Rust + Tauri 2, React 19.' }),
  /** `openspec init` is running. */
  busy: wizard({ context: 'Tech stack: Rust.', busy: true }),
  /** It refused, and the sentence is drawn. */
  failed: wizard({
    error: '`openspec init` could not be started: No such file or directory (os error 2)',
  }),
} satisfies Record<string, ConfigWizardProps>

export type WizardStoryName = keyof typeof WIZARD_STORIES
