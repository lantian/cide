/**
 * Renders both M18 sidebar panels under node and checks what came out.
 *
 * `pnpm build` proves they compile; `check-agents.mjs` proves the two `model.ts` files decide
 * the right things. Neither can see whether the components *paint* — this repository has
 * shipped a panel that compiled, mounted and drew nothing, twice, which is why
 * `check-git-render.mjs` exists and why this is its third instance. So both panels go through
 * `react-dom/server` for real, over every named state in their `fixture.ts`, and this script
 * asserts on the markup that came out.
 *
 * # What each assertion is pinned to
 *
 * Not one of them is decoration; each names a failure this project has actually had or a claim
 * the panel is uniquely able to make falsely.
 *
 *  - **`disabled` prints the config path, and the enable button.** Turning subagents on writes
 *    a file the user's repository will contain. A toggle that adds a tracked file without
 *    naming it is a surprise commit.
 *  - **`roster-unknown` / `board-unknown` draw nothing.** `agents.roster` and `tasks.board`
 *    reach the store through `pendingCommand` with a `null` fallback, so there is a real
 *    interval in which nobody has looked. Drawing `disabled`'s screen then tells a user whose
 *    project has subagents *on* that they are off, one frame early, under a button that commits
 *    a file; drawing `absent`'s says there is no tracker in a project that has one. So both
 *    stories must render **no button at all** and none of the designed sentences.
 *  - **Open is absent for a subagent that is doing nothing, and present for one that is.** The
 *    user's own rule, and asserted as a pair: `roles-resting` has no Open *element* anywhere,
 *    `role-running` has one inside the row of the role that is running. Elements are counted,
 *    not `disabled` attributes, precisely so a well-meaning "grey it out instead" cannot pass.
 *    `role-queued` is the second way to have none — a run with no session yet — so the absence
 *    is pinned from both of its causes.
 *  - **Every role row carries a Configure, in every story.** The count is compared against the
 *    number of rows rather than asserted positive, because "some rows have one" is the state the
 *    rule exists to make impossible. It is the control offered unconditionally, so the row that
 *    most needs it is the one whose role is broken — `role-unavailable` and `role-undefined`
 *    assert exactly that row.
 *  - **The `empty` screen is one centred button into Settings, and the worked YAML is gone.**
 *    Its absence is greped by name: a rewrite that left the example beside the new button would
 *    satisfy every positive assertion in that block.
 *  - **`role-multi-run` draws both runs and summarises with the more demanding.** A run on
 *    screen nowhere is a `claude` spending the user's quota that they cannot see, open or stop;
 *    a summary that reads `Running` over a run awaiting permission hides the one line where a
 *    human is the bottleneck.
 *  - **`role-undefined` still has a row.** A run whose role file was deleted mid-flight would
 *    otherwise vanish from a panel that is now a list of roles.
 *  - **`unreadable` renders zero writing controls.** An app that "recovers" from an
 *    unparseable tracker by overwriting it has destroyed the user's data to fix its own
 *    display — and the likeliest cause is a half-resolved merge conflict, a file still
 *    containing both sides of everything.
 *  - **`list-with-live-run`'s chip classes differ from `list`'s.** Same board, one run. A chip
 *    that looked the same either way would claim an agent that exited an hour ago is still
 *    working. This is the most important assertion in the file.
 *  - **The stale-turn bar has exactly two buttons**, `Retry turn` and `Leave it`. cide cannot
 *    see the model request, only a process it stopped and continued, so it may neither
 *    re-dispatch on its own (double-billing a turn that survived) nor stay silent.
 *  - **`project-paused-no-live-run` still offers the header's Resume.** A project-scope pause
 *    freezes the project's own console session, which is not a run and has no row; over runs
 *    that have all ended, `dispatching: false` is the only thing on screen that says the user is
 *    frozen at all. An offer condition read off the rows is false exactly there, and a Resume
 *    missing exactly there is a window with no way out of a state cide put it in.
 *  - **The rogue stories do not blank the panel or zero the counter.** That is
 *    `check-problems.mjs`'s regression verbatim: an unrecognised severity used to zero every
 *    counter, so the panel headlined "No problems found" above rows the user could see.
 *  - **`unclassed === 0` everywhere.** A CSS module is typed `Record<string, string>`, so
 *    `styles.typo` type-checks, evaluates to `undefined`, and React drops the attribute in
 *    silence — or, inside a template literal, writes the literal token `undefined` into the
 *    class list. Both are counted, and neither `tsc` nor `vite build` can see either.
 *
 * # Mechanics
 *
 * Two SSR bundles, built into `node_modules/.cache` and **not** the system temp dir: the bundle
 * keeps `react-dom/server` external, so node resolves it relative to the output, and under
 * /tmp there is no `node_modules` above it. `window` and `location` are stubbed because
 * `@tauri-apps/api` touches `window` on import; nothing is faked deeper than that, since a
 * check that fakes a browser proves things about the fake. The exit code is carried out of the
 * `try` rather than taken inside it — `process.exit` skips `finally`, and these build
 * directories live under `node_modules`.
 *
 * Run: `pnpm --dir ui run check:agents-render`
 */
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

/**
 * One source file, **with its comments stripped**, for the block at the foot that reads markup
 * it cannot render.
 *
 * Stripped because the assertions there are as much about absence as about presence, and these
 * files explain themselves at length: `TaskDetail.tsx`'s own header names `role="dialog"` and
 * `aria-modal` while arguing that it inherits them rather than declaring them, so a grep over
 * the raw text would read the explanation as the thing it warns against. `check-agents.mjs` does
 * the same to the Rust it scans, for the same reason.
 */
const src = (rel) =>
  readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8')
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/\/\/[^\n]*/g, '')

mkdirSync('node_modules/.cache', { recursive: true })
const out = mkdtempSync(join('node_modules/.cache', 'cide-agents-render-'))

/** The literal strings the panels must print — and, in the `unknown` stories, must not. */
const CONFIG_PATH = '/home/dev/work/thing/.cide/config.json'
const TASKS_PATH = '/home/dev/work/thing/.cide/tasks.json'
const OFF_SENTENCE = 'Subagents are off for this project.'
const ABSENT_SENTENCE = 'No task tracker in this project.'

/*
 * The Agents header's project-scope controls, as their rendered text.
 *
 * The digest carries a button's **text**, not its attributes, so the assertions below reach the
 * control through the word beside its glyph rather than through `title` — which is the other
 * reason that word is drawn at all (`AgentsPanel.module.css` gives the first). The run rows'
 * own pause and resume are bare glyphs, `'⏸'` and `'▶'`, so a label carrying a word is the
 * header's by construction and the two cannot be confused for one another here.
 */
const INTEGRATE = 'Integrate'
const INTEGRATE_CONFIRM = 'Confirm merge'

/*
 * The empty screen's one control, as its rendered text.
 *
 * A literal rather than an import of `AgentsPanel.tsx`'s constant, matching the pair above and
 * for the same reason: this script asserts on markup, and a check that imported the component's
 * own string would keep passing if the control were renamed to something a user cannot
 * recognise. It says *Settings* because that is where it goes, and the assertion is as much
 * about the word as about the element.
 */
const CONFIGURE_ALL = 'Configure subagents in Settings'

/*
 * The Tasks panel's delete, as its rendered text.
 *
 * Three literals rather than an import, matching the Integrate pair above: this script asserts
 * on markup, and a check that imported the component's own constants would keep passing if both
 * the control and the check were renamed to something a user cannot recognise.
 *
 * The row's unarmed control is a glyph and the card's is a word; **both** confirming halves are
 * the same word, and that is the property the pair of assertions below is about — an unarmed
 * control must not carry the destructive label, and an armed one must.
 */
const DELETE_LABEL = 'Delete task'
const DELETE_CONFIRM = 'Confirm delete'
const RESUME_ALL = '▶ Resume'
const PAUSE_ALL = '⏸ Pause'

let failed = 0
const fail = (what, detail) => {
  console.error(`FAIL ${what}${detail === undefined ? '' : `\n  ${detail}`}`)
  failed++
}
const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) fail(what, `actual:   ${a}\n  expected: ${b}`)
}
const ok = (cond, what) => {
  if (!cond) fail(what)
}

try {
  globalThis.window = globalThis
  globalThis.location = { search: '' }

  const agents = await render('src/sidebar/AgentsPanel/smokeEntry.tsx', 'agents')
  const tasks = await render('src/sidebar/TasksPanel/smokeEntry.tsx', 'tasks')

  const a = (name) => agents[name] ?? {}
  const t = (name) => tasks[name] ?? {}

  // ============================================================== the Agents panel ========

  /*
   * The screen most users see first, and the one whose button writes a file into their
   * repository. The path is printed in full and *before* the button; both halves are asserted
   * because either alone is a different screen.
   */
  {
    const d = a('disabled')
    ok(
      d.buttons?.includes('Enable subagents for this project'),
      'the disabled screen offers the enable button in as many words',
    )
    ok(
      d.text?.includes(CONFIG_PATH),
      'and prints the full path of the file it is about to add to the repository',
    )
    ok(
      d.text?.indexOf(CONFIG_PATH) < d.text?.indexOf('Enable subagents'),
      'with the path *before* the button — a toggle that quietly adds a tracked file is a ' +
        'surprise commit, and the sentence has to be readable before the click',
    )
    eq(d.claim, OFF_SENTENCE, 'the claim is the model’s sentence, not a second copy in the JSX')
    eq(d.opens, 0, 'and there is nothing to open')
    eq(d.roles, [], 'nor any role row: there are no roles until the feature is on')
  }

  /*
   * Nobody has looked. The whole assertion is negative.
   */
  {
    const d = a('roster-unknown')
    eq(d.buttons, [], 'an unread roster offers no control at all — not even a reveal')
    ok(
      !d.text?.includes(CONFIG_PATH),
      'and does not print the config path, which belongs to a claim nothing has checked',
    )
    ok(
      !d.text?.includes(OFF_SENTENCE),
      'nor say subagents are off, one frame before the roster says they are on',
    )
    eq(d.rows, [], 'no rows')
    eq(d.roles, [], 'and no role rows either — a row per subagent, for subagents nobody read')
    eq(d.claim, null, 'and no claim of any kind')
    eq(d.meta, '', 'the header withholds its figure rather than printing a zero nobody counted')
    eq(d.empty, false, 'and it is not the empty screen: "nobody looked" is not "there are none"')
  }

  /*
   * ===== The user's rule, which is what this rewrite was for. ==============================
   *
   * *"if does nothing right now — no way to open, only configure button."*
   *
   * Two stories, because there are two ways for a subagent to be doing nothing and a rule that
   * only handled one of them would be right by accident. Both count Open **as elements**, not
   * as `disabled` attributes, precisely so a well-meaning "grey it out instead" cannot pass.
   */
  {
    const d = a('roles-resting')
    eq(d.opens, 0, 'a roster whose roles have no runs renders NO Open element anywhere')
    eq(
      d.roles?.map((r) => r.opens),
      [0, 0],
      '...and none in either role row, counted per row so a stray Open in Recent could not ' +
        'satisfy the line above',
    )
    eq(
      d.roles?.map((r) => r.runs),
      [0, 0],
      'because neither row is drawing an activity line: there is nothing for an Open to open',
    )
    eq(
      d.roles?.map((r) => r.status),
      ['Not running', 'Not running'],
      'and the row says so in words rather than leaving the status blank',
    )
    ok(
      d.roles?.every((r) => r.glyph.length > 0),
      'with a glyph in the dot column — an empty cell reads as a rendering fault',
    )
    ok(
      d.dispatches > 0,
      'the story really was given handlers, so the zeroes above are the gate holding rather ' +
        'than nothing being wired',
    )
  }

  {
    const d = a('role-finished-only')
    eq(
      d.roles?.map((r) => r.opens),
      [0, 0],
      'a role whose runs have ENDED offers no Open either — the second way of doing nothing, ' +
        'and the one a rule written as "has this role ever run" would get wrong',
    )
    eq(
      d.roles?.map((r) => r.phase),
      ['none', 'none'],
      'both rows are back to resting',
    )
    ok(
      d.opens >= 1,
      'while the finished transcript is STILL reachable — from Recent, which is the only place ' +
        'it is, and the reason that section survived the rewrite',
    )
  }

  {
    const d = a('role-running')
    const dev = d.roles?.find((r) => r.id === 'developer')
    eq(dev?.runs, 1, 'a role with a live run draws a line for it')
    eq(dev?.opens, 1, 'and offers Open — this is the half of the rule that is positive')
    eq(dev?.status, 'Running', 'with what it is doing, in a word')
    eq(dev?.phase, 'running', 'and the phase on the element, so this is not an assertion about wording')
    const idle = d.roles?.find((r) => r.id === 'qa')
    eq(idle?.opens, 0, 'and the role beside it, which is doing nothing, still offers none')
  }

  {
    const d = a('role-idle')
    const dev = d.roles?.find((r) => r.id === 'developer')
    eq(dev?.status, 'Idle', 'a run that handed its turn back reads Idle, never Finished — ' +
      'nothing exited, and the row can still be given another turn')
    eq(
      dev?.opens,
      1,
      'and it offers Open, which is the case canOpen most exists for: a complete transcript in ' +
        'a pane the user can then type the next turn into',
    )
  }

  /*
   * The third state that draws no Open, and it must stay a *different* reason from the two
   * above: the role is doing something, there is a line on screen for it, and there is still no
   * session to mirror.
   */
  {
    const d = a('role-queued')
    const queued = d.roles?.find((r) => r.id === 'qa')
    eq(queued?.runs, 1, 'a queued run IS on screen — the role is doing something')
    eq(queued?.status, 'Queued', 'and says which something')
    eq(
      queued?.opens,
      0,
      'and renders NO Open element: there is no session to attach a pane to, and nothing on ' +
        'the row could change that. Not a disabled button — the control that changes this is ' +
        'Dispatch, on the same row',
    )
    eq(d.opens, 0, 'and none anywhere else in the document either')
    ok(d.dispatches > 0, 'the story really was given handlers')
    ok(
      d.rows?.[0]?.includes('t-15'),
      'the queued line still links its task — a run nothing can account for is the state most ' +
        'worth catching, and it is still worth catching before the run starts',
    )
  }

  /*
   * ===== Configure: on every role row, in every state. =====================================
   *
   * The user's other sentence. It is the one control that is offered unconditionally, so the
   * assertion is over *every* row of *every* story rather than over one screen — including the
   * role whose harness is not installed, and the role the roster does not define at all, which
   * are the two rows where nothing else on them can do anything.
   */
  {
    for (const [name, d] of Object.entries(agents)) {
      for (const role of d.roles ?? []) {
        ok(
          role.buttons.includes('Configure'),
          `${name}/${role.id}: every role row carries Configure — it is the control you reach ` +
            'for when the role itself is what is wrong, so the states it must not be missing ' +
            'in are exactly the broken ones',
        )
      }
      eq(
        d.configures,
        (d.roles ?? []).length,
        `${name}: exactly one Configure per role row, no more — a second would be a second ` +
          'route to the same screen from the same row',
      )
    }
    const un = a('role-unavailable')
    const artist = un.roles?.find((r) => r.id === 'artist')
    ok(
      artist?.buttons.includes('Configure') && !artist?.buttons.includes('Dispatch'),
      'the role whose harness is missing carries Configure and NOT Dispatch: it cannot be run, ' +
        'and it is the one somebody most wants to open the settings for',
    )
    eq(
      artist?.reason,
      'opencode is not installed.',
      'and it keeps its own sentence rather than being dropped from the list — "we could not ' +
        'tell" and "it is not there" must stay distinguishable',
    )
  }

  /*
   * Never both, never neither, per role row.
   */
  {
    const d = a('role-unavailable')
    eq(d.roles?.length, 3, 'three roles, including the one whose harness is missing')
    for (const role of d.roles ?? []) {
      if (role.buttons.includes('Dispatch')) {
        eq(role.reason, '', `${role.id}: a dispatchable role carries the button and no sentence`)
      } else {
        ok(role.reason.length > 0, `${role.id}: a refused role carries the sentence saying why`)
      }
    }
    ok(
      d.roles?.some((r) => r.reason !== '') && d.roles?.some((r) => r.buttons.includes('Dispatch')),
      'and the fixture really does exercise both halves',
    )
  }

  /*
   * ===== The cross-link, which is half a link with either half missing. ====================
   */
  {
    const d = a('role-running')
    const row = d.rows?.find((r) => r.startsWith('running|'))
    ok(row !== undefined, 'the running run is on screen')
    ok(row?.includes('t-14'), 'its line carries the task id — the fact')
    ok(
      row?.includes('Add the retry bar'),
      'and the task title — the decoration that makes it readable; a row with only one of the ' +
        'two is half a cross-link',
    )

    const adrift = a('role-awaiting')
    ok(
      adrift.rows?.some((r) => r.includes('no task')),
      'a run with no task says so rather than leaving a blank line — "unaccounted for" is the ' +
        'state most worth catching, and an empty line reads as a rendering gap',
    )
  }

  /*
   * ===== A role with several runs. =========================================================
   *
   * `effective_max_concurrent` clamps a role to one working run under worktree isolation, which
   * is the default, so this needs `isolation: shared` to happen at all. Which is exactly why it
   * is pinned: the wrong decision here hides a running `claude` behind a summary line.
   */
  {
    const d = a('role-multi-run')
    const qa = d.roles?.find((r) => r.id === 'qa')
    eq(qa?.runs, 2, 'both runs are drawn — a run on screen nowhere is an agent nobody can stop')
    eq(qa?.opens, 2, 'each with its own Open, because Open attaches a pane to one run')
    eq(
      qa?.status,
      'Awaiting permission',
      'and the role summarises itself with the run that is blocked on the USER rather than the ' +
        'one that came first: a summary reading "Running" would hide the one line where a human ' +
        'is the bottleneck',
    )
    eq(d.meta, '2', 'and both hold a slot, which the header figure counts')
  }

  /*
   * ===== A live run of a role the roster no longer defines. ================================
   *
   * The row this restructure could most easily have lost: a list built from `roster.agents`
   * alone drops it, and the agent goes on working and billing with no row anywhere.
   */
  {
    const d = a('role-undefined')
    const ghost = d.roles?.find((r) => r.id === 'ghost')
    ok(ghost !== undefined, 'a run whose role file was deleted still gets a row, invented from it')
    eq(ghost?.runs, 1, 'with its line on screen')
    eq(ghost?.opens, 1, 'openable, because there is a session and a child behind it')
    ok(!ghost?.buttons.includes('Dispatch'), 'and not dispatchable — there is no definition')
    ok(ghost?.reason.includes('.cide/agents/'), 'refused with a sentence naming where roles live')
    ok(ghost?.buttons.includes('Configure'), 'and it carries Configure, like every other row')
  }

  /*
   * The stale-turn bar: a suspicion, with exactly the two answers a suspicion admits.
   */
  {
    const d = a('stale-turn')
    eq(d.staleBars, 1, 'exactly one bar, on the run it is about')
    eq(d.staleActions, ['Retry turn', 'Leave it'], 'the two controls, verbatim and in order')
    eq(
      d.staleBarButtons,
      2,
      'and exactly two buttons inside the bar — a third would be a third answer to a question ' +
        'that has two',
    )
    ok(
      d.text?.includes('may have'),
      'the sentence stays hedged: cide saw a process it froze, never the model request',
    )
  }

  /*
   * The `check-problems.mjs` regression, reached by a new route.
   *
   * A phase this build cannot read is admitted by `isActivePhase` on purpose — the alternative,
   * membership of `ACTIVE_PHASES`, would file it as neither active nor done and it would appear
   * under no role and in no Recent at all.
   */
  {
    const d = a('rogue-phase')
    eq(d.rows?.length, 2, 'a phase nobody recognises does not blank the panel, or drop its row')
    eq(d.roles?.length, 2, 'both roles are still listed')
    eq(d.meta, '1', 'nor zero the header figure, which still counts the run beside it')
    ok(d.glyphs?.includes('?'), 'the unknown phase gets the fallback glyph')
    ok(
      d.roles?.some((r) => r.glyph === '?' && r.status === 'Unknown'),
      'and its role summarises as Unknown rather than as nothing',
    )
    ok(
      !d.glyphs?.some((g) => g === '' || g.includes('function')),
      'and never an empty cell or a stringified `Object.prototype.constructor`, which is what ' +
        'a `?? fallback` over a prototype key hands back',
    )
  }

  {
    const d = a('role-paused')
    ok(d.buttons?.includes('▶'), 'a paused run offers Resume')
    ok(!d.buttons?.includes('⏸'), 'and not Pause as well — the row draws one or the other')
    eq(d.meta, '1', 'a paused run still holds its slot, so it still counts as live')
    ok(
      d.buttons?.includes(RESUME_ALL),
      'and the header offers the project-scope Resume beside it: one frozen run is something ' +
        'frozen, whatever the queue is doing',
    )
  }

  /*
   * ===== The project-scope pause, in the header. ==========================================
   *
   * The control that freezes the user's **own console session** along with the agents. Which
   * makes its resume half unlike every other control in this panel: it is the one a user
   * reaches for from a window that has stopped answering the keyboard, so the condition it is
   * offered on has to be true in every state where anything is frozen — and there is a state
   * where nothing on screen says so.
   */
  {
    const d = a('project-paused')
    ok(
      d.buttons?.includes(RESUME_ALL),
      'a project-scope pause puts Resume in the header, where it is reachable from a window ' +
        'whose console pane is frozen',
    )
    ok(
      d.buttons?.some((b) => /resume/i.test(b)),
      'and it says "resume" in a word rather than only in a glyph — this is the control a user ' +
        'has to *find*, and a bare ▶ in a header is something you find by hovering',
    )
    ok(
      !d.buttons?.includes(PAUSE_ALL),
      'never both at once: a header offering Pause and Resume side by side asks a user whose ' +
        'console is already frozen to work out which half applies to them',
    )
    eq(d.dispatches, 0, 'and a shut queue refuses every dispatch, with canDispatch’s sentence')
    ok(
      d.roles?.every((r) => r.reason === 'The dispatch queue is paused.'),
      '...on every role row, in as many words',
    )
    eq(
      d.meta,
      '2',
      'the figure still counts both frozen runs — they hold their slots and their worktrees. ' +
        'It is a slot count, not an activity count; the Resume beside it is what stops the ' +
        'header reading as "2 agents working"',
    )
  }

  /*
   * **The assertion this control exists for.**
   *
   * Same project-scope pause, over runs that have all ended: nothing reads `paused`, nothing
   * reads `running`, every role row is resting, the header figure is `0`, and every run is in a
   * `<details>` that is closed. The console session is frozen all the same — it is not a run and
   * has never had a row — and `dispatching: false` is the only fact on screen that says so.
   *
   * Any offer condition that reads the rows, the live count, or "is this roster busy" is false
   * here, and a Resume that is missing here is a window with no way out of a state cide put it
   * in. `crates/cide-app/src/agents.rs` names the case in as many words: "the header has to be
   * able to draw Resume for a project whose runs are all finished".
   */
  {
    const d = a('project-paused-no-live-run')
    ok(
      d.buttons?.includes(RESUME_ALL),
      'Resume is offered with NO live run, NO paused row and a header figure of zero — the ' +
        'queue being shut is the whole of the evidence, and it is enough',
    )
    eq(d.meta, '0', 'the figure reads zero: nothing holds a slot')
    eq(
      d.roles?.map((r) => r.phase),
      ['none', 'none'],
      'and every role row is resting, which is exactly why the flag has to be read',
    )
    eq(d.dispatches, 0, 'the queue is shut, so no role offers a dispatch either')
  }

  /*
   * Both halves meaningful at once, and the tie broken the same way every time.
   *
   * `stale-turn` is one run frozen by its own row control while another runs and the queue is
   * open: there is something to freeze *and* something to thaw. Resume takes the slot, because
   * the two mistakes do not cost the same — a missing Pause is a gesture the user makes from a
   * row or the palette, and a missing Resume can be a window that has stopped responding.
   */
  {
    const d = a('stale-turn')
    ok(d.buttons?.includes(RESUME_ALL), 'with both meaningful, Resume wins the header slot')
    ok(!d.buttons?.includes(PAUSE_ALL), 'and Pause is not drawn beside it')
    eq(
      d.buttons?.filter((b) => b === '⏸').length,
      1,
      'the awaiting run keeps its own per-run pause, which is the scope that was never in ' +
        'question — the header withheld the *project* one, not every way to freeze anything',
    )
  }

  /*
   * Nothing frozen: no Resume anywhere, and the header offers the other half instead.
   */
  {
    const d = a('role-running')
    ok(
      !d.buttons?.includes(RESUME_ALL),
      'with nothing frozen there is nothing to thaw, and a Resume on that screen would be a ' +
        'button whose press is a no-op the backend has to refuse',
    )
    ok(
      d.buttons?.includes(PAUSE_ALL),
      'the header offers Pause instead: a live run and an open queue is something to freeze',
    )
  }

  {
    const d = a('roles-resting')
    eq(
      d.buttons?.filter((b) => b === PAUSE_ALL || b === RESUME_ALL),
      [],
      'a checked, idle roster gets neither half. Pause would still do one thing there — freeze ' +
        'the user’s own console — and a control whose only effect is its documented side ' +
        'effect, on a screen with no agents working, is not a control this panel should draw. ' +
        'The `agents.pause` palette row still carries it',
    )
    eq(d.rows, [], 'nothing running')
    eq(d.dispatches, 2, 'and both roles offer a dispatch')
    eq(d.meta, '0', 'a checked, idle roster may say zero — it looked')
  }

  /*
   * ===== The empty screen, which is the other half of what the user asked for. =============
   *
   * *"if no agent — we should show button at the center that will lead us to Settings to
   * configure all possible agents."*
   *
   * What used to be here was a static worked example of a role file — the panel answering "you
   * have no subagents" with a page of YAML to copy. `name: developer` was its first line, and
   * the grep for it is what stops it coming back.
   */
  {
    const d = a('empty')
    eq(d.empty, true, 'the empty roster draws its centred block')
    eq(
      d.emptyButtons,
      ['Configure subagents in Settings', 'Reveal .cide/'],
      'containing the button into Settings, and — second and quiet — the reveal, which is still ' +
        'the only route from this panel to the committed files themselves',
    )
    eq(
      d.buttons,
      ['Configure subagents in Settings', 'Reveal .cide/'],
      '...and nothing else anywhere on the screen. There is deliberately still no "create three ' +
        'roles for me" button: inventing a `qa` role nobody asked for is the same class of lie ' +
        'as a confident empty list, and Settings is where the user writes their own',
    )
    ok(
      !d.text?.includes('name: developer'),
      'the worked role file is GONE — a panel that answers "you have no subagents" with YAML to ' +
        'copy is the thing the user objected to, and this grep is what keeps it out',
    )
    ok(!d.text?.includes('---'), 'front matter and all')
    ok(
      d.claim !== null && d.claim !== OFF_SENTENCE,
      'the claim still says which empty this is: "on, and none defined" is not "off", which is ' +
        'the whole reason AgentRoster has two arms rather than one',
    )
    eq(d.roles, [], 'and there are no role rows to carry a Configure of their own')
  }

  {
    const d = a('no-project')
    eq(d.buttons, [], 'with no project open there is nothing to press')
    eq(d.meta, '', 'and nothing to count, whatever roster the host happened to be holding')
    ok(d.text?.includes('No project open'), 'the panel says which of its empties this is')
  }

  /*
   * ===== Integrate, which is the one gesture here that changes the user's OWN branch. ======
   *
   * A single press must never merge. The pair of assertions is the whole claim: the unarmed
   * control does not carry the confirming label, and the armed one does. Checking only the
   * second would pass for a control that merged on first click and merely relabelled itself.
   */
  {
    const un = a('integrate-unarmed')
    ok(
      un.buttons?.includes(INTEGRATE),
      'every role offers Integrate — a role branch outlives its runs, and the panel cannot know ' +
        'what is on one without asking',
    )
    ok(
      !un.buttons?.includes(INTEGRATE_CONFIRM),
      'and an unarmed role does NOT carry the confirming label: one press arms, it does not merge',
    )
    ok(
      un.roles?.every((r) => r.buttons.includes(INTEGRATE)),
      '...on every row, including the ones that are doing nothing at all',
    )

    const armed = a('integrate-armed')
    ok(
      armed.buttons?.includes(INTEGRATE_CONFIRM),
      'an armed role asks to confirm, in a word — this is the press that rewrites the branch the ' +
        'user has checked out, and a bare glyph is not consent',
    )
    eq(
      armed.roles?.filter((r) => r.buttons.includes(INTEGRATE_CONFIRM)).length,
      1,
      'exactly one row is armed: arming is single-valued, so a second press cannot land on a ' +
        'role the user never named',
    )
    eq(armed.unclassed, 0, 'the armed control names only classes its stylesheet defines')
  }

  // =============================================================== the Tasks panel ========

  /*
   * The strictest rule in the panel.
   */
  {
    const d = t('unreadable')
    eq(
      d.writeControls,
      0,
      'an unreadable tracker offers NOTHING that writes — no New task, no reset, no repair; ' +
        'the file is most likely a half-resolved merge conflict and overwriting it destroys ' +
        'the user’s data to fix cide’s display',
    )
    eq(d.buttons, ['Reveal file', 'Retry'], 'exactly two controls, and both are read-only')
    ok(d.text?.includes('conflict markers'), 'the parser’s own error is shown rather than hidden')
    ok(d.text?.includes(TASKS_PATH), 'and the file is named, so a human can go and fix it')
  }

  {
    const d = t('board-unknown')
    eq(d.buttons, [], 'an unread board offers no control at all')
    eq(d.writeControls, 0, 'and certainly nothing that writes')
    ok(
      !d.text?.includes(ABSENT_SENTENCE),
      'it does not claim there is no tracker one frame before the tracker arrives',
    )
    ok(!d.text?.includes(TASKS_PATH), 'nor name a file nothing has read')
    eq(d.meta, '', 'and withholds the header figure rather than printing a zero')
    eq(d.rows, [], 'no rows')
  }

  {
    const d = t('absent')
    ok(d.buttons?.includes('New task'), 'the absent screen offers one creating control')
    eq(d.writeControls, 1, 'exactly one, and it is the entry to the compose dialog rather than ' +
      'a write — creating the first task is what creates the file, so this screen and a ' +
      'populated one reach it the same way')
    ok(d.text?.includes(TASKS_PATH), 'with the path printed in full, before the button')
    ok(
      d.text?.indexOf(TASKS_PATH) < d.text?.indexOf('New task'),
      'because a control that quietly adds a tracked file to a repository is a surprise commit',
    )
  }

  /*
   * The single most important assertion in this file.
   */
  {
    const idle = t('list')
    const live = t('list-with-live-run')
    ok(idle.chipClasses?.length > 0, 'the idle list draws chips at all')
    ok(live.chipClasses?.length > 0, 'and so does the live one')
    ok(
      JSON.stringify([...new Set(idle.chipClasses)].sort()) !==
        JSON.stringify([...new Set(live.chipClasses)].sort()),
      'a live run’s chip and an assigned-but-idle chip use DIFFERENT classes — the same board, ' +
        'one run apart. A chip that looked the same either way would say an agent that exited ' +
        'an hour ago is still working',
    )
    eq(idle.chipLit, ['false', 'false', 'false'], 'no run, so nothing is lit')
    eq(live.chipLit, ['true', 'false', 'false'], 'one run, so exactly one chip is')
    eq(
      idle.chipLabels,
      live.chipLabels,
      'and the labels are identical, so the difference above is the *rendering* rather than the ' +
        'text — which is the whole claim',
    )
    eq(idle.rows?.length, 4, 'every task reaches the DOM')
    eq(
      idle.groups,
      ['Doing|1', 'Review|1', 'Todo|1', 'Done|1'],
      'grouped in GROUP_ORDER — active first, because the panel answers "what is happening"',
    )
    eq(idle.meta, '3/4', 'the header says how much is left and how big the tracker is')
  }

  /*
   * ===== The card: a modal, read-only until one field is put into edit. ===================
   *
   * The stories are `TaskDetail` — `TaskDetailModal` minus the `OverlayCard` wrapper, which is a
   * portal and which `react-dom/server` refuses outright. What the wrapper adds is asserted from
   * source at the foot of this file; everything a user can see and press inside the dialog is
   * here, rendered for real.
   *
   * The rule these assertions exist for is the one the user asked for in as many words: *if I'm
   * viewing the issue, all fields should be read-only*. The old card was a form — a title
   * `<input>` that committed on blur, a `<textarea>`, a `<select>` — so reading a task and
   * rewriting it were the same screen, and a stray keystroke was a write to a file the whole
   * team commits. `check-agents.mjs` can prove `beginEdit` opens one field at a time and still
   * not see a card that draws three live boxes regardless; only markup settles that.
   */
  {
    const rest = t('card')
    eq(rest.detail, true, 'the card renders at all')
    eq(
      rest.fields,
      ['title|rest', 'status|live', 'assignee|rest', 'body|rest'],
      'four fields, and three of them are AT REST: their values as text. `status` is the one ' +
        'that keeps its control, because a segment cannot be changed by a gesture that was not ' +
        'aimed at one of its four named buttons — and moving a task along is what a board is for',
    )
    eq(
      rest.fieldControls,
      0,
      'and at rest there is NOT ONE `<input>`, `<textarea>` or `<select>` in any field. Counted ' +
        'inside the field rows, so the comment composer — which is a log, not a field, and ' +
        'deliberately has no affordance — cannot make a read-only card look editable',
    )
    eq(
      rest.fieldEdits,
      3,
      'one edit affordance per editable field: title, assignee, body. Compared against the ' +
        'number rather than asserted positive, because "some fields have one" is the state that ' +
        'leaves a field readable and unchangeable',
    )
    eq(rest.fieldSaves, 0, 'nothing is in edit, so there is nothing to save')
    eq(
      rest.fieldValues,
      [
        'title|Add the retry bar',
        'assignee|Developer',
        'body|A frozen run may have lost its turn. Offer a retry rather than re-dispatching.',
      ],
      'and each rest row draws the value itself — the assignee as the role’s LABEL rather than ' +
        'its id, which is the only place on the card that difference is visible',
    )
    ok(rest.close, 'the card carries an explicit close control — a modal with none is a trap, ' +
      'and neither the scrim nor Escape is discoverable')
    /*
     * Who asked for this task. (M21)
     *
     * `card` is the user's and `card-bare` is the orchestrator's, and the pair is the assertion:
     * a card that string-matched a name, or that quietly drew the *assignee*, would pass one of
     * these and fail the other. The story below is unassigned on purpose, so a creator line
     * reading `Task::agent` prints nothing there.
     *
     * Reported as *missing* — a tracker whose rows are written by six subagents and a person, in
     * which nothing on screen says which of them wanted the row you are reading.
     */
    eq(
      rest.creator,
      'user|Created by You',
      'the card names its creator, and the user’s own tasks say You',
    )
    eq(
      rest.comments,
      [
        'user|The bar should offer a retry, not re-dispatch on its own.',
        'orchestrator|Assigned to developer. Two lines, and the break matters.',
        'agent|Retried once; the second attempt got through.',
      ],
      'the log is oldest first — a log out of time order reads as a different conversation, and ' +
        'this file is committed, so a merge can interleave two agents’ lines',
    )
    /*
     * The per-comment controls, and why they are counted rather than looked for. (M21)
     *
     * Both are optional props, so the card draws each only when its handler is passed — and
     * until this was written no story passed either, which meant the gate rendered a log with
     * no controls on it while the running app rendered two on every line. Reported as *"not
     * working Delete button"*, and a check that never drew the button could not have said
     * anything either way.
     *
     * The row count is the half that pins the layout. Three comments, three action rows, one
     * Edit and one Delete each: putting them back into the head beside the timestamp — where
     * they were, and where they were a pair of 13px-tall targets at the end of a line being
     * read rather than aimed at — takes `commentActionRows` to zero while leaving both button
     * counts at three.
     */
    eq(rest.commentEdits, 3, 'one Edit per comment, and only when a handler was passed')
    eq(rest.commentDeletes, 3, 'one Delete per comment — the control this whole batch is about')
    eq(
      rest.commentActionRows,
      3,
      'each pair on its OWN row under its comment. The head is provenance — who, when, edited ' +
        '— and it reads as one right-aligned fact; two buttons spliced into it took the ' +
        'rightmost position away from the timestamp, which is what the user asked to undo',
    )

    eq(rest.runStrip, false, 'with nothing running there is no live-run strip to draw')
    ok(
      rest.buttons?.includes('Add comment'),
      'the composer is still there and still append-only: the one thing on the card that is ' +
        'written by adding rather than by changing',
    )

    /*
     * The same card, one prop apart. This pair is the whole claim.
     */
    const editing = t('card-editing-title')
    eq(
      editing.fields,
      ['title|edit', 'status|live', 'assignee|rest', 'body|rest'],
      'ONE field in edit, and the other two still at rest. A card that put every field into edit ' +
        'together is the form this change removed',
    )
    eq(editing.fieldControls, 1, 'and exactly one control on screen, in that field')
    eq(
      editing.fieldValues.map((v) => v.split('|')[0]),
      ['assignee', 'body'],
      'the field in edit draws its editor INSTEAD of its rest text, not beside it',
    )
    eq(editing.fieldSaves, 1, 'with a Save, because a text field cannot commit itself')
    eq(
      editing.fieldEdits,
      2,
      'and the field in edit no longer offers a pencil — the same element is its Cancel, so ' +
        'React keeps the DOM node and the keyboard does not lose its place',
    )
    ok(editing.buttons?.includes('Cancel'), 'which says Cancel in a word')
    eq(
      rest.fields.length,
      editing.fields.length,
      'the two stories draw the same four rows: the mode is the only thing that differs, which ' +
        'is what makes every line above a statement about the posture rather than about two ' +
        'unrelated fixtures',
    )

    const assignee = t('card-editing-assignee')
    eq(assignee.fields, ['title|rest', 'status|live', 'assignee|edit', 'body|rest'], 'each ' +
      'editable field is reachable, and opening one closes none of the others because only one ' +
      'was ever open')
    eq(assignee.fieldControls, 1, 'one control again')
    eq(
      assignee.fieldSaves,
      0,
      'and NO Save beside the select: choosing is the commit. A `<select>` already costs a click ' +
        'to open and a click to choose; a third press to confirm the choice just made would be a ' +
        'button with nothing left to do',
    )
    const body = t('card-editing-body')
    eq(body.fields, ['title|rest', 'status|live', 'assignee|rest', 'body|edit'], 'and the body')
    eq(body.fieldSaves, 1, 'which does draw a Save — Enter in a body is a newline, not a commit')

    /*
     * The status segment, in every card story. It is the field with no affordance, so its
     * control has to be present at rest as well — a "consistency" pass that put it behind a
     * pencil would leave the task's own state hidden behind a click.
     */
    for (const [name, d] of [['card', rest], ['card-editing-title', editing]]) {
      ok(
        d.text?.includes('Doing') && d.text?.includes('Review'),
        `${name}: the status segment is drawn whatever else the card is doing`,
      )
      ok(
        !d.fields?.includes('status|rest') && !d.fields?.includes('status|edit'),
        `${name}: and status is never a rest/edit field — it has no pencil to be behind`,
      )
    }

    /*
     * Every field empty. `restText` is total and never returns an empty string, so all three
     * rows still draw — a card that collapsed them would look like one that failed to render.
     */
    const bare = t('card-bare')
    eq(
      bare.fields,
      ['title|rest', 'status|live', 'assignee|rest', 'body|rest'],
      'a task with no title, no body and nobody assigned still draws four rows',
    )
    eq(bare.fieldControls, 0, 'still read-only')
    eq(
      bare.fieldValues,
      ['title|Untitled', 'assignee|Unassigned', 'body|No description.'],
      'each with a placeholder rather than an empty row, so "this field is empty" and "this row ' +
        'failed to draw" cannot look the same',
    )
    ok(
      bare.fieldValues?.every((v) => (v.split('|')[1] ?? '') !== ''),
      '...and not one of the three is the empty string',
    )
    eq(
      bare.creator,
      'orchestrator|Created by Orchestrator',
      'and a task nobody is assigned to still names who asked for it — the creator is not the ' +
        'assignee, and this is the half of that pair where the two differ',
    )

    const live = t('card-with-live-run')
    eq(live.runStrip, true, 'a live run against this task gets its strip')
    ok(live.buttons?.includes('Open'), 'and the strip can attach a pane to it')
  }

  /*
   * ===== The compose dialog: a task that does not exist yet. ==============================
   *
   * Reported as *"i'm expecting that all fields are editable and task isn't created while not
   * press Create"*. *New task* used to create the row on the click and open its card, so a
   * mis-click put a titled-`New task` row into a file the whole team commits — one that had to
   * be deleted rather than abandoned, and that a dispatched agent could read in between.
   *
   * These assertions are the **inverse** of the card's two blocks above, and deliberately so:
   * the card must draw no control at rest, and this must draw every one of them at once. There
   * is nothing to read here and nobody else writing, so a pencil per field would be four
   * ceremonies to write one task.
   */
  {
    const empty = t('compose-empty')
    const filled = t('compose-filled')

    eq(empty.compose, true, 'the dialog renders at all')
    eq(
      empty.composeControls,
      3,
      'and draws a live control in every field at once — the title box, the assignee select and ' +
        'the body box. Three and not four because the status field is a segment of buttons, ' +
        'asserted on its own below; the card’s `fieldControls` counts the same three and must ' +
        'be ZERO at rest, which is the pair these two numbers make',
    )
    eq(empty.composeControls, filled.composeControls, 'the same four whether or not it is filled in')
    eq(
      empty.composeStatuses,
      ['todo|true', 'doing|false', 'review|false', 'done|false'],
      'the status segment offers all four and starts on `todo` — `EMPTY_DRAFT`’s value, and not ' +
        'the filter the list happens to be on: a status the user did not choose is one they will ' +
        'not notice, in a file their repository tracks',
    )
    eq(
      filled.composeStatuses,
      ['todo|false', 'doing|true', 'review|false', 'done|false'],
      'and a draft that named one is drawn on it. This is the field `TaskNew::status` was added ' +
        'for, so a user starting work now says so in one write instead of two',
    )

    /*
     * The pair the whole report turns on. Counted as `on`/`off` rather than by looking for the
     * button, so "Create vanished" and "Create is waiting" cannot digest the same — a Create
     * that materialised mid-word would pass a mere presence check on the filled story alone.
     */
    eq(
      empty.composeCreate,
      'off',
      'with no title, Create is drawn and REFUSES: `draftReady` is the gate, because ' +
        '`cide_tasks::validate` refuses a task with no title and a failure notice is a worse ' +
        'way to learn that than a button that is visibly waiting',
    )
    eq(filled.composeCreate, 'on', '...and goes live once there is one')
    eq(empty.composeCancel, true, 'Cancel is there in both')
    eq(filled.composeCancel, true, '...and in the filled one')
    ok(
      empty.buttons?.includes('Create') && empty.buttons?.includes('Cancel'),
      'both are named in words rather than glyphs — this is the one dialog in the panel whose ' +
        'two ways out do opposite things',
    )

    /*
     * The write in flight. The dialog stays up: closing on the click would throw the user's
     * paragraph away the first time a create failed, and the draft is in no file to recover it
     * from — which is precisely the property that makes "nothing is written until Create" true.
     */
    const busy = t('compose-busy')
    eq(busy.compose, true, 'a create in flight leaves the dialog on screen')
    eq(busy.composeControls, 3, 'still holding every field the user typed into')
    eq(busy.composeCreate, 'off', 'with Create inert, so a second press cannot write twice')
    eq(busy.composeCancel, true, 'and a way out that still works')

    for (const [name, d] of [['compose-empty', empty], ['compose-filled', filled], ['compose-busy', busy]]) {
      eq(d.unclassed, 0, `${name}: names only classes its stylesheet defines`)
    }

    /*
     * And the card, which shares the stylesheet, still has none of this. A digest is per-story,
     * so this is really a statement about the two components being separate: a compose form
     * spliced into `TaskDetail` would put four live boxes over a record several agents write to.
     */
    eq(t('card').compose, false, 'the card is not the compose dialog')
    eq(t('card').composeControls, 0, 'and draws none of its controls')
  }

  /*
   * The `check-problems.mjs` regression, on the tasks side.
   */
  {
    const d = t('rogue-status')
    eq(d.rows?.length, 4, 'a status nobody recognises does not drop the task from the tracker')
    eq(d.meta, '4', 'nor zero the header count')
    ok(
      d.rows?.some((r) => r.includes('|?|t-18|')),
      'the unknown status gets the fallback glyph rather than an empty marker cell',
    )
    ok(
      d.groups?.some((g) => g.startsWith('Todo|2')),
      'and lands in Todo — visible and actionable, in the group that claims the least. Not ' +
        'Doing, which would assert work is under way on the strength of a value nothing read',
    )
  }

  {
    const d = t('empty')
    eq(d.claim, 'No tasks yet.', 'a read, empty tracker may say so — it looked')
    eq(d.buttons, ['New task'], 'and offers the one thing to do about it')
    eq(d.meta, '0', 'zero is reportable once something actually read the file')
  }

  /*
   * ===== The delete, which was "built and reachable from nothing" case #22. ===============
   *
   * `task_delete` was a registered command with a wrapper in `client.ts` and a working
   * `tasksStore.remove`, and **no component rendered anything that called it**. Every gate in
   * the repository was satisfied: `contract-check` compares `generate_handler!` against
   * `contract/*.json` and saw no drift, `check:commands` walks the palette registry, which this
   * has no row in, and `check:agents` proves the command is spelled in the client — which it
   * was. The one thing nothing asked was whether a *user* could reach it, and that question can
   * only be answered against markup.
   *
   * So the controls are counted as **elements**, in both the places a user would look for one,
   * and the arming is counted separately from the confirming half. Checking only that the word
   * "Delete" appears somewhere would pass for a control that deleted on first click and merely
   * relabelled itself afterwards.
   */
  {
    const un = t('list')
    ok(
      un.deletes >= 4,
      'every row in the list carries a delete control — the sidebar is where the whole board ' +
        'is visible, and a task created by mistake wants deleting without a detour through it',
    )
    eq(
      un.deleteConfirms,
      0,
      'and not one of them is the confirming half: the first press arms, it never deletes. A ' +
        'row-level delete in a list is a mis-click waiting to happen',
    )
    ok(
      !un.buttons?.includes(DELETE_CONFIRM),
      'the destructive label appears nowhere on an unarmed list',
    )

    const armed = t('list-delete-armed')
    eq(armed.deleteConfirms, 1, 'arming one row draws exactly one confirming control')
    ok(
      armed.buttons?.includes(DELETE_CONFIRM),
      'and it says so in a word — the row’s unarmed control is a bare × for width, so the word ' +
        'appearing is exactly the transition from harmless to destructive',
    )
    eq(armed.deletes, 3, 'the other three rows keep their unarmed control and are unaffected')
    eq(armed.rows?.length, 4, 'the board is otherwise the same four rows')

    /*
     * **The assertion the rev exists for.** The same arming, one rev on. `armedDelete` refuses
     * it, so the confirming press is not on screen at all — a second click cannot land on a
     * board the user never agreed to. The host clears the state as well, but React runs effects
     * after paint, so the pure gate is what makes the intervening frame safe.
     */
    const stale = t('list-delete-stale')
    eq(
      stale.deleteConfirms,
      0,
      'a board that moved under the arming disarms it — one rev is enough, and this file has ' +
        'several writers',
    )
    eq(stale.deletes, 4, 'and every row is back to offering only the harmless half')
    eq(
      stale.rows,
      armed.rows,
      'the two stories are the same four rows: the rev is the only thing that differs, which ' +
        'is what makes the line above a statement about the arming',
    )

    const card = t('card')
    ok(
      card.buttons?.includes(DELETE_LABEL),
      'the card offers a delete too, and that is not a duplicate — this is where you delete the ' +
        'task you are already reading, without walking back to find its row',
    )
    ok(!card.buttons?.includes(DELETE_CONFIRM), 'unarmed, and not carrying the confirming label')

    const cardArmed = t('card-delete-armed')
    ok(cardArmed.buttons?.includes(DELETE_CONFIRM), 'armed, it asks to confirm')
    eq(
      cardArmed.deleteConfirms,
      1,
      'once, in the same words the row uses, and still as an ARMING rather than as a second ' +
        'dialog. A confirmation over a modal would take the board off screen behind two scrims ' +
        'to ask about an act `git log -p .cide/tasks.json` already has a copy of',
    )
    eq(cardArmed.unclassed, 0, 'and the armed control names only classes its stylesheet defines')

    const broken = t('unreadable')
    eq(
      broken.deletes + broken.deleteConfirms,
      0,
      'an unreadable tracker offers neither half. A delete is a write, `canWrite` is the gate, ' +
        'and the `writeControls` assertion above would already have caught it — which is the point',
    )
  }

  /*
   * ===== The status filter. ================================================================
   */
  {
    const all = t('list')
    eq(
      all.filters,
      ['All|true', 'Doing|false', 'Review|false', 'Todo|false', 'Done|false'],
      'the filter is a row of toggles — All, then the four statuses in GROUP_ORDER so the ' +
        'buttons sit in the order of the headings beneath them — with All on when nothing is ' +
        'filtered, because "all" is the absence of a filter rather than a fifth status',
    )

    const doing = t('list-filtered')
    eq(doing.rows?.length, 1, 'a filter narrows the list')
    ok(doing.rows?.[0]?.includes('t-14'), 'to the tasks in the group that was chosen')
    for (const id of ['t-15', 't-16', 't-17']) {
      ok(
        !doing.text?.includes(id),
        `${id} is ABSENT from the markup rather than merely dimmed — a filter that left the ` +
          'rows on screen would be a highlight, which is a different feature',
      )
    }
    eq(doing.groups, ['Doing|1'], 'and the headings go with the tasks: no Review over nothing')
    eq(
      doing.meta,
      all.meta,
      'the header figure does NOT follow the filter. It names the tracker, not the slice being ' +
        'read; under `done` a following figure would print 0/1 over a board with three open ' +
        'tasks, which is the "my tasks are gone" claim the filter’s own empty screen exists to ' +
        'prevent, moved into the one line a user trusts as a fact about the file',
    )
    eq(doing.meta, '3/4', '...and it is still open-over-total for the whole board')
    eq(
      doing.filters?.filter((f) => f.endsWith('|true')),
      ['Doing|true'],
      'exactly one toggle is lit, and it is the one that is on',
    )
    eq(doing.noMatch, false, 'with matches on screen there is nothing to explain')

    const rogue = t('list-filtered-rogue')
    ok(
      rogue.rows?.some((r) => r.includes('t-18')),
      'a status this build cannot read is matched by the filter it is DRAWN in. Filtering on ' +
        'the raw value would hide a task the unfiltered list had just shown under Todo — the ' +
        'tracker losing a row, arrived at from the filter’s side',
    )

    const none = t('list-filter-no-match')
    eq(none.rows, [], 'a filter can match nothing at all')
    eq(none.noMatch, true, 'and that is its own screen rather than a blank body')
    ok(
      none.claim !== null && none.claim !== t('empty').claim,
      'whose sentence DIFFERS from the empty tracker’s. "No tasks yet." over three real tasks ' +
        'is how a user concludes their board is gone, and this project has a standing rule ' +
        'against a confident empty list',
    )
    ok(/Done/.test(none.claim ?? ''), 'it names the filter that is hiding them')
    ok(none.text?.includes('3 tasks'), 'and counts the tasks that do exist, so the number argues')
    ok(none.buttons?.includes('Show all tasks'), 'with a way back out in the sentence’s own row')
    eq(
      none.filters?.length,
      5,
      'and the filter row itself is still drawn — a filter that hid its own control when it ' +
        'matched nothing would leave the user no way back to their tasks',
    )

    eq(t('empty').filters, [], 'an empty tracker draws no filter row: there is nothing to narrow')
    eq(t('absent').filters, [], 'nor does a project with no tracker file')
    eq(t('unreadable').filters, [], 'nor an unreadable one, which offers only Reveal and Retry')
    /*
     * **The list does not go away when a task is opened.** The card is a modal mounted beside
     * this view, not in place of it, so the board is still on screen behind the scrim: four
     * rows, five filter toggles, the same header figure. The old card replaced the whole body,
     * which meant opening a task took the filter, the groups and every other row with it.
     */
    const selected = t('list-selected')
    eq(selected.rows, all.rows, 'the list behind an open card is the same list, row for row')
    eq(selected.filters, all.filters, '...with its filter row still drawn')
    eq(selected.meta, all.meta, '...and the same header figure')
    eq(
      selected.openRows,
      ['t-14'],
      'and the row the card came from is marked. Exactly one: a modal over four rows of the ' +
        'same shape that does not say which one it belongs to makes the user close it to find out',
    )
    eq(all.openRows, [], 'with no card open, no row is marked — the mark is the card’s, not a ' +
      'second selection the list keeps on its own')
    eq(
      selected.detail,
      false,
      'and the card itself is NOT in this view’s markup: it is mounted by `TasksPanelHost` ' +
        'beside it, which is what keeps the portal out of this render',
    )
  }

  {
    const d = t('no-project')
    eq(d.buttons, [], 'with no project open there is nothing to press')
    eq(d.meta, '', 'and nothing to count')
  }

  /*
   * ===== The modal wrapper, read as SOURCE — and why it cannot be rendered. ================
   *
   * `TaskDetailModal` is `OverlayCard` around `TaskDetail`, and `OverlayCard` **portals to
   * `document.body`**. `react-dom/server` refuses a portal outright — *"Portals are not
   * currently supported by the server renderer. Render them conditionally so that they only
   * appear on the client render."* — so there is no story above that can contain one, and
   * stubbing `document` does not help: the refusal is the renderer's, not the DOM's.
   *
   * That is not a hole so much as a boundary, and the components are split along it: everything
   * a user can see or press inside the dialog lives in `TaskDetail` and is rendered for real
   * above. What is left is one wrapper, and these greps are what keep it honest. They are weaker
   * than a render and are written to be worth having anyway — each one names a *specific*
   * regression that would otherwise pass every other gate in the repository:
   *
   *  - the card quietly reverting to an in-place panel, which is the thing the user asked to
   *    change;
   *  - `TaskDetail` growing its own `role="dialog"` and its own scrim — a fifth definition of
   *    what an overlay looks like in this app, which is how two of them end up 4px apart;
   *  - `OverlayCard` losing the dialog semantics this card inherits instead of declaring.
   *
   * The last one reads a file this panel does not own, deliberately: an inherited guarantee that
   * nobody checks is a guarantee that leaves silently.
   */
  {
    const detail = src('../src/sidebar/TasksPanel/TaskDetail.tsx')
    const panel = src('../src/sidebar/TasksPanel/TasksPanel.tsx')
    const host = src('../src/sidebar/TasksPanel/TasksPanelHost.tsx')
    const shell = src('../src/overlays/ModalShell.tsx')

    ok(
      /import \{[^}]*\bOverlayCard\b[^}]*\} from '@\/overlays\/ModalShell'/.test(detail),
      'the card gets its dialog from `overlays/ModalShell`, and does not define a second one',
    )
    ok(
      /export function TaskDetailModal[\s\S]{0,600}<OverlayCard[\s\S]{0,400}<TaskDetail\b/.test(detail),
      '`TaskDetailModal` is an `OverlayCard` wrapped around `TaskDetail` — the whole card is ' +
        'inside the dialog, and the wrapper is the only thing outside the render gate',
    )
    ok(
      !/role="dialog"|aria-modal|className=\{styles\.scrim\}/.test(detail),
      'and the card declares NO dialog semantics of its own: no `role="dialog"`, no ' +
        '`aria-modal`, no scrim. It inherits them, which is what stops this dialog from drifting ' +
        'away from the Settings roles dialog and `ConfirmDestructive`',
    )

    ok(
      /export function OverlayCard[\s\S]*?role="dialog"/.test(shell),
      'and what it inherits is real: `OverlayCard` declares `role="dialog"`',
    )
    ok(
      /export function OverlayCard[\s\S]*?aria-modal="true"/.test(shell),
      '...and `aria-modal="true"`, so assistive technology treats the board behind it as inert',
    )
    ok(
      /export function OverlayCard[\s\S]*?data-audit="overlayScrim"/.test(shell),
      '...and draws the scrim the card is dismissed by',
    )
    ok(
      /export function OverlayCard[\s\S]*?createPortal\(/.test(shell),
      '...through a portal, which is the line this block exists on the wrong side of',
    )

    ok(
      /<TasksPanelView\b/.test(host) && /<TaskDetailModal\b/.test(host),
      'the host mounts the list AND the card, which is what puts the board behind the scrim ' +
        'rather than replacing it',
    )
    ok(
      !/<TaskDetail\b/.test(panel),
      'and the view no longer renders the card in place. If it did, the portal inside it would ' +
        'take every board state, every group and both empty screens out of this file’s reach ' +
        'along with it — the stories above would not render at all',
    )
  }

  // ================================================================= both panels =========

  for (const [name, d] of [...Object.entries(agents), ...Object.entries(tasks)]) {
    eq(
      d.unclassed,
      0,
      `${name}: every audit-hooked element carries a real class, and no class list contains ` +
        'the literal token `undefined` — the two shapes a `styles.typo` takes, both invisible ' +
        'to tsc and to vite build',
    )
  }

  if (failed === 0) {
    console.log(
      `check-agents-render: ok (${Object.keys(agents).length} agents stories, ` +
        `${Object.keys(tasks).length} tasks stories)`,
    )
  } else {
    console.error(`\n${failed} failure(s)`)
  }
} finally {
  rmSync(out, { recursive: true, force: true })
}

process.exit(failed > 0 ? 1 : 0)

/**
 * Bundle one smoke entry for SSR, run it, and return its digests keyed by story name.
 *
 * `console.log` is captured rather than parsed off stdout so a stray log from a dependency
 * cannot become the digest; the *last* line is taken, which is the contract both smoke entries
 * follow.
 */
async function render(entry, tag) {
  const dir = join(out, tag)
  execFileSync(
    'node',
    [
      'node_modules/vite/bin/vite.js',
      'build',
      '--ssr', entry,
      '--outDir', dir,
      '--logLevel', 'error',
    ],
    { stdio: 'inherit' },
  )
  const printed = []
  const log = console.log
  console.log = (line) => printed.push(line)
  try {
    await import(`file://${resolve(dir, 'smokeEntry.js')}`)
  } finally {
    console.log = log
  }
  return Object.fromEntries(JSON.parse(printed.at(-1)).map((d) => [d.story, d]))
}
