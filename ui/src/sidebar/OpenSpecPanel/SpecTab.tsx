/**
 * A change or a capability, as a whole page. (M28)
 *
 * # Why this exists, when the panel could open `proposal.md`
 *
 * Because it did, and that was the weakest thing in the feature. A change is a *directory* — a
 * proposal, a design, a task checklist and a delta spec per capability it touches — and which
 * files those are is decided by the workflow schema, not by cide. Answering *show me this change*
 * with one of its five documents left no way to reach the others, the checklist, the requirement
 * edits, or anything that could be done about any of them.
 *
 * The page also draws what no file contains: whether it validates, how far the checklist has got,
 * which task tracks it, and the actions. `TabKind::OpenSpec` carries the argument in full.
 *
 * # It owns no state that outlives it
 *
 * Everything is read through `spec.change` / `spec.spec` on mount and again whenever the board
 * moves, deliberately: a change's contents change while an agent works on it, and a page holding
 * its own copy would be a second, staler answer than the panel's. The one thing it keeps is the
 * open editor's draft, which is the user's typing and belongs to nobody else.
 */
import {
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react'
import { Icon, asIcon } from '@/icons/Icon'
import { notify, notifyFailure } from '@/chrome/notices'
import { parseMarkdown } from '@/editor/markdown/blocks'
import { MarkdownPreview } from '@/editor/markdown/MarkdownPreview'
import { dirOf, resolveLocal, targetKind } from '@/editor/markdown/links'
import { file, spec as specApi, type ChangeName, type ProjectId, type SpecId } from '@/ipc/client'
import { useSpec } from '../specStore'
import { useTasks } from '../tasksStore'
import { useAgents } from '../agentsStore'
import { EMPTY_DRAFT } from '../TasksPanel/model'
import { adaptChange } from './adapt'
import { RequirementEditor } from './RequirementEditor'
import styles from './SpecTab.module.css'
import {
  CHANGE_ICON,
  DESIGN_LABEL,
  DESIGN_TITLE,
  PROPOSAL_TITLE,
  documentArtifacts,
  hasDesign,
  isBlocking,
  issuesFor,
  opLabel,
  primaryAction,
  proposalArtifact,
  rowAction,
  splitAction,
  splitMatches,
  stageLabel,
  stepMatch,
  stageOf,
  taskProgress,
  unattributedIssues,
  validateBadge,
  type ActionOffer,
  type ChangeView,
  type RowAction,
} from './model'
import {
  compose,
  draftOf,
  type RequirementDraft,
} from './editModel'

export interface SpecTabProps {
  project: ProjectId
  /** Is this the tab on screen? See `App.tsx` — every tab is mounted, hidden or not. */
  active: boolean
  /** A change, or a capability. Exactly one. */
  change?: string | undefined
  spec?: string | undefined
}

/**
 * The change page, as a function of its props. (M28)
 *
 * Split from the host below for the reason every other surface in this directory is: the render
 * check SSRs this with fixed fixtures, and `renderToStaticMarkup` runs no effects — so a page
 * that fetched its own contents would be gated on nothing but its loading state. Everything it
 * draws arrives as a value.
 */
export interface SpecTabViewProps {
  /** `null` while the read is in flight. */
  view: ChangeView | null
  /** Why there is no view, when there is none and the read has finished. */
  failed: string | null
  reading: boolean
  /**
   * The task tracking this change: its id, assignee, status, and the conversation its work went
   * to, or `null`.
   *
   * `session` is carried beside `agent` and not folded into it — the two are different facts in
   * different id spaces, and Rust keeps them in different fields so a task cannot claim both.
   * Without it the page goes on offering *Approve & dispatch* for work that has already been
   * handed to a live conversation.
   */
  task: { id: string; agent: string | null; status: string; session: string | null } | null
  /**
   * Does `task.agent`'s role run in its own checkout? `null` for no role, or no roster yet.
   *
   * Its own prop rather than a field of `task`, because it is a fact about a **role** and not
   * about the task — the tracker knows nothing about it, and folding it in would invite the
   * next reader to look for it in `.cide/tasks.json`.
   *
   * The page only *draws* the primary action's line, but it has to draw the same one the card
   * acts on: `primaryAction`'s accept arm says *Integrate & Archive* or *Archive* depending on
   * this, and a page that answered differently from the card would be two surfaces disagreeing
   * about what the next step is.
   */
  roleWorktree: boolean | null
  /**
   * The proposal, as text, once it has been read — `null` while it is being read, when the
   * schema declares no proposal artifact, or when it declares one nobody has written.
   *
   * A separate prop and a separate read rather than a field of `view`, because it is a separate
   * *file*: `show --json` carries a change's deltas and none of its prose. Folding it into the
   * change read would put a file read on the path of every board refresh, and the board refreshes
   * every time an agent ticks a box.
   */
  proposal: { path: string; text: string; truncated: boolean } | null
  /** Following a link inside the proposal. See the host for what each kind of target does. */
  onNavigate: (href: string) => void
  edit: {
    target: string | null
    draft: RequirementDraft | null
    busy: boolean
    problem: { kind: 'regressed' | 'conflicted'; messages: readonly string[] } | null
  }
  /**
   * The primary button's label, tooltip and — when the tracker cannot answer — its refusal.
   *
   * Computed by `rowAction` in `model.ts`, the same call the panel row makes, because the page
   * and the row are two drawings of one decision and they were disagreeing about it silently.
   */
  startAction: RowAction
  onStart: () => void
  /**
   * *Split work*'s label, tooltip and — when the tracker cannot answer — its refusal.
   *
   * Drawn only while `task === null`: a change gets one task carrying its link, and splitting is
   * a thing you do to a change nobody has started. See `splitAction` in `model.ts`.
   */
  splitAction: ActionOffer
  onSplit: () => void
  onOpenDoc: (path: string) => void
  onEditOpen: (target: string, requirement: { name: string; text: string; scenarios: readonly { title: string; body: string }[] }) => void
  onEditDraft: (draft: RequirementDraft) => void
  onEditCancel: () => void
  onEditSave: (delta: { spec: string; op: string }, requirement: string) => void
  /**
   * The page's own element, so the host can scroll a hit into view and scope its Ctrl+F
   * listener. A plain prop and not `forwardRef`: React 19 passes `ref` through like any other,
   * and a forwarded one would be a second thing to thread for no gain.
   */
  ref?: React.Ref<HTMLDivElement> | undefined
  /** The find bar: `null` when it is shut. */
  find: { query: string; index: number } | null
  /**
   * How many matches the page holds.
   *
   * Counted by the *host* over the same text the view renders, not by the view over its own
   * markup: the view has to know the total before it draws the bar at the top, and it has not
   * drawn the content yet. Two counts of one thing, so the host's is the one the bar shows and
   * `Marked`'s running index is what it addresses.
   */
  matches: number
  onFind: (find: { query: string; index: number } | null) => void
}

function cx(...parts: readonly (string | false | null | undefined)[]): string {
  return parts.filter((part): part is string => typeof part === 'string' && part !== '').join(' ')
}

/**
 * One string, with the find bar's matches wrapped.
 *
 * Every piece of text the page draws goes through this, which is what makes the match count on
 * screen a count *of this page* — see `splitMatches` for why the browser's own find could not be.
 * `data-hit` is the index across the whole page, assigned by the caller through a counter, so
 * next/prev can scroll to the nth one without the marks having to know about each other.
 */
function Marked({
  text,
  query,
  counter,
  current,
}: {
  text: string
  query: string
  counter: { n: number }
  current: number
}) {
  const pieces = splitMatches(text, query)
  if (pieces.length === 1 && !pieces[0]!.hit) return <>{text}</>
  return (
    <>
      {pieces.map((piece, at) => {
        if (!piece.hit) return <span key={at}>{piece.text}</span>
        const index = counter.n
        counter.n += 1
        return (
          <mark
            key={at}
            className={index === current ? styles.hitCurrent : styles.hit}
            data-audit="specTabMark"
            data-hit={index}
            data-current={index === current ? 'true' : 'false'}
          >
            {piece.text}
          </mark>
        )
      })}
    </>
  )
}

/**
 * One collapsible block.
 *
 * `<details open>` — **open by default**, which is the opposite of `tasksHistory`'s rule one
 * panel over and deliberately so: that disclosure hides a log nobody opened the card to read,
 * while these are the page's content. A page that opened with everything shut would answer *show
 * me this change* with four headings.
 *
 * Uncontrolled, so collapsing survives the re-read that fires every time the board moves. Lifting
 * it into the host would mean an agent ticking a box re-expanded a section the user had just
 * shut.
 */
function Block({
  id,
  title,
  children,
}: {
  id: string
  title: string
  children: ReactNode
}) {
  return (
    <details className={styles.block} data-audit="specTabBlock" data-block={id} open>
      <summary className={styles.heading} data-audit="specTabBlockHead">
        {title}
      </summary>
      {children}
    </details>
  )
}

export function SpecTabView(props: SpecTabViewProps) {
  const { view, failed, reading, task, edit, find } = props
  const query = find?.query ?? ''
  // One counter threaded through every `Marked` on the page, so a mark's index is its position in
  // reading order and next/prev can address it without the marks knowing about each other.
  const counter = { n: 0 }
  const current = find?.index ?? 0

  if (reading) {
    return (
      <div className={styles.page} data-audit="specTab">
        <p className={styles.reading} data-audit="specTabReading">
          Reading…
        </p>
      </div>
    )
  }

  if (view === null) {
    return (
      <div className={styles.page} data-audit="specTab">
        <p className={styles.failed} data-audit="specTabFailed">
          {failed ?? 'This change could not be read.'}
        </p>
      </div>
    )
  }

  const stage = stageOf(view)
  // The second argument is what keeps an archived change from printing *Valid*: nothing
  // validated it and nothing can, so `unchecked` is the honest state. See `ChangeView`.
  const badge = validateBadge(view.validation, view.archivedAs != null)
  const progress = taskProgress(view)
  const action = primaryAction(
    view,
    task?.agent ?? null,
    task?.status ?? null,
    task?.session ?? null,
    props.roleWorktree,
  )
  const names = view.deltas.flatMap((delta) => delta.requirements.map((req) => req.name))
  const orphans = unattributedIssues(view.validation, names)
  const design = hasDesign(view)
  const proposalDoc = proposalArtifact(view)
  const prose = proposalDoc !== null && props.proposal !== null
  const documents = documentArtifacts(view, prose)

  return (
    <div ref={props.ref} className={styles.page} data-audit="specTab" data-change={view.name}>
      {/*
        * The find bar is the first thing in the page and sticks to the top of it — the editor's
        * own bar is `position: sticky; top: 0` in its scroller for the same reason. It sat below
        * the header, which meant scrolling to a hit scrolled the bar off with it and the count
        * and Enter went with it.
        */}
      {find !== null && <FindBar find={find} onFind={props.onFind} total={props.matches} />}
      <header className={styles.head}>
        <Icon name={asIcon(CHANGE_ICON)} size={2} />
        <h1 className={styles.title} data-audit="specTabTitle">
          {view.name}
        </h1>
        <span className={styles.stage} data-audit="specTabStage" data-stage={stage}>
          {stageLabel(stage)}
        </span>
        <span className={styles.badge} data-audit="specTabValidity" data-state={badge.state}>
          {badge.label}
        </span>
        {/*
          * The design marker: drawn only when there is a design document, never as an empty or
          * greyed-out slot. `design.md` is optional and `propose` does not write one, so
          * its presence is somebody's decision that this change needed an argument settled
          * first — which is the cheapest possible signal that it is not trivial. A permanent
          * chip that said "no design" would be noise on the majority of changes and would bury
          * the signal on the minority that matter.
          */}
        {design && (
          <span className={styles.design} data-audit="specTabDesign" title={DESIGN_TITLE}>
            <Icon name={asIcon('book-open-text')} size={1} />
            {DESIGN_LABEL}
          </span>
        )}
      </header>


      {/*
        * The checklist strip, and **not** for an archived change.
        *
        * Its checklist is not recoverable from the directory — which file is the checklist and
        * what counts as an item are the schema's business, and there is no CLI answer for a
        * change that has been moved — so `progress` comes back `0/0`. A full-width empty bar
        * reading *no steps planned yet* over work that is finished and merged is a picture that
        * is wrong rather than missing, so the line below says where the change went instead.
        */}
      {view.archivedAs != null ? (
        <section className={styles.strip} data-audit="specTabArchived">
          <span className={styles.figure}>
            archived as openspec/changes/archive/{view.archivedAs}/ — its requirements are in
            openspec/specs/ now
          </span>
        </section>
      ) : (
      <section className={styles.strip}>
        <div className={styles.bar} role="progressbar" aria-valuenow={progress.pct} aria-valuemin={0} aria-valuemax={100} data-audit="specTabProgress">
          {/*
            * The scale is an inline style, not a typed `attr()` in the stylesheet: WebKitGTK does
            * not implement CSS Values 5 `attr()`, so that declaration was invalid and dropped and
            * every bar drew full — see `.barFill`. `data-pct` is what the SSR digest reads.
            */}
          <span
            className={styles.barFill}
            data-pct={progress.pct}
            style={{ transform: `scaleX(${progress.pct / 100})` }}
          />
        </div>
        <span className={styles.figure}>
          {progress.total === 0 ? 'no steps planned yet' : `${progress.done} / ${progress.total} steps`}
        </span>
      </section>
      )}

      <section className={styles.actions}>
        {/*
          * Nothing to start on an archived change. (M28)
          *
          * *Start work* creates a task and is the road into a dispatch — offered on a change
          * whose deltas are already merged into `openspec/specs/`, it is a button that makes work
          * out of work that is finished. *Open task* survives, because a link back to the task
          * that did it is the most useful thing on this page once it is archived.
          */}
        {(task !== null || view.archivedAs == null) && (
        <button
          type="button"
          className={cx(styles.button, styles.primary)}
          data-audit="specTabStart"
          data-write="true"
          /*
           * Label, tooltip and inertness all come from `rowAction` now. They used to be spelled
           * here off `task === null` alone, which reads *no task* into a board nobody has read
           * yet — and `tasksStore.create` refuses exactly those arms in silence, so the page
           * offered *Start work* on a change with a finished task and the click did nothing.
           */
          title={props.startAction.disabledReason ?? props.startAction.title}
          disabled={props.startAction.disabledReason !== undefined}
          data-disabled={props.startAction.disabledReason === undefined ? undefined : 'true'}
          onClick={props.onStart}
        >
          {task === null ? props.startAction.label : `Open ${task.id}`}
        </button>
        )}
        {/*
          * *Split work*, and the two facts about where it sits.
          *
          * **Only while there is no task.** A change gets exactly one task carrying `change` —
          * `spec_triggers::consider_one` finds a change's task by that field and, on more than
          * one match, moves neither — so the fan-out is a thing you do to a change nobody has
          * started. Once a task exists the primary reads `Open t-NN` and this is gone.
          *
          * **After the primary and before the hint.** The hint sentence below is about the
          * primary action; a control wedged between the two reattributes it.
          *
          * Not `styles.primary`: the accent belongs to the one action that is the change's next
          * step, and two accented buttons say neither is.
          */}
        {task === null && (
          <button
            type="button"
            className={styles.button}
            data-audit="specTabSplit"
            data-write="true"
            title={props.splitAction.disabledReason ?? props.splitAction.title}
            disabled={props.splitAction.disabledReason !== undefined}
            data-disabled={props.splitAction.disabledReason === undefined ? undefined : 'true'}
            onClick={props.onSplit}
          >
            {props.splitAction.label}
          </button>
        )}
        {/*
          * The primary action is *drawn* here and acted on from the task card, deliberately.
          * Approving is assigning and accepting integrates a branch — both belong beside the
          * task's own status and history, and a second place to press them would be a second
          * place to get the ordering wrong. This says what the next step is; the card does it.
          */}
        <p className={styles.hint} data-audit="specTabHint">
          {action.gate.ok ? action.hint : action.gate.reason}
        </p>
      </section>

      {orphans.length > 0 && (
        <section className={styles.problems} data-audit="specTabProblems">
          {orphans.map((issue, index) => (
            <p key={index} className={styles.problem}>
              {issue.message}
            </p>
          ))}
        </section>
      )}

      <Block id="docs" title="Documents">
        {/*
          * Links, one per file — **not** the documents' contents. Rendering them inline was
          * tried and reverted: these are markdown files, the editor already opens them properly,
          * and a page that inlined four of them buried the parts of a change that exist nowhere
          * else (the checklist, the requirement edits, the actions) under a wall of prose.
          *
          * Two things this does fix. Every artifact is listed, **including one with no file** —
          * `design.md` is not written by `propose` and its status is `ready`, meaning
          * *this is the next thing to write*, so its absence from the list made the page look
          * like a change with no design rather than one whose design was still to come.
          *
          * And the label is the path **relative to the change**, not the basename. Every
          * capability's delta is called `spec.md`, so a change touching two of them drew two
          * rows that read identically — the "spec.md twice".
          */}
        {documents.map((artifact) =>
          artifact.existing.length === 0 ? (
            <div
              key={artifact.id}
              className={cx(styles.doc, styles.missing)}
              data-audit="specTabDoc"
              data-artifact={artifact.id}
              data-state={artifact.state}
            >
              <Icon name={asIcon('square')} size={1} />
              <span className={styles.docName}>{artifact.id}</span>
              <span className={styles.docPath}>
                {artifact.state === 'blocked'
                  ? 'blocked — an earlier artifact comes first'
                  : artifact.state === 'skipped'
                    ? 'skipped'
                    : 'not written yet'}
              </span>
            </div>
          ) : (
            artifact.existing.map((path) => (
              <button
                key={path}
                type="button"
                className={styles.doc}
                data-audit="specTabDoc"
                data-artifact={artifact.id}
                data-state={artifact.state}
                onClick={() => props.onOpenDoc(path)}
              >
                <Icon name={asIcon('file-text')} size={1} />
                {/*
                  * The **path leads**, and the artifact id is the dim half.
                  *
                  * The first fix put the path in the secondary column and left both delta rows
                  * leading with the word `specs`, so at a glance a change touching two
                  * capabilities still read as the same row twice. The path is the only part that
                  * differs between them, so it is the part that has to be read first.
                  */}
                <span className={styles.docName}>{relativeTo(view.name, path)}</span>
                <span className={styles.docPath}>{artifact.id}</span>
              </button>
            ))
          ),
        )}
      </Block>

      {/*
        * The proposal, rendered — not a link to it.
        *
        * It is the one document somebody opens this page *for*: the proposal **is** the change,
        * in prose. Everything else here is either drawn already (the checklist as steps, the
        * deltas as requirement cards) or genuinely a separate document, so the proposal was the
        * single link whose destination the page should have been showing.
        *
        * `MarkdownPreview` and not a second renderer: `ui/src/editor/markdown/` is a hand-written
        * implementation with `check:markdown` behind it, including the assertion that no node it
        * produces can carry markup. A second, simpler one here would be a second thing to keep
        * safe. It is also why this is not `dangerouslySetInnerHTML` over a `.md` file an agent
        * wrote.
        *
        * The prose is **not searchable by the find bar**, and that is a known edge rather than an
        * oversight: `Marked` wraps the strings this file lays out, and the preview lays out its
        * own. The count cannot lie about it — it is read off the marks that were actually drawn —
        * so the bar says what it can find, and Documents still offers the file to open in an
        * editor, which has a find bar of its own.
        */}
      {prose && proposalDoc !== null && props.proposal !== null && (
        <Block id="proposal" title={PROPOSAL_TITLE}>
          <div className={styles.prose} data-audit="specTabProposal">
            {/*
              * `scrolls={false}`: this rendering is a section of a page, not a scrollport.
              *
              * The preview reserves 40vh under its last block so the end of a document can be
              * scrolled to the top of the pane — which is what makes the editor's split view
              * usable at the bottom of a file, and which here was a screenful of blank between
              * the proposal and the Steps below it.
              */}
            <MarkdownPreview
              doc={parseMarkdown(props.proposal.text)}
              path={props.proposal.path}
              onNavigate={props.onNavigate}
              scrolls={false}
            />
          </div>
          {/*
            * No file row under the prose.
            *
            * One shipped, on the reasoning that removing the proposal from Documents left the
            * file with no opener — and it reads as exactly what it is: a link to the document
            * whose entire text is printed immediately above it. The prose *is* the proposal; a
            * row offering to open it is the page apologising for having drawn it.
            *
            * The only case that still needs a way through is the truncated one, and it has one
            * of its own below, because there the page genuinely is not showing everything.
            */}
          {props.proposal.truncated && (
            <button
              type="button"
              className={styles.doc}
              data-audit="specTabProposalCut"
              data-artifact={proposalDoc.id}
              data-state={proposalDoc.state}
              onClick={() => props.onOpenDoc(props.proposal?.path ?? '')}
            >
              <Icon name={asIcon('file-text')} size={1} />
              <span className={styles.docName}>
                This proposal is longer than cide draws here — open the file to read the rest
              </span>
            </button>
          )}
        </Block>
      )}

      <Block id="steps" title="Steps">
        {view.tasks.length === 0 ? (
          <p className={styles.empty}>
            Nothing in the task list yet — that is what OpenSpec’s propose writes, and what an agent
            works through.
          </p>
        ) : (
          <ul className={styles.steps}>
            {view.tasks.map((step, index) => (
              <li
                key={`${index}:${step.description}`}
                className={styles.step}
                data-audit="specTabStep"
                data-done={step.done ? 'true' : 'false'}
              >
                <Icon name={asIcon(step.done ? 'check' : 'square')} size={1} />
                <span>
                  <Marked text={step.description} query={query} counter={counter} current={current} />
                </span>
              </li>
            ))}
          </ul>
        )}
      </Block>

      <Block id="deltas" title="What this changes">
        {view.deltas.length === 0 && (
          <p className={styles.empty}>
            No requirement edits. A change with none cannot be archived — see the accept gesture on
            its task.
          </p>
        )}
        {view.deltas.map((delta, deltaIndex) =>
          delta.requirements.map((requirement, index) => {
            const target = `d${deltaIndex}.r${index}`
            const issues = issuesFor(view.validation, requirement.name).filter(isBlocking)
            return (
              <article
                key={target}
                className={styles.delta}
                data-audit="specTabDelta"
                data-op={delta.op}
                data-spec={delta.spec}
              >
                <div className={styles.deltaHead}>
                  <span className={styles.op} data-op={delta.op}>
                    {opLabel(delta.op)}
                  </span>
                  <span className={styles.deltaName}>
                    <Marked text={requirement.name} query={query} counter={counter} current={current} />
                  </span>
                  <span className={styles.deltaSpec}>{delta.spec}</span>
                  <button
                    type="button"
                    className={styles.edit}
                    data-audit="specTabEdit"
                    data-target={target}
                    data-write="true"
                    title="Edit this requirement"
                    onClick={() => props.onEditOpen(target, requirement)}
                  >
                    <Icon name={asIcon('pencil')} size={1} label="Edit" />
                  </button>
                </div>
                {edit.target === target && edit.draft !== null ? (
                  <RequirementEditor
                    draft={edit.draft}
                    busy={edit.busy}
                    problem={edit.problem}
                    onDraft={props.onEditDraft}
                    onCancel={props.onEditCancel}
                    onSave={() => props.onEditSave({ spec: delta.spec, op: delta.op }, requirement.name)}
                  />
                ) : (
                  <>
                    <p className={styles.deltaText}>
                      <Marked text={requirement.text} query={query} counter={counter} current={current} />
                    </p>
                    {requirement.scenarios.map((scenario, at) => (
                      <div key={at} className={styles.scenario} data-audit="specTabScenario">
                        <p className={styles.scenarioTitle}>
                          <Marked text={scenario.title} query={query} counter={counter} current={current} />
                        </p>
                        <pre className={styles.scenarioBody}>
                          <Marked text={scenario.body} query={query} counter={counter} current={current} />
                        </pre>
                      </div>
                    ))}
                    {issues.map((issue, at) => (
                      <p key={at} className={styles.problem} data-audit="specTabIssue">
                        {issue.message}
                      </p>
                    ))}
                  </>
                )}
              </article>
            )
          }),
        )}
      </Block>
    </div>
  )
}

/**
 * A path as it reads inside the change — `specs/acl-engine/spec.md`.
 *
 * Every capability's delta is called `spec.md`, so a basename is not a name: a change touching
 * two capabilities drew two rows that read identically, which is what "it shows spec.md twice"
 * was. Falls back to the whole path when the change's own directory is not in it, which should
 * not happen and is not worth hiding if it does.
 */
function relativeTo(change: string, path: string): string {
  const marker = `/${change}/`
  const at = path.lastIndexOf(marker)
  return at < 0 ? path : path.slice(at + marker.length)
}

/**
 * The find bar.
 *
 * Enter and Shift+Enter step, Escape shuts it — the three keys every find bar has had since
 * before any of us, and the reason none of them is configurable here.
 */
function FindBar({
  find,
  onFind,
  total,
}: {
  find: { query: string; index: number }
  onFind: (find: { query: string; index: number } | null) => void
  total: number
}) {
  const field = useRef<HTMLInputElement | null>(null)
  /*
   * Focus the box when the bar opens, explicitly.
   *
   * `autoFocus` was doing this and did not: React applies it when the node mounts, and the chord
   * that opens the bar is handled on `window` in the **capture** phase — so the browser's own
   * default for Ctrl+F and whatever had focus when it was pressed both get their say around the
   * same tick, and the caret was left wherever it started. Pressing Ctrl+F and then typing put
   * the query nowhere.
   *
   * A layout effect rather than an effect, so the caret is in the field before the frame paints
   * and the first keystroke cannot be dropped. `select()` as well as `focus()`: reopening the bar
   * on a query that is already there should replace it, the way every find bar does.
   */
  useLayoutEffect(() => {
    field.current?.focus()
    field.current?.select()
  }, [])
  return (
    <div className={styles.find} data-audit="specTabFind">
      <Icon name={asIcon('search')} size={1} />
      <input
        ref={field}
        className={styles.findInput}
        data-audit="specTabFindInput"
        data-write="true"
        placeholder="Find in this change"
        value={find.query}
        onChange={(event) => onFind({ query: event.target.value, index: 0 })}
        onKeyDown={(event) => {
          if (event.key === 'Escape') {
            event.preventDefault()
            onFind(null)
            return
          }
          if (event.key === 'Enter') {
            event.preventDefault()
            onFind({
              query: find.query,
              index: stepMatch(find.index, event.shiftKey ? -1 : 1, total),
            })
          }
        }}
      />
      <span className={styles.findCount} data-audit="specTabFindCount">
        {total === 0 ? (find.query.trim() === '' ? '' : 'no matches') : `${find.index + 1}/${total}`}
      </span>
      <button
        type="button"
        className={styles.findClose}
        data-audit="specTabFindClose"
        title="Close (Escape)"
        onClick={() => onFind(null)}
      >
        <Icon name={asIcon('x')} size={1} />
      </button>
    </div>
  )
}

/**
 * The change page's host: everything the view is not allowed to touch.
 *
 * The reads, the task lookup, the editor's draft and every write. `SpecTabView` above stays a
 * function of its props so `check:openspec-render` can SSR it — `renderToStaticMarkup` runs no
 * effects, so a page that fetched its own contents would be gated on its spinner and nothing
 * else.
 */
function ChangeTab({
  project,
  active,
  change,
}: {
  project: ProjectId
  active: boolean
  change: string
}) {
  const board = useSpec((state) => state.board)
  const [view, setView] = useState<ChangeView | null>(null)
  const [reading, setReading] = useState(true)
  const [failed, setFailed] = useState<string | null>(null)

  const [target, setTarget] = useState<string | null>(null)
  const [draft, setDraft] = useState<RequirementDraft | null>(null)
  const [busy, setBusy] = useState(false)
  const [problem, setProblem] = useState<
    { kind: 'regressed' | 'conflicted'; messages: readonly string[] } | null
  >(null)
  const [proposal, setProposal] = useState<
    { path: string; text: string; truncated: boolean } | null
  >(null)
  const [find, setFind] = useState<{ query: string; index: number } | null>(null)
  const [matches, setMatches] = useState(0)
  /*
   * The total, as a ref as well as state.
   *
   * The key listener is bound once per `active` and would otherwise close over the count as it
   * was at bind time — so F3 would step within "0 matches" for ever. Re-binding on every count
   * change would instead reattach a window listener on each keystroke of the query.
   */
  const matchesRef = useRef(0)
  matchesRef.current = matches

  const page = useRef<HTMLDivElement | null>(null)

  /*
   * Re-read on every board move, which is what makes the page follow an agent: the watcher route
   * for a run's own worktree fires `cide://spec-changed`, `specStore` adopts it, and this asks
   * again. Without the dependency the page would be a snapshot of the moment it was opened.
   */
  useEffect(() => {
    let live = true
    void specApi
      .change(project, change as ChangeName)
      .then((wire) => {
        if (!live) return
        setReading(false)
        const next = adaptChange(wire)
        setView(next)
        // A change that cannot be read is a state to draw, not an empty page — most often it has
        // just been archived, which is a thing the user did and should be told about.
        setFailed(
          next === null ? 'This change could not be read. It may have been archived.' : null,
        )
      })
      .catch((error: unknown) => {
        if (!live) return
        setReading(false)
        setFailed(String(error))
      })
    return () => {
      live = false
    }
  }, [project, change, board])

  /*
   * The proposal's text: a second read, keyed on the *path* rather than on `view`.
   *
   * Keyed on the path because that is what decides which bytes to fetch, and because the change
   * object is a fresh value on every board move — an effect that depended on it would re-read the
   * file every time an agent ticked a box, on a page that is otherwise already re-reading four
   * subprocesses. `board` is in the deps as well so that editing the proposal itself still
   * refreshes it; that is a file read, which is cheap, and it is the one signal that the prose on
   * screen has gone stale.
   *
   * Reset to `null` first, so a change whose proposal cannot be read does not keep drawing the
   * previous change's prose under the new change's title.
   */
  const proposalPath = useMemo(() => {
    if (view === null) return null
    const artifact = proposalArtifact(view)
    return artifact?.existing[0] ?? null
  }, [view])

  useEffect(() => {
    if (proposalPath === null) {
      setProposal(null)
      return
    }
    let live = true
    void specApi
      .artifact(project, proposalPath)
      .then((answer) => {
        if (!live) return
        setProposal(
          answer === null
            ? null
            : { path: proposalPath, text: answer.text, truncated: answer.truncated },
        )
      })
      .catch(() => {
        if (live) setProposal(null)
      })
    return () => {
      live = false
    }
  }, [project, proposalPath, board])

  const tasks = useTasks((state) => state.board)
  const task = useMemo(() => {
    if (tasks.kind !== 'ready') return null
    const found = tasks.tasks.find((row) => row.change === change)
    return found === undefined
      ? null
      : { id: found.id, agent: found.agent, status: found.status, session: found.session }
  }, [tasks, change])
  /*
   * The tracker's arm, kept beside `task` — because `task === null` means *no task* only when
   * the board is `ready`, and on the other three arms it means nobody has looked. Drawing them
   * the same way is what made this page offer *Start work* on a change with a finished task, on
   * a button that then did nothing at all: see `rowAction` in `model.ts`, which holds the rule.
   */
  /*
   * Does the tracked task's role run in its own checkout? `null` for no role, or no roster yet.
   *
   * Read the same way `TaskDetailHost` reads it, from the same store and the same field, so the
   * page and the card cannot name the accept gesture differently — see `SpecTabViewProps.roleWorktree`.
   * A stored value out of the selector (`check:selectors`' rule) with the lookup memoised on the
   * roster's identity, which `adopt` replaces wholesale on every broadcast.
   */
  const roster = useAgents((s) => s.roster)
  const roleWorktree = useMemo(() => {
    const agent = task?.agent ?? null
    if (agent === null || roster.kind !== 'ready') return null
    return roster.agents.find((def) => def.id === agent)?.worktree ?? null
  }, [roster, task])

  const startAction = rowAction(task === null ? null : task.id, tasks.kind)
  /**
   * *Split work*'s offer. Takes no task id — the button is drawn only while there is none — but
   * still asks the tracker's arm, because a control that writes must not be live while nobody
   * knows what is already on the board.
   */
  const split = splitAction(tasks.kind)

  const onStart = useCallback(() => {
    const store = useTasks.getState()
    if (task !== null) {
      store.select(task.id as never)
      return
    }
    void store
      .create({ ...EMPTY_DRAFT, title: change, change })
      .then(() => {
        const next = useTasks.getState().board
        // Silence after a click is what this path was reported for. The write may well have
        // landed — what failed is finding the row it made — so the sentence says that.
        if (next.kind !== 'ready') {
          notify(`Created a task for ${change}, but the task board has not answered yet.`, {
              kind: 'warn',
            })
          return
        }
        const made = next.tasks.filter((row) => row.change === change).at(-1)
        if (made === undefined) {
          notify(`Created a task for ${change}, but it is not on the board yet.`, {
              kind: 'warn',
            })
          return
        }
        useTasks.getState().select(made.id as never)
      })
      .catch(notifyFailure)
  }, [task, change])

  /**
   * Ask the project's conversation to fan this change out into a main task and subtasks.
   *
   * **cide writes nothing here**, which is the difference from `onStart` above and the reason
   * this has no board-watching afterwards: the conversation creates the main task, sets `change`
   * on it, and hangs the pieces off it as `subtaskOf` children, so one author owns the whole
   * decomposition. What cide does is type the line and take the user to where the answer will
   * appear.
   *
   * Uncaught by design. Rust refuses a change that already has a task — naming it — a project
   * with no Claude session, and a Claude tab that is not running, each with a sentence to act on,
   * and `chrome/Failures.tsx` is what puts it on screen. Swallowing them here is how this button
   * would join the list of ones that looked broken.
   *
   * The reveal is `ProposeDialog`'s, for its reason: `spec_split_work` types into the project's
   * *primary* session, and typing into a tab nobody is looking at is indistinguishable from
   * nothing having happened. `consolePaneOf` is the helper Ctrl+1 uses rather than a second walk
   * to `tabs[0]`, and `revealPane` reports rather than throws — the line has already landed, so a
   * red toast about the reveal would be a lie about what happened.
   */
  const onSplit = useCallback(() => {
    void specApi.splitWork(project, change as ChangeName).then(async () => {
      /*
       * **Imported here and not at the top of the file, and that is not a style choice.**
       *
       * `revealPane` and `useWorkspace` both reach `layout/paneHosts`, which imports xterm — and
       * this module exports `SpecTabView`, which `check:openspec-render` SSR-bundles and runs
       * under node, where xterm's addon bundle dies on a bare `self`. Static imports here took
       * the whole render check down. `ProposeForm` is split out of `ProposeDialog` for exactly
       * this reason; the page cannot be split the same way, so the reach happens at the moment
       * the button is pressed instead, where there is a browser.
       */
      const [{ revealPane }, { consolePaneOf }, { useWorkspace }] = await Promise.all([
        import('@/editor/revealPane'),
        import('@/keys/target'),
        import('@/store/workspace'),
      ])
      const target = consolePaneOf(useWorkspace.getState().boot)
      if (target !== null) await revealPane(target.project, target.pane)
    })
  }, [project, change])

  const onEditSave = useCallback(
    (delta: { spec: string; op: string }, requirement: string) => {
      if (draft === null) return
      setBusy(true)
      setProblem(null)
      void specApi
        .setRequirement({
          project,
          change: change as ChangeName,
          spec: delta.spec as SpecId,
          operation: delta.op as never,
          // The name it had when the editor opened, never the draft's: renaming a requirement is
          // a RENAMED delta, and Rust refuses a header that does not match the block it is
          // replacing. Sending the new name would ask it to replace something that is not there.
          requirement,
          block: compose(draft),
        })
        .then((outcome) => {
          setBusy(false)
          if (outcome.kind === 'written') {
            setTarget(null)
            setDraft(null)
            void useSpec.getState().refresh()
            return
          }
          // Both failures leave the form up with the typing in it — see `RequirementEditor`.
          setProblem(
            outcome.kind === 'conflicted'
              ? { kind: 'conflicted', messages: [outcome.path] }
              : { kind: 'regressed', messages: outcome.issues.map((issue) => issue.message) },
          )
        })
        .catch((error: unknown) => {
          setBusy(false)
          notifyFailure(error)
        })
    },
    [project, change, draft],
  )

  /*
   * Ctrl+F, scoped to this page.
   *
   * A **local** handler and not a keymap binding, because `cide_core::keymap` has a test
   * forbidding a default for `ctrl+f`: CodeMirror means its find bar by it and a terminal means
   * its own focus-scoped chord, and a window-capture gate would take it from both at once. Every
   * surface that wants the chord handles it where it can tell whether the event is its own — for
   * this page, that is a listener on its own subtree.
   */
  useEffect(() => {
    // Only the tab on screen listens. Every tab is mounted — hidden ones are
    // `visibility: hidden`, not unmounted — so without this every open change would answer the
    // chord at once.
    if (!active) return
    const onKey = (event: KeyboardEvent) => {
      /*
       * F3 and Shift+F3 step, which is what every editor on this platform means by them — and
       * they work whether or not the caret is in the find box, because the point of the key is
       * stepping *while reading the page*.
       *
       * Handled here for the same reason Ctrl+F is: `cide_core::keymap` has a test
       * (`nothing_binds_the_find_bars_f_keys`) forbidding a default for F3, because inside an
       * editor it is `@codemirror/search`'s find-next and a window-capture binding would take it
       * from the buffer and the find field at once. A page-local handler is the shape that test
       * leaves room for — and an OpenSpec tab replaces the pane tree, so there is no editor in
       * this tab to take it from.
       *
       * `find === null` ignores them: F3 with no search running has nothing to step through, and
       * opening the bar on it would be a surprise. Nothing is consumed in that case either, so
       * the key stays available to whatever else might want it.
       */
      if (event.key === 'F3' && !event.ctrlKey && !event.metaKey && !event.altKey) {
        setFind((current) =>
          current === null
            ? null
            : {
                query: current.query,
                index: stepMatch(current.index, event.shiftKey ? -1 : 1, matchesRef.current),
              },
        )
        if (find !== null) {
          event.preventDefault()
          event.stopPropagation()
        }
        return
      }
      if (event.key !== 'f' || !(event.ctrlKey || event.metaKey) || event.altKey) return
      // Not while the caret is in a field this page owns: Ctrl+F inside the requirement editor
      // is the editor's, and stealing it there would be this page doing to its own textarea what
      // a global binding would have done to CodeMirror.
      const target = event.target
      if (
        target instanceof HTMLElement &&
        (target.tagName === 'TEXTAREA' ||
          (target.tagName === 'INPUT' && target.getAttribute('data-audit') !== 'specTabFindInput'))
      ) {
        return
      }
      event.preventDefault()
      event.stopPropagation()
      setFind((current) => current ?? { query: '', index: 0 })
      // Already open: the bar will not remount, so its own mount effect cannot put the caret
      // back. Ctrl+F on an open bar means *give me the box again*, which is what every editor
      // does with it.
      const field = page.current?.querySelector('[data-audit="specTabFindInput"]')
      if (field instanceof HTMLInputElement) {
        field.focus()
        field.select()
      }
    }
    /*
     * On `window`, in the **capture** phase, and this is the whole of why the first version did
     * nothing.
     *
     * It listened on the page's own element, which only sees an event that bubbles *through* it —
     * and a click on prose leaves focus on `document.body`, so the keydown never went near the
     * page. Capture on the window sees it wherever focus is, and `active` is what keeps it from
     * being a global binding, which `cide_core::keymap` has a test forbidding.
     */
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [active, find])

  /*
   * The match count, read from the marks the page actually drew.
   *
   * It used to be a second walk over the model — the host counting the same strings the view was
   * about to mark — and the two disagreed, which is how a page showing three hits reported
   * fifteen. Any parallel count is a second implementation of "what is on this page" that has to
   * be kept in step by hand with the one that renders, and there is no way to notice when it
   * drifts: both numbers look plausible.
   *
   * So there is one implementation now. `Marked` assigns each hit its index as it renders, and
   * this counts the elements that came out. It cannot disagree with the screen because it *is*
   * the screen.
   *
   * `useLayoutEffect` with no dependency array, guarded by an equality check: it runs after every
   * render, and the guard is what stops the state it sets from causing another one.
   */
  useLayoutEffect(() => {
    const node = page.current
    if (node === null) return
    const total = node.querySelectorAll('[data-audit="specTabMark"]').length
    setMatches((current) => (current === total ? current : total))
  })

  /*
   * Bring the current hit into view — including out of a section somebody has collapsed.
   *
   * A `<details>` that is shut still has its content in the DOM, so a hit inside one is counted
   * and addressable and completely invisible. Opening the ancestors is what keeps "15 matches"
   * from meaning "3 you can see and 12 you cannot".
   *
   * `block: 'center'`, so a match at the bottom of a long requirement is not left at the very
   * edge of the fold it was scrolled to.
   */
  useEffect(() => {
    if (find === null) return
    const node = page.current?.querySelector(`[data-hit="${find.index}"]`)
    if (!(node instanceof HTMLElement)) return
    for (let at: HTMLElement | null = node; at !== null; at = at.parentElement) {
      if (at instanceof HTMLDetailsElement) at.open = true
    }
    node.scrollIntoView({ block: 'center' })
  }, [find])

  /*
   * A link inside the proposal.
   *
   * `MarkdownFrame` has the same three arms for the same three reasons, and the external one is
   * the one worth restating: cide does not navigate to a URL from a webview. The JS opener command
   * is capability-gated per window and a detached-pane window deliberately has none, so a JS-side
   * open would work in the shell window and silently do nothing in a detached one — see
   * `terminal/xterm.ts` for the argument in full. `links.ts` refuses to resolve a scheme at all,
   * which is what makes `javascript:` and `data:` unreachable rather than merely unlisted.
   */
  const onNavigate = useCallback(
    (href: string) => {
      const kind = targetKind(href)
      if (kind === 'fragment') {
        const id = href.slice(1)
        // `[top](#)` means the top, and would otherwise be `querySelector('#')` — a syntax error
        // thrown out of a click handler with no boundary above it.
        if (id === '') {
          page.current?.scrollTo({ top: 0 })
          return
        }
        page.current?.querySelector(`#${CSS.escape(id)}`)?.scrollIntoView({ block: 'start' })
        return
      }
      if (kind === 'local') {
        const resolved = proposalPath === null ? null : resolveLocal(dirOf(proposalPath), href)
        if (resolved === null) {
          notify(`cide cannot resolve ${href} from this proposal`, { kind: 'warn' })
          return
        }
        void file.open(project, resolved).catch(notifyFailure)
        return
      }
      notify(`cide does not open external links: ${href}`, { kind: 'warn' })
    },
    [project, proposalPath],
  )

  return (
    <SpecTabView
      ref={page}
      proposal={proposal}
      onNavigate={onNavigate}
      find={find}
      matches={matches}
      onFind={setFind}
      view={view}
      failed={failed}
      reading={reading}
      task={task}
      roleWorktree={roleWorktree}
      edit={{ target, draft, busy, problem }}
      startAction={startAction}
      onStart={onStart}
      splitAction={split}
      onSplit={onSplit}
      onOpenDoc={(path) => void file.open(project, path).catch(notifyFailure)}
      onEditOpen={(next, requirement) => {
        setTarget(next)
        setDraft(draftOf(requirement))
        setProblem(null)
      }}
      onEditDraft={setDraft}
      onEditCancel={() => {
        setTarget(null)
        setDraft(null)
        setProblem(null)
      }}
      onEditSave={onEditSave}
    />
  )
}

/**
 * A capability's page: what the specs say today.
 *
 * Read-only, and that is the design rather than a gap. `openspec/specs/` is written by `archive`
 * and by nothing else — editing a requirement there directly would put behaviour into the source
 * of truth with no change, no proposal and no review behind it, which is the whole thing OpenSpec
 * exists to prevent. The action is *Propose a change to this*, which is the road that leads back
 * here properly.
 */
function CapabilityPage({ project, spec }: { project: ProjectId; spec: string }) {
  const [text, setText] = useState<string | null>(null)
  const [path, setPath] = useState<string | null>(null)
  const board = useSpec((state) => state.board)

  useEffect(() => {
    if (board.kind !== 'ready') return
    let live = true
    const file = `${board.root}/openspec/specs/${spec}/spec.md`
    void specApi
      .artifact(project, file)
      .then((answer) => {
        if (!live) return
        setText(answer?.text ?? null)
        setPath(answer === null ? null : file)
      })
      .catch(() => {
        if (live) setText(null)
      })
    return () => {
      live = false
    }
  }, [project, spec, board])

  /*
   * A link inside a capability, resolved the way the proposal's are.
   *
   * The same three arms as `ChangeTab`'s `onNavigate` and for the same reasons — the shared
   * helper is `targetKind`/`resolveLocal`, and the external arm refuses rather than shelling
   * out, because opening a URL is capability-gated per window and a detached-pane window has no
   * such capability.
   */
  const onNavigate = useCallback(
    (href: string) => {
      if (path === null) return
      const kind = targetKind(href)
      if (kind === 'local') {
        const resolved = resolveLocal(dirOf(path), href)
        if (resolved === null) {
          notify(`cide cannot resolve ${href} from this spec`, { kind: 'warn' })
          return
        }
        void file.open(project, resolved).catch(notifyFailure)
        return
      }
      if (kind === 'external') {
        notify(`cide does not open external links: ${href}`, { kind: 'warn' })
      }
    },
    [project, path],
  )

  /*
   * Parsed once per read, not once per render: `parseMarkdown` walks the whole document and a
   * capability's `spec.md` is the longest file OpenSpec keeps — it accumulates every requirement
   * every archived change ever added.
   */
  const doc = useMemo(() => (text === null ? null : parseMarkdown(text)), [text])

  return (
    <div className={styles.page} data-audit="specTab" data-spec={spec}>
      <header className={styles.head}>
        <Icon name={asIcon('file-text')} size={2} />
        <h1 className={styles.title} data-audit="specTabTitle">
          {spec}
        </h1>
      </header>
      {doc === null || path === null ? (
        <p className={styles.reading} data-audit="specTabReading">
          Reading {spec}…
        </p>
      ) : (
        /*
         * Rendered, not printed. This drew the file in a `<pre>` — every `###`, every `-`, every
         * pipe table exactly as written — which is the one presentation a *spec* cannot afford:
         * the whole document is nested headings and bullets, so unrendered it is a wall with no
         * structure at all, and the page that draws a change's proposal properly two files away
         * was drawing the requirements it archived as source.
         *
         * `MarkdownPreview` and not a second renderer, and `scrolls={false}`, for the two reasons
         * the proposal block states above: `ui/src/editor/markdown/` is the hand-written one with
         * `check:markdown` behind it and no node it produces can carry markup, and the preview's
         * 40vh tail is for a scrollport, which a section of a page is not.
         */
        <div className={cx(styles.prose, styles.specProse)} data-audit="specTabBody">
          <MarkdownPreview doc={doc} path={path} onNavigate={onNavigate} scrolls={false} />
        </div>
      )}
    </div>
  )
}

/** A change or a capability. Exactly one of the two props. */
function SpecTabImpl({ project, active, change, spec }: SpecTabProps) {
  if (spec !== undefined) return <CapabilityPage project={project} spec={spec} />
  if (change !== undefined)
    return <ChangeTab project={project} active={active} change={change} />
  // Neither: a tab kind that carries no subject cannot exist — `SpecSubject` has two arms and
  // both fill one of these — so this is belt and braces rather than a state to design for.
  return null
}

export const SpecTab = memo(SpecTabImpl)
