/**
 * The New project wizard's decisions, with no React and no imports. (M97)
 *
 * Import-free for the reason `menus/model.ts` and `chrome/pushModel.ts` are: `check-new-project.mjs`
 * compiles this file standalone with the TypeScript already in `node_modules` and asserts on what
 * it answers. Everything the wizard *decides* lives here — which steps a road has, when Next is
 * allowed, what the request says, which checklist lines creation will draw — and
 * `NewProjectWizard.tsx` only draws it. The wire types are restated structurally rather than
 * imported, `pushModel.ts`'s `PushPreviewLike` shape: the compiler checks the two agree at the
 * one call site that passes a real `NewProjectProbe` in.
 */

export type Kind = 'empty' | 'spec' | 'tasks'
export type StepId = 'kind' | 'location' | 'openspec' | 'agents' | 'brief' | 'create'
export type CreateStep = 'folder' | 'git' | 'openspec' | 'agents' | 'open' | 'brief'

/** The fields of `NewProjectProbe` a decision here reads. */
export interface ProbeLike {
  path: string
  exists: boolean
  entries: number
  hasGit: boolean
  insideRepo?: string
  hasOpenspec: boolean
  hasCide: boolean
  openAlready: boolean
  problem?: string
  openspecMissing?: string
}

export interface Form {
  kind: Kind | null
  path: string
  /** What the checkbox says. Forced on by subagents; see `gitInit`. */
  gitInit: boolean
  specContext: string
  /** The Spec road's toggle. The Tasks road ignores it — subagents are that road. */
  agents: boolean
  brief: string
}

export const EMPTY_FORM: Form = {
  kind: null,
  path: '',
  gitInit: true,
  specContext: '',
  agents: false,
  brief: '',
}

/** The steps a road walks, in order. `kind` first and `create` last on every one. */
export function steps(kind: Kind | null): StepId[] {
  switch (kind) {
    case 'spec':
      return ['kind', 'location', 'openspec', 'agents', 'create']
    case 'tasks':
      return ['kind', 'location', 'agents', 'brief', 'create']
    default:
      return ['kind', 'location', 'create']
  }
}

export const STEP_LABEL: Record<StepId, string> = {
  kind: 'Project type',
  location: 'Location',
  openspec: 'OpenSpec',
  agents: 'Subagents',
  brief: 'Your brief',
  create: 'Create',
}

/** Subagents are on for this form. The Tasks road has no switch: they are what it is. */
export function agentsOn(form: Form): boolean {
  return form.kind === 'tasks' || (form.kind === 'spec' && form.agents)
}

/**
 * Whether `git init` runs. Subagents force it, because enabling them on a directory that is not
 * a repository is refused (`cmd::agents::worktree_refusal`) — an unticked box there would be a
 * checklist line failing for a reason the wizard knew about before the user pressed Create.
 */
export function gitInit(form: Form): boolean {
  return form.gitInit || agentsOn(form)
}

/** The path the probe answered is the path in the box — a stale probe decides nothing. */
export function probeIsCurrent(form: Form, probe: ProbeLike | null, asked: string): boolean {
  return probe !== null && asked === form.path.trim()
}

/** Why Next is not allowed on `step`, or `null` when it is. */
export function blocked(
  step: StepId,
  form: Form,
  probe: ProbeLike | null,
  asked: string,
): string | null {
  switch (step) {
    case 'kind':
      return form.kind === null ? 'Choose a project type' : null
    case 'location':
      if (form.path.trim() === '') return 'Choose a folder'
      if (!probeIsCurrent(form, probe, asked)) return 'Checking the folder…'
      return probe?.problem ?? null
    default:
      return null
  }
}

/** The location step's one-line verdict on a probe that has no problem. */
export function locationNote(probe: ProbeLike): { tone: 'ok' | 'warn' | 'info'; text: string } {
  if (probe.openAlready) {
    return { tone: 'info', text: 'This folder is already open in cide — it will be set up and brought to the front.' }
  }
  if (!probe.exists) return { tone: 'ok', text: 'A new folder will be created here.' }
  if (probe.entries === 0) return { tone: 'ok', text: 'An empty folder — a clean start.' }
  const items = probe.entries === 1 ? '1 item' : `${probe.entries} items`
  return {
    tone: 'warn',
    text: `This folder is not empty (${items}). Nothing in it will be changed or removed.`,
  }
}

/** Extra facts about the folder, each a short sentence, for under the verdict. */
export function locationFacts(probe: ProbeLike): string[] {
  const facts: string[] = []
  if (probe.hasGit) facts.push('Already a git repository.')
  else if (probe.insideRepo !== undefined) {
    facts.push(`Inside the repository at ${probe.insideRepo} — a new repository will be made here.`)
  }
  if (probe.hasOpenspec) facts.push('OpenSpec is already set up here.')
  if (probe.hasCide) facts.push('Has a .cide folder — its tasks and roles are kept.')
  return facts
}

/**
 * The checklist creation will draw, in the order `project_new` runs it — so the lines that tick
 * are the lines that were promised, and a line that never arrives is visible as not ticked.
 */
export function plannedSteps(form: Form, probe: ProbeLike | null): CreateStep[] {
  const out: CreateStep[] = ['folder']
  if (gitInit(form)) out.push('git')
  if (form.kind === 'spec' && !(probe?.hasOpenspec ?? false)) out.push('openspec')
  if (agentsOn(form)) out.push('agents')
  out.push('open')
  if (form.kind === 'tasks') out.push('brief')
  return out
}

export const CREATE_LABEL: Record<CreateStep, string> = {
  folder: 'Create the folder',
  git: 'Initialise a git repository',
  openspec: 'Set up OpenSpec',
  agents: 'Enable subagents',
  open: 'Open the project',
  brief: 'Brief the console',
}

/** The request, shaped like `NewProjectRequest`. Empty text is sent as absent. */
export function request(form: Form): {
  path: string
  kind: Kind
  gitInit: boolean
  agents: boolean
  specContext?: string
  brief?: string
} {
  const kind = form.kind ?? 'empty'
  const out: ReturnType<typeof request> = {
    path: form.path.trim(),
    kind,
    gitInit: gitInit(form),
    agents: agentsOn(form),
  }
  if (kind === 'spec' && form.specContext.trim() !== '') out.specContext = form.specContext
  if (kind === 'tasks' && form.brief.trim() !== '') out.brief = form.brief
  return out
}

/** The three cards on the first step. */
export const KINDS: readonly {
  kind: Kind
  title: string
  pitch: string
  points: readonly string[]
}[] = [
  {
    kind: 'empty',
    title: 'Empty',
    pitch: 'A folder and a console. Add structure whenever you want it.',
    points: [
      'Creates the folder, and a git repository if you like',
      'Opens straight onto the Claude console',
      'OpenSpec and subagents stay one click away',
    ],
  },
  {
    kind: 'spec',
    title: 'OpenSpec-driven',
    pitch: 'Agree on what to build before building it. Every change starts as a proposal.',
    points: [
      'Runs openspec init and installs its Claude commands',
      'Proposals, requirement deltas and tasks on the Specs panel',
      'Optionally, subagents that implement approved changes',
    ],
  },
  {
    kind: 'tasks',
    title: 'Task-driven',
    pitch: 'Describe the goal, and the console plans it with you: milestones, roles, tasks.',
    points: [
      'Milestones with gate commands that prove each is met',
      'Roles that work in parallel, each in its own worktree',
      'A board you review and integrate from',
    ],
  },
]

/** What the console will do first, for the brief step's preview. */
export const BRIEF_PLAN: readonly string[] = [
  'Define 2 to 5 milestones, each with a gate command',
  'Create the roles the work needs',
  "Create the first milestone's tasks, unassigned",
  'Summarise the plan and wait for your go-ahead',
]
