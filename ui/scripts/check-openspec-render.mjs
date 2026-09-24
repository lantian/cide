/**
 * SSR-renders every OpenSpec panel story and asserts on the markup. (M28)
 *
 * `check-agents-render.mjs`'s shape: Vite builds `smokeEntry.tsx` as an SSR bundle into
 * `node_modules/.cache` (it must be under `node_modules`, because the bundle keeps
 * `react-dom/server` external), node imports it, and the **last** `console.log` line is one JSON
 * array of digests.
 *
 * `check:openspec` drives the pure model directly. What this adds is the half a model cannot
 * speak for — what the markup actually contains.
 *
 * Run: `pnpm --dir ui run check:openspec-render`
 */
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const UI = resolve(import.meta.dirname, '..')
mkdirSync(join(UI, 'node_modules/.cache'), { recursive: true })
const out = mkdtempSync(join(UI, 'node_modules/.cache', 'cide-openspec-render-'))

let failed = 0
const fail = (what, detail) => {
  failed += 1
  console.error(`FAIL ${what}${detail === undefined ? '' : `\n  ${detail}`}`)
}
const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) fail(what, `actual:   ${a}\n  expected: ${b}`)
}
const ok = (cond, what) => {
  if (cond !== true) fail(what)
}
const read = (rel) => readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8')

try {
  execFileSync(
    'node',
    [
      'node_modules/vite/bin/vite.js',
      'build',
      '--ssr',
      'src/sidebar/OpenSpecPanel/smokeEntry.tsx',
      '--outDir',
      out,
      '--logLevel',
      'error',
    ],
    { cwd: UI, stdio: 'inherit' },
  )

  // `@tauri-apps/api` touches `window` on import. Nothing deeper is faked.
  globalThis.window = globalThis
  globalThis.location = { search: '' }

  const printed = []
  const say = console.log
  console.log = (line) => printed.push(String(line))
  try {
    await import(`file://${resolve(out, 'smokeEntry.js')}`)
  } finally {
    console.log = say
  }
  const stories = Object.fromEntries(
    JSON.parse(printed.at(-1)).map((digest) => [digest.story, digest]),
  )
  const t = (name) => {
    const digest = stories[name]
    if (digest === undefined) throw new Error(`no story ${name}`)
    return digest
  }

  /* ------------------------------------------- unknown draws nothing; absent draws a pitch */

  {
    const unknown = t('unknown')
    eq(unknown.panel, true, 'the panel frame renders even before anything has been read')
    eq(unknown.claim, null, 'an unknown board makes no claim about this project')
    eq(
      unknown.path,
      null,
      'and above all names no directory: Set up writes a folder the repository will contain, ' +
        'and offering that on the strength of not having looked is proposing a commit blind',
    )
    eq(unknown.setUp, null, 'so there is no Set up button at all')
    eq(unknown.writeControls, 0, 'and nothing that writes')
    eq(unknown.meta, null, 'a header that counted would be claiming to have looked')
    /*
     * ------------------------------------------------ the configuration gear, in the header (M28)
     *
     * `openspec/config.yaml` was edited under Settings, and cide's Settings is **global**: nine of
     * its eleven sections write `Settings`, which follows a person into every repository they
     * open. This is one committed YAML file in one project, so a section in that list said it was
     * a preference. It is a gear on this panel's header instead — the surface that is already
     * scoped to exactly one project — which states the scope by where the control is rather than
     * by a paragraph under a heading that contradicts it.
     *
     * In the *header* and not in the tree, which is what answers the objection the old placement
     * raised: this panel is a board, and a change row is not a surface anybody opened in order to
     * edit a committed file. A panel header is where a panel's own affordances live.
     */
    eq(
      unknown.configure,
      false,
      'no configuration gear before anything has been read — a control offering to configure a ' +
        'project whose state cide cannot describe does not know what it would open',
    )
    eq(
      unknown.writeControls,
      0,
      'and the gear is not counted as one when it is drawn: it opens a dialog, and the Save ' +
        'inside that dialog is the thing that writes',
    )
    for (const name of ['absent', 'board', 'board-empty', 'cli-missing', 'unusable']) {
      eq(
        t(name).configure,
        true,
        `${name}: every other arm draws the gear — the file is read and written by cide's own ` +
          `scanner with no subprocess, so a machine with no \`openspec\` on PATH is one where ` +
          `this still works, and on \`absent\` the dialog is the init wizard`,
      )
    }

    const absent = t('absent')
    ok(absent.claim !== null && absent.claim.length > 0, 'the absent screen states its claim')
    ok(
      absent.detail !== null && absent.detail.includes('openspec/specs/'),
      `and names the real directories — the brand is shown, not hidden: ${absent.detail}`,
    )
    eq(absent.path, '/home/dev/work/thing/openspec', 'and names what would be created')
    eq(
      absent.pathBeforeSetUp,
      true,
      'the path is printed BEFORE the button, so it is read before the click rather than after',
    )
    eq(absent.setUp, 'on', 'and the button is live')
    eq(absent.setUpLabel, 'Set up OpenSpec', 'and says what it is at rest')
    eq(absent.setUpMark, false, 'with no loader mark — a mark at rest would claim init is running')

    /*
     * The in-flight state is a mark and a present participle, never only `disabled`. Init is
     * two node subprocesses and takes seconds; the button shipped merely 0.55-opacity dimmer
     * for all of them, and a slightly dimmer label was read as a missed click.
     */
    const busy = t('absent-busy')
    eq(busy.setUp, 'off', 'a Set up in flight is inert, so `init` cannot run twice')
    eq(busy.setUpLabel, 'Setting up…', 'and its words say init is running, not a frozen label')
    eq(busy.setUpMark, true, 'with the loader mark the run strip already taught')
  }

  /* ----------------------------------------------- a board cide could not read writes nothing */

  for (const name of ['cli-missing', 'unusable']) {
    const digest = t(name)
    eq(digest.setUp, null, `${name}: no Set up — cide could not read what is already there`)
    eq(digest.retry, true, `${name}: Retry is the way out`)
    eq(
      digest.writeControls,
      1,
      `${name}: Retry and nothing else writes — a board cide cannot read must not be written over`,
    )
    ok(
      digest.detail !== null && digest.detail.length > 0,
      `${name}: the sentence from Rust is drawn, not summarised`,
    )
  }
  ok(
    t('cli-missing').detail.includes('desktop launcher'),
    'the CLI-missing sentence carries the launcher-PATH difference, which is the whole answer ' +
      'to "but I have it installed"',
  )
  ok(t('cli-missing').install.includes('@fission-ai/openspec'), 'and the install command')
  ok(
    t('unusable').detail !== t('cli-missing').detail,
    'one board arm, two sentences — which is why Rust composes them and the panel does not',
  )

  /* ------------------------------------------------------------------------- the tree */

  {
    const board = t('board')
    eq(
      board.sections,
      ['changes|true', 'specs|true'],
      'both sections, both open, changes first — that is what is being worked on',
    )
    eq(
      board.rows,
      [
        'change|drop-legacy-theme|ready',
        'change|rework-auth|proposed',
        'change|add-dark-mode|inProgress',
        'spec|auth',
        'spec|dark-mode',
      ],
      'changes by remaining work ascending then name; specs alphabetical',
    )
    /*
     * Every change offers an action, and which one depends on whether a task already tracks it.
     *
     * This is what makes the list actionable at all: the whole lifecycle — approve, dispatch,
     * watch, accept — hangs off a *task* that names a change, and before this the panel could
     * list changes and reach none of them. `Start work` on a change that already had a task
     * would create a second one for it, so the two states are asserted together.
     */
    eq(
      board.rowActions,
      ['drop-legacy-theme|start', 'rework-auth|start', 'add-dark-mode|open'],
      'a change with a task opens it; one without offers to start the work',
    )
    /*
     * And the same board with the **task** tracker unread draws none of them live.
     *
     * The reported bug, exactly: `tasks` is empty on three of `Board`'s four arms and the panel
     * read empty as *no task*, so a change with a finished task got a confident `Start work` —
     * on a button that then did nothing at all, because `tasksStore.create` refuses `unknown`
     * and `unreadable` without a word. Note `add-dark-mode` greys too: its task is in the map,
     * but a map built from a board nobody has read is not evidence of anything.
     */
    eq(
      t('board-tracker-unread').rowActions,
      ['drop-legacy-theme|start|inert', 'rework-auth|start|inert', 'add-dark-mode|start|inert'],
      'a tracker that has not answered greys every row action rather than guessing',
    )

    ok(board.meta !== null && board.meta.includes('2 specs'), `the header counts: ${board.meta}`)
    eq(
      board.entryPoints,
      ['propose', 'explore'],
      'the two actions are drawn whatever the board holds. They lived inside a "no changes yet" ' +
        'screen and vanished the moment the first change appeared — the panel offering its ' +
        'actions exactly until there was reason to use them again',
    )
    eq(
      board.sectionHints,
      0,
      'and neither section explains itself while it has rows: a section with content is ' +
        'explained by its content, and four wrapped lines of caption over two of list is the ' +
        'caption burying the thing it captions',
    )

    // `0/0` is `proposed`, never `ready` — the same rule the Review hop turns on in Rust.
    ok(
      board.rows.includes('change|rework-auth|proposed'),
      'a change whose task list has no checkboxes is proposed, not ready',
    )
  }

  {
    const collapsed = t('board-collapsed')
    eq(collapsed.sections, ['changes|false', 'specs|false'], 'both sections report themselves shut')
    eq(collapsed.rows, [], 'and really draw none of their rows')
    eq(collapsed.sectionHints, 0, 'a shut section hides its hint with it')
  }

  {
    const empty = t('board-empty')
    eq(empty.rows, [], 'an empty board has no rows')
    eq(
      empty.entryPoints,
      ['propose', 'explore'],
      'the same two commands, which are the ones OpenSpec’s default profile actually installs. ' +
        '`onboard` is in the CLI’s templates and is NOT installed, which shipped once as a ' +
        'button that made Claude answer `Unknown command` with nothing in cide able to explain it',
    )
    eq(
      empty.sectionHints,
      2,
      'and *now* both sections explain themselves, because neither has anything else to show',
    )
  }

  /* ------------------------------------------------------------------- the composer */

  /*
   * It is a **modal** now, not a box in the sidebar. (M28)
   *
   * Two reasons, and the second decided it: a 320px column somebody dragged to the width they
   * wanted their *tree* at is the wrong place to write the description of a change — the most
   * consequential sentence this feature ever asks for — and the gesture had exactly one door,
   * behind opening the sidebar and switching it to OpenSpec. It is a command now (`spec.propose`,
   * `spec.explore`), so it is in the palette, and a command needs a surface that does not assume
   * the panel is on screen.
   *
   * So these stories render `ProposeFormView` rather than the panel, and what the *panel* still
   * owes is one thing: a button whose press opened something has to say so.
   */
  {
    for (const name of ['board', 'board-empty', 'absent', 'cli-missing']) {
      eq(t(name).ask, null, `${name}: the panel draws no composer of its own any more`)
    }
    eq(
      t('ask-propose').ask,
      null,
      'not even while it is open — the box is a portal at top level, and a second copy inside ' +
        'the panel would be two boxes over one piece of state',
    )
    eq(
      t('ask-propose').entryPoints,
      ['propose', 'explore'],
      'the buttons stay in the panel, because that is where somebody already looking at their ' +
        'changes expects them',
    )

    const open = t('propose')
    eq(open.ask, 'propose', 'the dialog draws the composer')
    eq(
      open.askSend,
      'off',
      'and Send is off until there is something to say — a bare propose is legal and ' +
        'useless: Claude answers by asking what to propose, which is the round trip we ' +
        'already had the user’s attention for',
    )
    ok(
      open.askPlaceholder !== null && open.askPlaceholder.length > 0,
      `with a placeholder saying what it wants: ${open.askPlaceholder}`,
    )
    eq(open.askPreview, '/openspec-propose', 'and the preview shows the bare command')

    const typed = t('propose-typed')
    eq(typed.askSend, 'on', 'a described change can be sent')
    eq(
      typed.askPreview,
      '/openspec-propose a dark theme that follows the system setting',
      'and the preview is the line that will be typed, verbatim',
    )

    /*
     * **The invocation comes off the board, and this story is the whole reason.**
     *
     * OpenSpec installed its workflow as slash commands (`/opsx:propose`) and then moved it to
     * Claude Code skills (`/openspec-propose`). cide had the old prefix in nine strings, so every
     * button in this panel previewed — and typed — a command no current project has, and refused
     * with a sentence recommending `openspec update`, which on those projects writes nothing.
     * Both surfaces still exist on disk in the wild, so the box draws whichever the project has;
     * a surface that resolves it itself is right for half the world and passes every other
     * assertion in this file.
     */
    eq(
      t('propose-legacy').askPreview,
      '/opsx:propose a dark theme',
      'a project set up by an older CLI previews its own spelling',
    )

    /*
     * The one that matters. The command is typed into a PTY and ended with `\r`, so a newline in
     * the middle submits the first half as a turn and feeds the rest in as further turns — the
     * failure that looks like a model answering nonsense. Rust flattens it again on the way
     * through, which is the guard that counts; this asserts the box *shows* the same thing it
     * sends, because displaying something other than what is sent is its own small lie.
     */
    const multiline = t('propose-multiline')
    eq(
      multiline.askPreview,
      '/openspec-propose a dark theme that follows the system setting',
      'a description typed across three lines previews as the one line that will be sent',
    )
    ok(
      !multiline.askPreview.includes('\n'),
      'and carries no newline, which would submit it in halves',
    )

    /* A send in flight: nothing that can be pressed twice, and nothing that can be typed into. */
    eq(t('propose-busy').askSend, 'off', 'a send in flight cannot be started again')
    eq(t('propose-busy').askInput, 'off', 'and the box cannot be typed into while it runs')
    eq(t('propose').askInput, 'on', 'while an idle one plainly can')

    // Explore is a mode, not a request, so it sends empty.
    const explore = t('explore')
    eq(explore.ask, 'explore')
    eq(explore.askSend, 'on', 'explore may be entered with nothing typed — it is a mode')
    eq(explore.askPreview, '/openspec-explore')
    ok(
      explore.askPlaceholder !== t('propose').askPlaceholder,
      'and asks a different question from propose’s, because it is one',
    )
  }

  /* ------------------------------------------------------------- the requirement editor */

  {
    const open = t('editor')
    eq(
      open.editFields,
      4,
      'name, behaviour, and one scenario’s title and body — four live fields and no more',
    )
    eq(open.editScenarios, 1, 'one scenario block')
    eq(
      open.editClauses,
      ['WHEN', 'THEN', 'AND'],
      'and three clause chips. AND is there because two clauses is a convention, not a rule — ' +
        'a scenario is free markdown and the chips teach the shape without owning it',
    )
    eq(open.editSave, 'on', 'a complete requirement can be saved')
    eq(open.editRefusal, null, 'with nothing refusing it')
    eq(open.editProblem, null, 'and nothing to report')

    // Save off, and the reason drawn as *text* — `disabled` tells a screen reader the control is
    // off and never why, and a title attribute is invisible to one.
    for (const name of ['editor-nameless', 'editor-no-scenarios']) {
      const digest = t(name)
      eq(digest.editSave, 'off', `${name}: Save is off`)
      ok(
        digest.editRefusal !== null && digest.editRefusal.length > 0,
        `${name}: and says why, in text rather than only in a tooltip`,
      )
    }
    ok(
      t('editor-nameless').editRefusal.toLowerCase().includes('name'),
      `the refusal names the field: ${t('editor-nameless').editRefusal}`,
    )

    // A save in flight leaves the form up and inert.
    eq(t('editor-busy').editSave, 'off', 'a save in flight cannot be pressed twice')
    eq(
      t('editor-busy').editFields,
      4,
      'and the form stays up holding every field the user typed into',
    )

    // Both failures keep the typing on screen. A save that closed the editor and reported
    // elsewhere would throw away the paragraph it failed to write.
    for (const [name, kind] of [
      ['editor-regressed', 'regressed'],
      ['editor-conflicted', 'conflicted'],
    ]) {
      const digest = t(name)
      eq(digest.editProblem, kind, `${name}: the failure is drawn as a state of this form`)
      eq(digest.editFields, 4, `${name}: with the draft still in it`)
      eq(digest.editSave, 'on', `${name}: and Save live again, so it can be fixed and retried`)
    }
  }

  /* ------------------------------------------------------------------- the change page */

  {
    // A read in flight is a *state*, not an empty page — and an empty page is what a change that
    // could not be read used to be. Most often it has just been archived, which is something the
    // user did and should be told about.
    const reading = t('tab-reading')
    eq(reading.tabDeltas, [], 'nothing is drawn while the read is in flight')
    eq(reading.tabTitle, null, 'not even a title, which would be a page claiming to have loaded')
    ok(
      (t('tab-failed').tabFailed ?? '').includes('deleted'),
      `and a change that could not be read says the likeliest reason: ${t('tab-failed').tabFailed}`,
    )

    /*
     * ------------------------------------------------------------------- and one that was archived
     *
     * `tab-failed` is what this page used to be. `openspec archive` **moves** the change to
     * `openspec/changes/archive/<stamp>-<name>/`; the CLI's `show` resolves
     * `openspec/changes/<name>/proposal.md` and nothing else, so from the moment work was
     * accepted the page said *"could not be read"* about a change whose every file was still on
     * disk — and the task that did the work lost its record at exactly the point it mattered.
     *
     * The three assertions below are each a different way the fix could be half-done, and two of
     * them are about what must **not** be drawn: an archive cannot answer the checklist or the
     * verdict, so those come back `0/0` and vacuously valid, which renders naively as an empty
     * bar reading *no steps planned yet* and a green *Valid* over finished, merged work.
     */
    const archived = t('tab-archived')
    eq(archived.tabStage, 'archived', 'the stage says what happened, not `proposed`')
    eq(archived.tabValidity, 'unchecked', 'nothing validated it and nothing can — never `ok`')
    eq(archived.tabProgress, null, 'and there is no progress bar at all, rather than an empty one')
    ok(
      (archived.tabArchived ?? '').includes('2026-08-27-add-dark-mode'),
      `the strip names the directory instead: ${archived.tabArchived}`,
    )
    eq(
      archived.tabDeltas,
      ['added|dark-mode'],
      'and the requirements are still there — recovered by the same scanner the write path uses, ' +
        'which is the whole point of reading the archive rather than refusing',
    )
    eq(archived.tabStart, null, 'nothing to start: the work is done and merged')

    const page = t('tab')
    eq(page.tabTitle, 'add-dark-mode', 'the page is titled by the change')
    eq(page.tabStage, 'inProgress', '3 of 9 steps is in progress')
    eq(page.tabValidity, 'ok', 'and it validates')
    eq(page.tabProgress, '33', 'the bar reports a real percentage')
    eq(
      page.tabDocs,
      ['proposal', 'tasks', 'design'],
      'every artifact the CLI reported is reachable — this page exists because opening one of ' +
        'five documents was the whole of what the panel could do',
    )
    eq(
      page.tabSteps,
      ['true', 'true', 'true', 'false'],
      'the checklist is drawn with each box in its real state',
    )
    eq(page.tabDeltas, ['added|dark-mode'], 'and the requirement edits it makes')
    eq(page.tabStart, 'Start work', 'a change nobody is working offers to start it')
    eq(t('tab-with-task').tabStart, 'Open t-14', 'and one with a task opens it')

    /*
     * *Split work*, and the three things its rendering has to say. (M31)
     *
     * **Drawn only while there is no task.** A change gets one task carrying `change` —
     * `spec_triggers::consider_one` finds a change's task by that field and, on more than one
     * match, moves *neither*, silently and for the life of the change. So the fan-out is offered
     * to a change nobody has started, and `tab-with-task` asserting `null` is that rule as
     * markup.
     *
     * **Inert on an unread board, with a sentence.** `task === null` means *no task* only when
     * the tracker is `ready`; `tab-tracker-unknown` is the arm where both controls must be grey,
     * and it is the story that had never been rendered before this button existed.
     *
     * **Between the primary and the hint.** The hint is about the primary action; a control
     * wedged between them reattributes it, and nothing else in the digest could show that — both
     * buttons would still be present, both still saying the right words.
     */
    eq(page.tabSplit, 'Split work', 'a change nobody is working also offers to split it')
    eq(
      t('tab-with-task').tabSplit,
      null,
      'and one that already has a task does not — a second task naming the change would switch ' +
        'the finished-checklist hop off for good',
    )
    eq(
      t('tab-tracker-unknown').tabSplit,
      'Split work|inert',
      'nor is it live before the task board has answered',
    )
    eq(
      t('tab-tracker-unknown').tabStart,
      'Start work',
      'the primary keeps its name while inert — an unnamed grey button is the complaint the ' +
        'whole reason-not-a-boolean convention exists for',
    )
    ok(page.tabSplitPlaced, 'split sits after the primary and before the hint about the primary')

    // Finished-and-valid against finished-and-refused: the pair the accept gesture turns on.
    eq(t('tab-ready').tabStage, 'ready', 'every box ticked and valid is ready')
    eq(t('tab-invalid').tabStage, 'invalid', 'every box ticked and refused is not')
    eq(t('tab-invalid').tabValidity, 'failed', 'and the badge says so')
    eq(
      t('tab-invalid').tabIssues,
      1,
      'with the complaint drawn beside the requirement that caused it — never a toast, because ' +
        'an error about a paragraph belongs next to the paragraph',
    )
    eq(t('tab-ready').tabIssues, 0, 'and a clean change shows none')

    /*
     * Every section collapses, and every one starts open.
     *
     * The opposite of `tasksHistory`'s rule one panel over, deliberately: that disclosure hides a
     * log nobody opened the card to read, while these are the page's content. A page that opened
     * with everything shut would answer *show me this change* with four headings.
     */
    eq(
      page.tabBlocks,
      ['docs|open', 'steps|open', 'deltas|open'],
      'three collapsible blocks, all expanded by default',
    )
    eq(
      t('tab-proposal').tabBlocks,
      ['docs|open', 'proposal|open', 'steps|open', 'deltas|open'],
      'and the proposal is a fourth, between the documents it came out of and the steps',
    )

    /*
     * The Documents block lists **every** artifact, not only the ones with a file.
     *
     * `design.md` is not written by propose and its status is `ready` — *this is the next
     * thing to write* — so listing only what exists made the page look like a change that had no
     * design rather than one whose design was still to come. An artifact with no file is
     * information.
     */
    eq(
      page.tabDocs,
      ['proposal', 'tasks', 'design'],
      'every artifact is drawn, each as a block of its own',
    )
    /*
     * Each row is labelled by its path **within the change**, never the basename.
     *
     * Every capability's delta is called `spec.md`, so a change touching two of them drew two
     * rows that read identically — the "spec.md twice". Inlining the documents' text was tried
     * here instead and reverted: the editor already opens markdown properly, and four inlined
     * files buried the parts of a change that exist nowhere else.
     */
    ok(
      page.tabDocPaths.every((path) => !path.startsWith('/')),
      `a row names its path within the change, not an absolute one: ${page.tabDocPaths}`,
    )
    // And every row reads differently from every other. Two capability deltas are both called
    // `spec.md`, so a label that led with the artifact id gave two rows that were identical to
    // read — which is what "it shows spec.md twice" was, twice over.
    eq(
      new Set(page.tabDocPaths).size,
      page.tabDocPaths.length,
      `two rows read identically: ${page.tabDocPaths}`,
    )
    eq(page.tabDocBodies, 0, 'and no document is inlined')

    /*
     * ------------------------------------------------------- the proposal, and the design marker
     *
     * The proposal is the one document a reader opens this page *for* — it is the change, in
     * prose — so it is rendered here rather than linked. Everything else stays a link on purpose:
     * `tasks.md` is already drawn as the checklist, the delta specs as requirement cards, and
     * `design.md` is genuinely a separate document.
     */
    const withProse = t('tab-proposal')
    eq(withProse.tabProposal, true, 'the proposal is drawn as prose')
    ok(
      withProse.tabProseBlocks >= 4,
      `and it went through the markdown renderer — a heading, a paragraph, a list and a fence ` +
        `is at least four top-level blocks, and this story has all four: ` +
        `${withProse.tabProseBlocks}`,
    )
    eq(
      page.tabProseBlocks,
      0,
      'while a page whose proposal has not been read yet renders no prose at all',
    )
    /*
     * **`scrolls={false}`**, and it is not cosmetic.
     *
     * `MarkdownPreview` reserves 40vh under its last block so the end of a document can be
     * scrolled to the top of the pane — load-bearing for the editor's split view, where
     * `scrollSync` has to map the bottom of a file to *somewhere*. On this page the prose is a
     * section between the Documents list and the checklist, so that reserve is a screenful of
     * blank in the middle of a page, which is how it was reported.
     *
     * Asserted as source rather than off the digest: the padding lives in a CSS module and the
     * class name in the markup is hashed, so what the render can see is the modifier being asked
     * for, not the rule it resolves to.
     */
    ok(
      /<MarkdownPreview[\s\S]{0,400}?scrolls=\{false\}/.test(
        read('../src/sidebar/OpenSpecPanel/SpecTab.tsx'),
      ),
      'the proposal is rendered as a section of the page, not as its own scrollport — without ' +
        'it the preview reserves 40vh of scroll-past-end between the prose and the Steps',
    )
    /*
     * **The row leaves Documents only once the prose replaces it**, and this pair is the whole
     * assertion. Its text is a second read, so between the change arriving and the file arriving
     * there is a render with neither — and dropping the row in that window would take the
     * proposal off the page entirely for a beat.
     */
    /*
     * Documents loses its proposal row and **nothing replaces it**.
     *
     * A file row under the prose shipped first, on the reasoning that removing the proposal from
     * Documents left the file with no opener — and it read as what it was: a link to the document
     * whose whole text was printed directly above it. The prose is the proposal.
     */
    eq(
      withProse.tabDocs,
      ['tasks', 'design'],
      'Documents loses its proposal row when the prose is drawn, and no link takes its place ' +
        'under the text it links to',
    )
    ok(page.tabDocs.includes('proposal'), 'but only once there is something to replace it with')

    /*
     * The design marker.
     *
     * `design.md` is optional in the `spec-driven` schema and propose writes none, so a
     * change that has one is a change somebody decided needed an argument settled before it could
     * be implemented. That is the cheapest signal on the page that it is **not trivial**, and
     * before the marker the only way to learn it was to notice one extra row among the file
     * links.
     *
     * The pair is the point: the same change with the artifact *declared and unwritten* — which
     * is the majority of changes — must draw nothing. A permanent chip reading "no design" would
     * be noise on most changes and would bury the signal on the ones that matter.
     */
    eq(t('tab-design').tabDesign, true, 'a change with a design document says so')
    eq(page.tabDesign, false, 'and one whose design is declared but unwritten does not')

    /*
     * The find bar. It is a *local* Ctrl+F, because `cide_core::keymap` has a test forbidding a
     * default binding for it: CodeMirror means its find bar by that chord and a terminal means
     * its own, and a window-capture gate would take it from both at once.
     */
    eq(page.tabFind, false, 'no bar until it is opened')
    eq(page.tabMarks, 0, 'and nothing marked')

    const finding = t('tab-finding')
    eq(finding.tabFind, true, 'the bar draws when it is open')
    eq(
      finding.tabFindFirst,
      true,
      'and is the first thing in the page, so it can stick to the top of the scroller — it sat ' +
        'below the header, and scrolling to a hit took the count and Enter off screen with it',
    )
    ok(finding.tabMarks > 0, `and the matches are marked: ${finding.tabMarks}`)
    eq(
      finding.tabCurrentMark,
      1,
      'exactly one mark is the current one — a bar that highlighted every hit identically ' +
        'would leave the reader counting rows to find where Enter just went',
    )
    eq(finding.tabFindCount, '2/3', 'and the count reads as a position, one-based')
    /*
     * **The count is the marks.**
     *
     * The host used to walk the model and count matches itself while the view marked them
     * separately — two implementations of "what is on this page", kept in step by hand, with no
     * way to notice a drift because both numbers look plausible. A page showing three hits
     * reported fifteen. The count is read off the rendered marks now, and this is the assertion
     * that the two cannot come apart again.
     */
    eq(
      finding.tabMarks,
      Number((finding.tabFindCount ?? '0/0').split('/')[1]),
      `the bar's total and the marks on the page are the same number: ${finding.tabFindCount} ` +
        `vs ${finding.tabMarks} marks`,
    )

    // A search that found nothing says so. Silence is how a reader concludes the page is broken.
    const nothing = t('tab-finding-nothing')
    eq(nothing.tabMarks, 0, 'nothing matched')
    eq(nothing.tabFindCount, 'no matches', 'and the bar says so rather than going blank')
    eq(nothing.tabCurrentMark, 0, 'with no current hit to point at')

    // The editor replaces the requirement it is editing, rather than appearing beside it.
    const editing = t('tab-editing')
    eq(editing.editFields, 4, 'the editor opens on the requirement')
    eq(editing.tabDeltas.length, 1, 'inside its own card')
    eq(t('tab').editFields, 0, 'and a page at rest has no live fields at all')
  }

  /* ------------------------------------------------------------------ classes really exist */

  {
    const css = read('../src/sidebar/OpenSpecPanel/OpenSpecPanel.module.css')
    const tsx = [
      read('../src/sidebar/OpenSpecPanel/OpenSpecPanel.tsx'),
      read('../src/sidebar/OpenSpecPanel/RequirementEditor.tsx'),
    ].join('\n')
    const used = new Set([...tsx.matchAll(/styles\.([A-Za-z0-9_]+)/g)].map((m) => m[1]))
    for (const name of used) {
      ok(
        new RegExp(`\\.${name}[\\s,{:]`).test(css),
        `styles.${name} is referenced and the stylesheet has no such class — neither tsc nor ` +
          `vite can see this, and the element renders unstyled`,
      )
    }
    ok(used.size >= 10, `the class scan found something (${used.size})`)

    /*
     * The change page's own stylesheet, scanned the same way — and it was **not**, which is how
     * `styles.design` and `styles.prose` could have shipped naming nothing. A missing class is
     * invisible to tsc (a CSS module's type is a string index) and to vite, and the element
     * simply renders unstyled: no error, no warning, no gap on screen big enough to notice.
     */
    {
      const tabCss = read('../src/sidebar/OpenSpecPanel/SpecTab.module.css')
      const tabUsed = new Set(
        [...read('../src/sidebar/OpenSpecPanel/SpecTab.tsx').matchAll(/styles\.([A-Za-z0-9_]+)/g)]
          .map((m) => m[1]),
      )
      for (const name of tabUsed) {
        ok(
          new RegExp(`\\.${name}[\\s,{:]`).test(tabCss),
          `SpecTab's styles.${name} is referenced and its stylesheet has no such class`,
        )
      }
      ok(tabUsed.size >= 20, `the page's class scan found something (${tabUsed.size})`)

      /*
       * **The page has exactly one scroller, and it is `.page`.**
       *
       * The proposal block shipped with `max-height: 60vh; overflow-y: auto`, to stop a long
       * proposal pushing the checklist and the requirement cards off the bottom. Wrong problem,
       * wrong tool: the block is a `<details>`, so a reader who wants the rest of the page
       * collapses it in one click — and a scrollport nested inside a scrolling page makes a wheel
       * gesture land on whichever of the two the pointer happens to be over.
       *
       * It also breaks the page's own machinery, which is why this is a rule and not a taste.
       * The find bar sticks to `.page` and `scrollIntoView` addresses it, so a hit *inside* a
       * nested box scrolls within the box while the page stays where it was — and the run of
       * `data-hit` indices, which is counted off the rendered marks, would be pointing at
       * positions in a viewport nobody is looking at.
       */
      const scrollers = [...tabCss.matchAll(/([^{}]*)\{([^}]*)\}/g)]
        .filter(([, , body]) => /overflow(-y)?\s*:\s*(auto|scroll)/.test(body))
        .map(([, selector]) => selector.trim().split('\n').pop()?.trim() ?? '')
      eq(
        scrollers,
        ['.page'],
        `only .page may scroll — a scrollport nested inside it steals the wheel and puts the ` +
          `find bar's scrollIntoView in the wrong viewport: ${JSON.stringify(scrollers)}`,
      )
    }

    /*
     * The header is the sidebar's header, not one of its own.
     *
     * It shipped as tracked-out uppercase in `--dim` — precisely the style the sidebar was moved
     * *off*, and whose four reasons `TasksPanel.module.css`'s header comment records. Compared
     * against that file rather than against literals here, so the two cannot drift apart later:
     * a panel whose title sets differently from every other one reads as a rendering fault.
     */
    {
      // Since the redesign (2026-09-24) every sidebar header is the UI kit's `PanelHeader`,
      // composed rather than restated — so "the same as every other" is "composes the kit's".
      const tasksCss = read('../src/sidebar/TasksPanel/TasksPanel.module.css')
      const rules = (source, selector) => {
        const at = source.indexOf(`\n${selector} {`)
        return at < 0 ? '' : source.slice(at, source.indexOf('}', at))
      }
      const mine = rules(css, '.head')
      const theirs = rules(tasksCss, '.header')
      ok(mine !== '' && theirs !== '', 'both header rules were found')
      const kitHeader = /composes:\s*panelHeader from '[^']*kit\/components\/Surface\.module\.css'/
      ok(kitHeader.test(mine), "the OpenSpec header composes the kit's PanelHeader")
      ok(kitHeader.test(theirs), '…and so does the Tasks header it used to be compared with')
      // And the content starts on the same left edge as the title, which is the other half of
      // looking like one panel: a header at one indent over rows at another reads as a fault.
      ok(
        /\.section \{[^}]*padding:[^;]*var\(--sp-5\)/.test(css),
        'section rows start on the header’s own left edge',
      )
    }

    /*
     * The panel is sized from the token its splitter writes to.
     *
     * `App.tsx` decides *which* token a drag moves, and this panel is in that ternary's `agents`
     * arm — but a drag only moves what a stylesheet reads. Shipped once without this declaration:
     * the divider dragged and the panel did not follow, because it was taking its width from the
     * flex row instead. `check:sidebar` asserts that *some* stylesheet reads each token, and the
     * two older panels already satisfied it, so nothing caught the omission.
     */
    ok(
      /\.panel\s*\{[^}]*width:\s*var\(--w-sidebar-agents\)/.test(css),
      'the panel reads --w-sidebar-agents, the token its splitter writes — without it the ' +
        'divider drags and nothing moves',
    )
    ok(
      /\.panel\s*\{[^}]*flex:\s*none/.test(css),
      'and does not shrink, or the width the user dragged to silently stops being the width',
    )
  }

  /* ------------------------------------------------------------- nothing hooked is unclassed */

  for (const [name, digest] of Object.entries(stories)) {
    eq(
      digest.unclassed,
      0,
      `${name}: an element carries a hook with no class, or a class list containing the literal ` +
        `token \`undefined\` — which is what a template-string className produces when a lookup misses`,
    )
  }

  /* ------------------------------------------------- the source facts a render cannot show */

  {
    const app = read('../src/App.tsx')
    ok(
      /\{sidebar\.view === 'openspec' &&/.test(app),
      'App.tsx draws the panel from a literal branch — `check:boundary` scrapes that exact shape',
    )
    ok(
      /<PanelBoundary\s+name="OpenSpec"/.test(app),
      'and wraps it, so a throw takes the panel down rather than the window. Matched with a ' +
        'regex and not a substring because the attribute is on its own line whenever the ' +
        'formatter decides the props do not fit on one',
    )
    ok(
      app.includes('specEvents.onChanged'),
      'the spec-changed subscription is in App, not in the host — the rail badge outlives the panel',
    )
    const index = read('../src/sidebar/OpenSpecPanel/index.ts')
    ok(
      !index.includes("from './model'"),
      'index.ts must not re-export model.ts — a barrel entry would put React one import away ' +
        'from a module `check:openspec` compiles standalone with a bare tsc',
    )
  }
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`\n${failed} failure(s)`)
  process.exit(1)
}
console.log(
  'openspec render: ok (22 stories, the absent/unknown pair, the tree, the page, the editor)',
)
