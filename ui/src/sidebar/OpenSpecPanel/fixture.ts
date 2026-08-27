/**
 * The OpenSpec panel's named states, as plain props. (M28)
 *
 * Deterministic by construction — no clock, no minted ids, no `Math.random` — because
 * `check:openspec-render` compares a digest and anything that varied per run would fail for a
 * reason with nothing to do with the panel.
 *
 * # The pairs are the point
 *
 * `unknown` and `absent` differ in **one** thing: whether anybody has asked. That is the pair the
 * render check exists for, because the difference is invisible in the model and enormous on
 * screen — the absent card offers a button that writes a tracked directory into somebody's
 * repository, and drawing it while nothing has been read is proposing a commit on the strength of
 * not knowing. `cli-missing` and `unusable` are one board arm with two sentences, which is the
 * whole reason Rust builds the sentence rather than the panel.
 */
import type { OpenSpecPanelViewProps } from './OpenSpecPanel'
import type { ProposeFormProps } from './ProposeForm'
import type { RequirementEditorProps } from './RequirementEditor'
import type { SpecTabViewProps } from './SpecTab'
import type { ChangeView } from './model'
import type { Board } from './model'
import { rowAction, splitAction } from './model'

/**
 * Every handler, on every story, so an absence assertion is never vacuous.
 *
 * `onConfigure` is here for exactly that reason: the gear is withheld by the *host*, on the
 * `unknown` arm alone, and a fixture that omitted the callback would make "no gear on `unknown`"
 * true of every story and prove nothing. The `board-unknown` story overrides it back to
 * `undefined`, which is the one place the absence is the claim.
 */
const HANDLERS = {
  onToggleSection: () => {},
  onOpenChange: () => {},
  onRowAction: () => {},
  onOpenSpec: () => {},
  onSetUp: () => {},
  onAsk: () => {},
  onRetry: () => {},
  onConfigure: () => {},
}

const READY: Board = {
  kind: 'ready',
  root: '/home/dev/work/thing',
  /*
   * The skill spelling, because that is what `openspec init --tools claude` writes today and
   * what a screenshot of this panel has to show. The board carries it rather than the panel
   * spelling it: cide typed `/opsx:propose` at projects that had no such command for a year.
   * `board-legacy` below is the same panel on a project set up by an older CLI.
   */
  commands: [
    { name: 'explore', line: '/openspec-explore' },
    { name: 'propose', line: '/openspec-propose' },
  ],
  specs: [
    { id: 'dark-mode', requirements: 4 },
    { id: 'auth', requirements: 7 },
  ],
  changes: [
    { name: 'add-dark-mode', completed: 3, total: 9, status: 'in-progress' },
    { name: 'rework-auth', completed: 0, total: 0, status: 'no-tasks' },
    { name: 'drop-legacy-theme', completed: 5, total: 5, status: 'complete' },
  ],
}

/**
 * Which change already has a task, so the row action's two states are both drawn somewhere.
 *
 * `add-dark-mode` has one and the other two do not — the pair is the point, because a *Start
 * work* on a change that is already being worked would create a second task for it.
 */
const TASKS: Readonly<Record<string, string>> = { 'add-dark-mode': 't-14' }

function story(board: Board, over: Partial<OpenSpecPanelViewProps> = {}): OpenSpecPanelViewProps {
  return {
    board,
    expanded: {},
    busy: false,
    // Ready unless a story says otherwise — the stories that matter here are about the *spec*
    // board, and a tracker nobody has read would grey every row action in all of them.
    tracker: 'ready',
    asking: null,
    tasks: TASKS,
    ...HANDLERS,
    ...over,
  }
}

export type SpecStoryName =
  | 'unknown'
  | 'absent'
  | 'absent-busy'
  | 'cli-missing'
  | 'unusable'
  | 'board'
  | 'board-empty'
  | 'board-collapsed'
  | 'board-tracker-unread'
  | 'ask-propose'
  | 'ask-propose-legacy'
  | 'ask-explore'

export const SPEC_STORIES: Record<SpecStoryName, OpenSpecPanelViewProps> = {
  /**
   * Nobody has looked. Draws nothing at all — see `Board`'s doc.
   *
   * And no configuration gear: `onConfigure` is `undefined` here and on no other story, because a
   * control offering to configure OpenSpec for a project whose state cide cannot yet describe is
   * a control that does not know what it would open. Every other arm draws it, `absent` included
   * — there the dialog is the init wizard.
   */
  unknown: story({ kind: 'unknown' }, { onConfigure: undefined }),

  absent: story({
    kind: 'absent',
    hint: 'OpenSpec keeps this project’s requirements in openspec/specs/, and each piece of work in openspec/changes/ — a proposal, a task list, and the requirement edits it makes.',
    path: '/home/dev/work/thing/openspec',
  }),

  /** Set up in flight: the button is inert so a second press cannot run `init` twice. */
  'absent-busy': story(
    {
      kind: 'absent',
      hint: 'OpenSpec keeps this project’s requirements in openspec/specs/.',
      path: '/home/dev/work/thing/openspec',
    },
    { busy: true },
  ),

  /**
   * The likeliest failure on a real machine, and the sentence is Rust's: it is the only layer
   * that can say a desktop-launched cide has a different PATH from a terminal one.
   */
  'cli-missing': story({
    kind: 'unusable',
    reason:
      '`openspec` is not on this app’s PATH, nor in any Node installation directory cide searched, so cide cannot read this project’s specs. Install it with `npm install -g @fission-ai/openspec`. A cide started from a desktop launcher has a different PATH from one started in a terminal.',
  }),

  /** The same arm, a different sentence — which is why the panel does not compose either. */
  unusable: story({
    kind: 'unusable',
    reason:
      '`openspec` resolved its root to /home/dev/work rather than to /home/dev/work/thing. OpenSpec searches parent directories, so this would have reported another project’s specs.',
  }),

  board: story(READY),

  /** Set up, and nothing proposed. The teaching button is the only thing to do. */
  'board-empty': story({
    kind: 'ready',
    root: '/home/dev/work/thing',
    specs: [],
    changes: [],
    commands: [
      { name: 'explore', line: '/openspec-explore' },
      { name: 'propose', line: '/openspec-propose' },
    ],
  }),

  /**
   * A project set up by an older `openspec`, which invokes the same workflow as slash commands.
   *
   * Drawn as its own story because the prefix is the whole of what differs, and a panel that
   * hard-codes one of the two spellings passes every other story in this file while refusing
   * every real project on the other surface — which is exactly how it shipped.
   */
  'ask-propose-legacy': story(
    {
      kind: 'ready',
      root: '/home/dev/work/thing',
      specs: [],
      changes: [],
      commands: [
        { name: 'explore', line: '/opsx:explore' },
        { name: 'propose', line: '/opsx:propose' },
      ],
    },
    { asking: 'propose' },
  ),

  'board-collapsed': story(READY, { expanded: { changes: false, specs: false } }),

  /**
   * The spec board is ready and the **task** board is not — and this story exists because the
   * panel used to draw the two as one.
   *
   * `tasks` is empty on three of `Board`'s four arms, and only on `ready` does empty mean *this
   * change has no task*. Rendered as though it did, every row offered a live *Start work* — on
   * changes that already had tasks — and pressing it did nothing whatever, because
   * `tasksStore.create` refuses `unknown` and `unreadable` in silence. So the rows here carry
   * their action inert, with the reason on it. `TASKS` is passed all the same: the point is that
   * the tracker's arm decides, not the map.
   */
  'board-tracker-unread': story(READY, { tracker: 'unknown' }),

  /*
   * The buttons, with the composer open on each. (M28)
   *
   * The box itself is `ProposeDialog`'s and has stories of its own below; what these two pin is
   * the panel's half — that a button whose press opened something says so, through
   * `aria-expanded`, which is the only thing a screen reader has to go on when the surface that
   * opened is a portal somewhere else in the document.
   */
  'ask-propose': story(READY, { asking: 'propose' }),
  'ask-explore': story(READY, { asking: 'explore' }),
}

/* ---------------------------------------------------------------- the composer (M28) */

/**
 * The propose/explore composer, which is a modal now rather than a box in the sidebar.
 *
 * The `line` is a **prop** and not derived here, which is the thing these stories exist to keep
 * honest: a project's invocation is read from its board, because OpenSpec moved `/opsx:propose`
 * to `/openspec-propose` and every surface with the old one baked in previewed a line no project
 * had. `propose-legacy` is a project still on the old surface, and its digest is what says cide
 * shows whichever one that project actually has.
 */
const PROPOSE_HANDLERS = {
  onText: () => {},
  onSend: () => {},
  onCancel: () => {},
}

function propose(over: Partial<ProposeFormProps> = {}): ProposeFormProps {
  return {
    command: 'propose',
    line: '/openspec-propose',
    text: '',
    busy: false,
    ...PROPOSE_HANDLERS,
    ...over,
  }
}

export type ProposeStoryName =
  | 'propose'
  | 'propose-typed'
  | 'propose-multiline'
  | 'propose-legacy'
  | 'propose-busy'
  | 'explore'

export const PROPOSE_STORIES: Record<ProposeStoryName, ProposeFormProps> = {
  /** Nothing typed: Send is off, because a bare propose asks Claude to ask what to propose. */
  propose: propose(),

  'propose-typed': propose({ text: 'a dark theme that follows the system setting' }),

  /*
   * A description across three lines, which is the case the preview exists for: the command is
   * *typed* into a PTY and ended with Enter, so a newline in the middle submits the first half as
   * a turn. The box shows exactly the one line that will be sent.
   */
  'propose-multiline': propose({ text: 'a dark theme\n  that follows\n\tthe system setting' }),

  /** A project still on the slash-command surface. The line is the project's, never a template. */
  'propose-legacy': propose({ line: '/opsx:propose', text: 'a dark theme' }),

  /** A send in flight: every control inert, so a second press cannot type the line twice. */
  'propose-busy': propose({ text: 'a dark theme', busy: true }),

  /** Explore sends empty, because it is a mode rather than a request. */
  explore: propose({ command: 'explore', line: '/openspec-explore' }),
}

/* ------------------------------------------------------------- the requirement editor (M28) */

const DRAFT = {
  name: 'Theme switching',
  text: 'The app SHALL switch between a light and a dark theme.',
  scenarios: [
    { title: 'The user picks dark', body: '- **WHEN** the user selects dark\n- **THEN** it repaints' },
  ],
}

const EDIT_HANDLERS = {
  onDraft: () => {},
  onSave: () => {},
  onCancel: () => {},
}

function editor(over: Partial<RequirementEditorProps> = {}): RequirementEditorProps {
  return { draft: DRAFT, busy: false, problem: null, ...EDIT_HANDLERS, ...over }
}

export type EditorStoryName =
  | 'editor'
  | 'editor-busy'
  | 'editor-nameless'
  | 'editor-no-scenarios'
  | 'editor-regressed'
  | 'editor-conflicted'

export const EDITOR_STORIES: Record<EditorStoryName, RequirementEditorProps> = {
  editor: editor(),

  /** A save in flight: every control inert, so a second press cannot write twice. */
  'editor-busy': editor({ busy: true }),

  /** Save off, with the sentence naming the field — never off in silence. */
  'editor-nameless': editor({ draft: { ...DRAFT, name: '  ' } }),
  'editor-no-scenarios': editor({ draft: { ...DRAFT, scenarios: [] } }),

  /**
   * The two failures that leave the form up with the typing still in it. A save that closed the
   * editor and reported elsewhere would throw away the paragraph it failed to write.
   */
  'editor-regressed': editor({
    problem: {
      kind: 'regressed',
      messages: ['Requirement must have at least one scenario', 'Scenario must not be empty'],
    },
  }),
  'editor-conflicted': editor({
    problem: {
      kind: 'conflicted',
      messages: ['/home/dev/work/thing/openspec/changes/add-dark-mode/specs/dark-mode/spec.md'],
    },
  }),
}

/* ------------------------------------------------------------------ the change page (M28) */

const CHANGE: ChangeView = {
  name: 'add-dark-mode',
  title: 'add-dark-mode',
  completed: 3,
  total: 9,
  tasks: [
    { done: true, description: 'Add the theme tokens' },
    { done: true, description: 'Wire the palette switch' },
    { done: true, description: 'Repaint the editor panes' },
    { done: false, description: 'Follow the system setting' },
  ],
  artifacts: [
    {
      id: 'proposal',
      generates: 'proposal.md',
      state: 'done',
      existing: ['/home/dev/work/thing/openspec/changes/add-dark-mode/proposal.md'],
    },
    {
      id: 'tasks',
      generates: 'tasks.md',
      state: 'ready',
      existing: ['/home/dev/work/thing/openspec/changes/add-dark-mode/tasks.md'],
    },
    /*
     * Declared and **not written**, which is the state the marker exists against: `design` is
     * optional and `propose` writes none, so the majority of changes look like this and
     * must draw no marker at all. `tab-design` is the same change with the file present.
     */
    {
      id: 'design',
      generates: 'design.md',
      state: 'ready',
      existing: [],
    },
  ],
  deltas: [
    {
      spec: 'dark-mode',
      op: 'added',
      description: 'Add theme switching',
      renamedFrom: null,
      renamedTo: null,
      requirements: [
        {
          name: 'Theme switching',
          text: 'The app SHALL switch between a light and a dark theme.',
          scenarios: [
            { title: 'The user picks dark', body: '- **WHEN** the user selects dark\n- **THEN** it repaints' },
          ],
          block: '### Requirement: Theme switching',
        },
      ],
    },
  ],
  validation: { valid: true, issues: [] },
  archivedAs: null,
}

/**
 * The same change after `openspec archive` moved it. (M28)
 *
 * The three fields an archived read cannot answer are exactly as `cide_spec::archived_change`
 * leaves them — `0/0`, no checklist, a vacuously clean verdict — because that combination is what
 * every surface has to *not* draw. Rendered naively it reads as a brand-new proposal with nothing
 * planned and a green tick, which is the opposite of the truth in all three.
 */
const ARCHIVED: ChangeView = {
  ...CHANGE,
  completed: 0,
  total: 0,
  tasks: [],
  validation: { valid: true, issues: [] },
  archivedAs: '2026-08-27-add-dark-mode',
  artifacts: CHANGE.artifacts.map((artifact) => ({
    ...artifact,
    existing: artifact.existing.map((path) =>
      path.replace('/changes/add-dark-mode/', '/changes/archive/2026-08-27-add-dark-mode/'),
    ),
  })),
}

/**
 * A proposal, as the page draws it: markdown, not a link.
 *
 * Deliberately exercises the constructs a proposal actually contains — a heading, prose, a list
 * and a fence — because the point of the story is that they are *rendered*, and a fixture of one
 * plain paragraph would have the same digest whether the renderer ran or not.
 */
const PROPOSAL = {
  path: '/home/dev/work/thing/openspec/changes/add-dark-mode/proposal.md',
  text: [
    '## Why',
    '',
    'The editor is unreadable at night, and the OS already publishes a preference.',
    '',
    '- Follow `prefers-color-scheme` by default',
    '- Let the setting override it',
    '',
    '```ts',
    'const dark = matchMedia("(prefers-color-scheme: dark)")',
    '```',
  ].join('\n'),
  truncated: false,
}

const TAB_HANDLERS = {
  onFind: () => {},
  onNavigate: () => {},
  onStart: () => {},
  onSplit: () => {},
  onOpenDoc: () => {},
  onEditOpen: () => {},
  onEditDraft: () => {},
  onEditCancel: () => {},
  onEditSave: () => {},
}

const NO_EDIT = { target: null, draft: null, busy: false, problem: null }

function tab(over: Partial<SpecTabViewProps> = {}): SpecTabViewProps {
  return {
    view: CHANGE,
    failed: null,
    reading: false,
    task: null,
    // No task means no role, so no branch — which is also what a story overriding `task` with a
    // conversation-dispatched one wants, and why the default is `null` rather than `true`.
    roleWorktree: null,
    startAction: rowAction(null, 'ready'),
    splitAction: splitAction('ready'),
    edit: NO_EDIT,
    proposal: null,
    find: null,
    matches: 0,
    ...TAB_HANDLERS,
    ...over,
  }
}

export type TabStoryName =
  | 'tab-reading'
  | 'tab-failed'
  | 'tab'
  | 'tab-with-task'
  | 'tab-tracker-unknown'
  | 'tab-ready'
  | 'tab-invalid'
  | 'tab-finding'
  | 'tab-finding-nothing'
  | 'tab-editing'
  | 'tab-proposal'
  | 'tab-design'
  | 'tab-archived'

export const TAB_STORIES: Record<TabStoryName, SpecTabViewProps> = {
  /** The read in flight. Deliberately not an empty page — see the host. */
  'tab-reading': tab({ view: null, reading: true }),

  /**
   * The read finished and there is nothing.
   *
   * No longer the archive case — see `tab-archived`, which is what that used to render as. This
   * arm is what is left: a change that was deleted, renamed by hand, or never existed.
   */
  'tab-failed': tab({
    view: null,
    reading: false,
    failed: 'This change could not be read. It may have been deleted.',
  }),

  /**
   * The change after `openspec archive` moved it. (M28)
   *
   * `tab-failed` is what this used to be: the CLI resolves `openspec/changes/<name>/proposal.md`
   * and an archived change is not there, so the page said *"could not be read"* about a change
   * whose every file is still on disk, from the moment the work was accepted.
   *
   * The story is as much about what is **not** drawn. The three fields an archive cannot answer
   * come back `0/0` and vacuously valid; naively that is an empty progress bar reading *no steps
   * planned yet* and a green *Valid*, over work that is finished and merged. So the strip names
   * the directory instead, the badge reads `Archived`, and the stage does too.
   */
  'tab-archived': tab({ view: ARCHIVED }),

  /** Nobody is working it: the page offers to start, which creates the task. */
  tab: tab(),

  'tab-with-task': tab({ task: { id: 't-14', agent: 'developer', status: 'doing', session: null } }),

  /**
   * The board has not answered yet, which is the arm where **both** controls must be inert.
   *
   * `task === null` means *this change has no task* only on a `ready` board; here it means nobody
   * has looked, and drawing a confident *Start work* on it is the bug `rowAction`'s doc is
   * written about. Unstoried until *Split work* arrived, which is its own argument for the story:
   * the primary's inert arm had never been rendered.
   */
  'tab-tracker-unknown': tab({
    startAction: rowAction(null, 'unknown'),
    splitAction: splitAction('unknown'),
  }),

  /** Every box ticked and valid — the state whose next step is Integrate & Archive. */
  'tab-ready': tab({
    view: { ...CHANGE, completed: 4, total: 4 },
    task: { id: 't-14', agent: 'developer', status: 'review', session: null },
  }),

  /** Finished and refused by the validator, which is the pair `tab-ready` exists against. */
  'tab-invalid': tab({
    view: {
      ...CHANGE,
      completed: 4,
      total: 4,
      validation: {
        valid: false,
        issues: [
          {
            level: 'ERROR',
            path: 'Theme switching',
            message: 'Requirement must have at least one scenario',
            line: 4,
          },
        ],
      },
    },
    task: { id: 't-14', agent: 'developer', status: 'review', session: null },
  }),

  /**
   * The find bar with hits. Two stories, because the pair is the point: a bar showing a count
   * and one saying so when there is nothing are different screens, and a search that silently
   * found nothing is how a reader concludes the page is broken.
   */
  'tab-finding': tab({ find: { query: 'theme', index: 1 }, matches: 3 }),
  'tab-finding-nothing': tab({ find: { query: 'zzz', index: 0 }, matches: 0 }),

  /**
   * The proposal drawn as prose. Its pair is plain `tab`, which has the same change with the text
   * not yet read — and the difference between the two is the whole claim: the Documents list
   * loses its proposal row **only** when the prose that replaces it is on screen.
   */
  'tab-proposal': tab({ proposal: PROPOSAL }),

  /** A change with a design document: the one story that draws the marker. */
  'tab-design': tab({
    view: {
      ...CHANGE,
      artifacts: CHANGE.artifacts.map((artifact) =>
        artifact.id === 'design'
          ? {
              ...artifact,
              state: 'done',
              existing: ['/home/dev/work/thing/openspec/changes/add-dark-mode/design.md'],
            }
          : artifact,
      ),
    },
  }),

  'tab-editing': tab({
    edit: {
      target: 'd0.r0',
      draft: {
        name: 'Theme switching',
        text: 'The app SHALL switch between a light and a dark theme.',
        scenarios: [
          { title: 'The user picks dark', body: '- **WHEN** the user selects dark\n- **THEN** it repaints' },
        ],
      },
      busy: false,
      problem: null,
    },
  }),
}
