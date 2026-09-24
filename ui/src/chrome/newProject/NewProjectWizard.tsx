/**
 * *New project* — a small wizard: what kind of project, where, and what it needs. (M97)
 *
 * Three roads, one card. *Empty* is a folder and a console. *OpenSpec-driven* adds `openspec
 * init`, an optional project context and, optionally, subagents. *Task-driven* turns subagents on
 * and takes a brief, which the project's console is handed once the project opens: it drafts the
 * milestones, the roles and the first tasks, then waits for the user. `cmd::new_project` does the
 * work; this component only collects the answers and draws the checklist as the steps land.
 *
 * `wizardModel.ts` owns every decision — the steps of each road, when Next is allowed, the request,
 * the checklist — so `check:new-project` can assert them with no DOM. `art.tsx` draws the pictures.
 *
 * # Three things this component must keep doing
 *
 * **It can be raised with no project open.** It is mounted beside `PushDialog` in `App.tsx`, in
 * both window kinds, and never inside `OverlayHost`, which mounts only while a project is open.
 *
 * **Its form is reset on every open.** `NewProjectWizard` renders `Wizard` only while the store
 * says open, so each open mounts a fresh one. A path left from an earlier, abandoned attempt is a
 * project created somewhere the user did not look.
 *
 * **It cannot be dismissed while creating.** Escape and the scrim are ignored once Create is
 * pressed: the work continues in Rust whatever the webview does, and a closed wizard would leave
 * the user with no word of which steps failed.
 */
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactElement,
} from 'react'

import { OverlayCard } from '@/overlays/ModalShell'
import { Icon } from '@/icons/Icon'
import { newProjectApi, type NewProjectProbe, type NewProjectStepState } from '@/ipc/client'
import { useWorkspace } from '@/store/workspace'

import { AgentsArt, EmptyArt, MilestonesArt, SpecArt, TasksArt } from './art'
import { useNewProject } from './newProjectStore'
import {
  BRIEF_PLAN,
  CREATE_LABEL,
  EMPTY_FORM,
  KINDS,
  STEP_LABEL,
  agentsOn,
  blocked,
  gitInit,
  locationFacts,
  locationNote,
  plannedSteps,
  request,
  steps,
  type CreateStep,
  type Form,
  type Kind,
  type StepId,
} from './wizardModel'

import styles from './NewProjectWizard.module.css'

export function NewProjectWizard() {
  const open = useNewProject((s) => s.open)
  return open ? <Wizard /> : null
}

type Phase = 'form' | 'running' | 'failed'
type Line = { state: NewProjectStepState; detail?: string }

const ART: Record<Kind, () => ReactElement> = {
  empty: EmptyArt,
  spec: SpecArt,
  tasks: TasksArt,
}

function Wizard() {
  const [form, setForm] = useState<Form>(EMPTY_FORM)
  const [at, setAt] = useState(0)
  const [probe, setProbe] = useState<NewProjectProbe | null>(null)
  const [asked, setAsked] = useState('')
  const [phase, setPhase] = useState<Phase>('form')
  const [lines, setLines] = useState<Partial<Record<CreateStep, Line>>>({})
  const [error, setError] = useState<string | null>(null)
  const body = useRef<HTMLDivElement>(null)

  const road = steps(form.kind)
  const step: StepId = road[Math.min(at, road.length - 1)] ?? 'kind'
  const busy = phase === 'running'
  const close = useCallback(() => {
    if (!busy) useNewProject.getState().close()
  }, [busy])
  const patch = (next: Partial<Form>) => setForm((now) => ({ ...now, ...next }))

  // The probe, debounced, and dropped when the path moved on while it was out. `asked` is what
  // makes a slow answer for `~/a` unable to bless `~/ab`.
  useEffect(() => {
    const path = form.path.trim()
    if (path === '') {
      setProbe(null)
      setAsked('')
      return
    }
    let live = true
    const timer = window.setTimeout(() => {
      newProjectApi
        .probe(path)
        .then((answer) => {
          if (!live) return
          setProbe(answer)
          setAsked(path)
        })
        .catch(() => {})
    }, 180)
    return () => {
      live = false
      window.clearTimeout(timer)
    }
  }, [form.path])

  // The checklist, as Rust reports it. Subscribed for the life of the wizard rather than around
  // the call, so no event can fall between a subscription and the command it describes.
  useEffect(() => {
    const off = newProjectApi.onProgress((progress) => {
      setLines((now) => ({
        ...now,
        [progress.step]: {
          state: progress.state,
          ...(progress.detail === undefined ? {} : { detail: progress.detail }),
        },
      }))
    })
    return () => {
      void off.then((stop) => stop())
    }
  }, [])

  // Focus the step's first field each time the step changes — never on every render, or typing
  // into a textarea would keep pulling the caret back.
  useEffect(() => {
    const first = body.current?.querySelector<HTMLElement>('[data-autofocus]')
    first?.focus()
  }, [step])

  const why = blocked(step, form, probe, asked)
  const last = step === 'create'

  const next = () => {
    if (why !== null || last) return
    setAt((i) => Math.min(i + 1, road.length - 1))
  }
  const back = () => {
    if (busy) return
    setPhase('form')
    setAt((i) => Math.max(i - 1, 0))
  }

  const create = async () => {
    setPhase('running')
    setError(null)
    setLines({})
    try {
      const outcome = await newProjectApi.create(request(form))
      // The answer repeats every failure, for a window that missed an event.
      setLines((now) => {
        const merged = { ...now }
        for (const failure of outcome.failures) {
          merged[failure.step] = {
            state: 'failed',
            ...(failure.detail === undefined ? {} : { detail: failure.detail }),
          }
        }
        return merged
      })
      if (outcome.project === undefined || outcome.failures.length > 0) {
        setPhase('failed')
        return
      }
      await useWorkspace.getState().synced()
      useNewProject.getState().close()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      setPhase('failed')
    }
  }

  const browse = async () => {
    const picked = await newProjectApi.pickLocation().catch(() => null)
    if (picked !== null) patch({ path: picked })
  }

  const onKeyDown = (ev: KeyboardEvent) => {
    if (ev.key === 'Escape') {
      ev.stopPropagation()
      close()
      return
    }
    if (ev.key !== 'Enter' || busy) return
    const target = ev.target as HTMLElement
    // Enter on a type card chooses it *and* moves on — the keyboard's double-click. Without this
    // the preventDefault below would swallow the button's own activation, and Enter on an
    // unchosen card would do nothing at all.
    const card = target.closest<HTMLElement>('[data-kind]')
    if (step === 'kind' && card !== null) {
      ev.preventDefault()
      patch({ kind: card.dataset.kind as Kind })
      setAt(1)
      return
    }
    // Any other button keeps its own Enter: Back, Browse and Cancel must not mean Continue.
    if (target.tagName === 'BUTTON') return
    // Enter in a textarea is a newline; Ctrl+Enter moves on from anywhere.
    const inText = target.tagName === 'TEXTAREA'
    if (inText && !(ev.ctrlKey || ev.metaKey)) return
    ev.preventDefault()
    if (last) {
      if (phase === 'form') void create()
    } else next()
  }

  return (
    <OverlayCard label="New project" onDismiss={close} className={styles.card}>
      <div className={styles.shell} onKeyDown={onKeyDown} data-audit="newProject">
        <aside className={styles.rail}>
          <div className={styles.brand}>
            <span className={styles.brandMark}>
              <Icon name="plus" size={2} />
            </span>
            <span className={styles.brandText}>New project</span>
          </div>
          <ol className={styles.stepper}>
            {road.map((id, i) => {
              const state = i < at ? 'done' : i === at ? 'current' : 'todo'
              return (
                <li key={id} className={styles.stepItem} data-state={state}>
                  <span className={styles.stepDot}>
                    {state === 'done' ? <Icon name="check" size={1} /> : i + 1}
                  </span>
                  <span className={styles.stepName}>{STEP_LABEL[id]}</span>
                </li>
              )
            })}
          </ol>
          {form.kind === null ? (
            <p className={styles.railNote}>More steps appear once you choose a type.</p>
          ) : null}
        </aside>

        <section className={styles.main}>
          <div className={styles.body} ref={body} key={step}>
            {step === 'kind' && (
              <KindStep
                kind={form.kind}
                pick={(kind) => patch({ kind })}
                pickAndGo={(kind) => {
                  patch({ kind })
                  setAt(1)
                }}
              />
            )}
            {step === 'location' && (
              <LocationStep
                form={form}
                probe={asked === form.path.trim() ? probe : null}
                patch={patch}
                browse={() => void browse()}
              />
            )}
            {step === 'openspec' && <OpenSpecStep form={form} probe={probe} patch={patch} />}
            {step === 'agents' && <AgentsStep form={form} patch={patch} />}
            {step === 'brief' && <BriefStep form={form} patch={patch} />}
            {step === 'create' && (
              <CreateStepView
                form={form}
                probe={probe}
                phase={phase}
                lines={lines}
                error={error}
              />
            )}
          </div>

          <footer className={styles.foot}>
            <span className={styles.footNote}>
              {why !== null && step !== 'kind' ? why : last ? '' : 'Enter to continue'}
            </span>
            <button
              type="button"
              className={styles.ghost}
              onClick={at === 0 ? close : back}
              disabled={busy}
              data-audit="newProjectBack"
            >
              {at === 0 ? 'Cancel' : 'Back'}
            </button>
            {last ? (
              phase === 'failed' ? (
                <button
                  type="button"
                  className={styles.primary}
                  onClick={() => useNewProject.getState().close()}
                  data-audit="newProjectDone"
                >
                  Close
                </button>
              ) : (
                <button
                  type="button"
                  className={styles.primary}
                  onClick={() => void create()}
                  disabled={busy}
                  data-audit="newProjectCreate"
                >
                  {busy ? 'Creating…' : 'Create project'}
                </button>
              )
            ) : (
              <button
                type="button"
                className={styles.primary}
                onClick={next}
                disabled={why !== null}
                data-audit="newProjectNext"
              >
                Continue
                <Icon name="chevron-right" size={1} />
              </button>
            )}
          </footer>
        </section>
      </div>
    </OverlayCard>
  )
}

function Heading({ title, lead }: { title: string; lead: string }) {
  return (
    <header className={styles.heading}>
      <h2 className={styles.title}>{title}</h2>
      <p className={styles.lead}>{lead}</p>
    </header>
  )
}

function KindStep({
  kind,
  pick,
  pickAndGo,
}: {
  kind: Kind | null
  pick: (kind: Kind) => void
  pickAndGo: (kind: Kind) => void
}) {
  return (
    <>
      <Heading
        title="What are you starting?"
        lead="Pick how the project will be driven. You can add OpenSpec or subagents to any project later."
      />
      <div className={styles.kinds} role="radiogroup" aria-label="Project type">
        {KINDS.map((option, i) => {
          const Art = ART[option.kind]
          const on = kind === option.kind
          return (
            <button
              key={option.kind}
              type="button"
              role="radio"
              aria-checked={on}
              className={styles.kind}
              data-on={on}
              data-audit="newProjectKind"
              data-kind={option.kind}
              {...(i === 0 ? { 'data-autofocus': true } : {})}
              onClick={() => pick(option.kind)}
              onDoubleClick={() => pickAndGo(option.kind)}
            >
              <span className={styles.kindArt}>
                <Art />
              </span>
              <span className={styles.kindTitle}>
                {option.title}
                {on && (
                  <span className={styles.kindCheck}>
                    <Icon name="check" size={1} />
                  </span>
                )}
              </span>
              <span className={styles.kindPitch}>{option.pitch}</span>
              <ul className={styles.points}>
                {option.points.map((point) => (
                  <li key={point}>{point}</li>
                ))}
              </ul>
            </button>
          )
        })}
      </div>
    </>
  )
}

function LocationStep({
  form,
  probe,
  patch,
  browse,
}: {
  form: Form
  probe: NewProjectProbe | null
  patch: (next: Partial<Form>) => void
  browse: () => void
}) {
  const note = probe !== null && probe.problem === undefined ? locationNote(probe) : null
  const forced = agentsOn(form)
  return (
    <>
      <Heading
        title="Where should it live?"
        lead="Type a path or browse to a folder. Missing folders are created, and nothing already in the folder is changed."
      />
      <label className={styles.label} htmlFor="new-project-path">
        Project folder
      </label>
      <div className={styles.pathRow}>
        <span className={styles.pathIcon}>
          <Icon name="folder" size={2} />
        </span>
        <input
          id="new-project-path"
          className={styles.pathInput}
          value={form.path}
          placeholder="~/projects/my-app"
          spellCheck={false}
          autoComplete="off"
          data-autofocus
          data-audit="newProjectPath"
          onChange={(ev) => patch({ path: ev.target.value })}
        />
        <button type="button" className={styles.ghost} onClick={browse} data-audit="newProjectBrowse">
          Browse…
        </button>
      </div>

      <div className={styles.verdict} aria-live="polite">
        {probe?.problem !== undefined && (
          <p className={styles.note} data-tone="bad">
            <Icon name="circle-alert" size={1} />
            {probe.problem}
          </p>
        )}
        {note !== null && (
          <p className={styles.note} data-tone={note.tone}>
            <Icon name={note.tone === 'warn' ? 'triangle-alert' : note.tone === 'ok' ? 'circle-check' : 'info'} size={1} />
            {note.text}
          </p>
        )}
        {probe !== null && probe.problem === undefined &&
          locationFacts(probe).map((fact) => (
            <p key={fact} className={styles.fact}>
              {fact}
            </p>
          ))}
      </div>

      {probe?.hasGit !== true && (
        <label className={styles.toggle} data-disabled={forced}>
          <input
            type="checkbox"
            checked={gitInit(form)}
            disabled={forced}
            onChange={(ev) => patch({ gitInit: ev.target.checked })}
          />
          <span>
            <strong>Initialise a git repository</strong>
            <span className={styles.toggleHint}>
              {forced
                ? 'Required: subagents work on branches in their own git worktrees.'
                : 'Recommended. Diffs, history and blame all read it.'}
            </span>
          </span>
        </label>
      )}
    </>
  )
}

function OpenSpecStep({
  form,
  probe,
  patch,
}: {
  form: Form
  probe: NewProjectProbe | null
  patch: (next: Partial<Form>) => void
}) {
  const already = probe?.hasOpenspec === true
  return (
    <>
      <div className={styles.banner}>
        <SpecArt />
      </div>
      <Heading
        title="Specs first, then code"
        lead="OpenSpec keeps a living description of what the project does. Every change starts as a proposal — why, what, and the requirement deltas — which you approve before anyone writes code. Claude gets /openspec commands to propose, apply and archive."
      />
      {already ? (
        <p className={styles.note} data-tone="info">
          <Icon name="info" size={1} />
          OpenSpec is already set up in this folder, so this step is skipped.
        </p>
      ) : probe?.openspecMissing !== undefined ? (
        <p className={styles.note} data-tone="warn">
          <Icon name="triangle-alert" size={1} />
          {probe.openspecMissing} You can continue: the project is still created, and the Specs
          panel sets OpenSpec up once the CLI is installed.
        </p>
      ) : null}
      {!already && (
        <>
          <label className={styles.label} htmlFor="new-project-context">
            Project context <span className={styles.optional}>optional</span>
          </label>
          <textarea
            id="new-project-context"
            className={styles.textarea}
            rows={5}
            value={form.specContext}
            placeholder={'Tech stack, conventions, constraints.\nFor example: TypeScript + React, Postgres, deploys to Fly.io.'}
            data-autofocus
            onChange={(ev) => patch({ specContext: ev.target.value })}
          />
          <p className={styles.fieldHint}>
            Written to openspec/config.yaml. Every proposal reads it.
          </p>
        </>
      )}
    </>
  )
}

function AgentsStep({ form, patch }: { form: Form; patch: (next: Partial<Form>) => void }) {
  const tasks = form.kind === 'tasks'
  return (
    <>
      <div className={styles.banner}>
        <AgentsArt />
      </div>
      <Heading
        title="A team of subagents"
        lead="Roles are the workers you hand tasks to. Each run gets its own git worktree and branch, reports back on its task, and waits for you to review and integrate the work."
      />
      <ul className={styles.features}>
        <li>
          <Icon name="square-check-big" size={2} />
          <span>
            <strong>Tasks drive the work.</strong> Assigning a task to a role starts it. Progress
            lands as comments on the task.
          </span>
        </li>
        <li>
          <Icon name="git-branch" size={2} />
          <span>
            <strong>Parallel and isolated.</strong> Runs never share a checkout, so they cannot
            overwrite each other's work.
          </span>
        </li>
        <li>
          <Icon name="git-merge" size={2} />
          <span>
            <strong>You integrate.</strong> Nothing reaches your branch until you accept it.
          </span>
        </li>
      </ul>
      {tasks ? (
        <p className={styles.note} data-tone="info">
          <Icon name="info" size={1} />
          Always on for a task-driven project. The console creates the roles from your brief.
        </p>
      ) : (
        <label className={styles.toggle}>
          <input
            type="checkbox"
            checked={form.agents}
            data-autofocus
            onChange={(ev) => patch({ agents: ev.target.checked })}
          />
          <span>
            <strong>Enable subagents for this project</strong>
            <span className={styles.toggleHint}>
              Writes .cide/config.json. Add roles later from the Agents panel, or ask the console.
            </span>
          </span>
        </label>
      )}
    </>
  )
}

function BriefStep({ form, patch }: { form: Form; patch: (next: Partial<Form>) => void }) {
  return (
    <>
      <div className={styles.banner}>
        <MilestonesArt />
      </div>
      <Heading
        title="What do you want to build?"
        lead="Describe it the way you would to a colleague. When the project opens, the console reads this and drafts the plan with you. Milestones have gate commands, so done means a check passes."
      />
      <textarea
        className={styles.textarea}
        rows={5}
        value={form.brief}
        placeholder={'A command-line habit tracker in Rust. Stores data in SQLite,\nshows streaks, and exports to CSV. Tests from day one.'}
        data-autofocus
        data-audit="newProjectBrief"
        onChange={(ev) => patch({ brief: ev.target.value })}
      />
      <p className={styles.fieldHint}>
        {form.brief.trim() === ''
          ? 'Optional. Leave it empty and the console will ask you instead.'
          : 'Ctrl+Enter to continue.'}
      </p>
      <div className={styles.plan}>
        <span className={styles.planTitle}>The console will then</span>
        <ol className={styles.planList}>
          {BRIEF_PLAN.map((line) => (
            <li key={line}>{line}</li>
          ))}
        </ol>
      </div>
    </>
  )
}

function CreateStepView({
  form,
  probe,
  phase,
  lines,
  error,
}: {
  form: Form
  probe: NewProjectProbe | null
  phase: Phase
  lines: Partial<Record<CreateStep, Line>>
  error: string | null
}) {
  const kind = KINDS.find((k) => k.kind === form.kind)
  const planned = plannedSteps(form, probe)
  const started = phase !== 'form'
  const rows: [string, string][] = [
    ['Type', kind?.title ?? 'Empty'],
    ['Folder', probe?.path ?? form.path.trim()],
    ['Git', probe?.hasGit === true ? 'Already a repository' : gitInit(form) ? 'New repository' : 'None'],
  ]
  if (form.kind === 'spec') {
    rows.push(['OpenSpec', probe?.hasOpenspec === true ? 'Already set up' : form.specContext.trim() === '' ? 'Set up' : 'Set up, with context'])
  }
  if (form.kind !== 'empty') rows.push(['Subagents', agentsOn(form) ? 'Enabled' : 'Off'])
  if (form.kind === 'tasks') {
    rows.push(['Console', form.brief.trim() === '' ? 'Will ask what to build' : 'Will plan from your brief'])
  }
  return (
    <>
      <Heading
        title={started ? (phase === 'failed' ? 'Not everything went through' : 'Creating your project') : 'Ready to create'}
        lead={
          started
            ? phase === 'failed'
              ? 'The steps below that failed say why. Anything that succeeded is kept, and the project opens if its folder was created.'
              : 'This takes a few seconds. The project opens as soon as it is ready.'
            : 'Check the summary, then create. Nothing is written before you press Create project.'
        }
      />
      {!started ? (
        <dl className={styles.summary} data-audit="newProjectSummary">
          {rows.map(([k, v]) => (
            <div key={k} className={styles.summaryRow}>
              <dt>{k}</dt>
              <dd title={v}>{v}</dd>
            </div>
          ))}
        </dl>
      ) : (
        <ol className={styles.checklist} data-audit="newProjectChecklist">
          {planned.map((id) => {
            const line = lines[id]
            const state = line?.state ?? 'pending'
            return (
              <li key={id} className={styles.check} data-state={state}>
                <span className={styles.checkMark}>
                  {state === 'done' ? (
                    <Icon name="circle-check" size={2} />
                  ) : state === 'failed' ? (
                    <Icon name="circle-x" size={2} />
                  ) : state === 'running' ? (
                    <Icon name="loader-circle" size={2} className={styles.spin} />
                  ) : (
                    <Icon name="circle-dashed" size={2} />
                  )}
                </span>
                <span className={styles.checkBody}>
                  <span>{CREATE_LABEL[id]}</span>
                  {line?.detail !== undefined && (
                    <span className={styles.checkDetail}>{line.detail}</span>
                  )}
                </span>
              </li>
            )
          })}
        </ol>
      )}
      {error !== null && (
        <p className={styles.note} data-tone="bad">
          <Icon name="circle-alert" size={1} />
          {error}
        </p>
      )}
    </>
  )
}
