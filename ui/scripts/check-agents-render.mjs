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
 *    working. This is the most important assertion in the file. Since the stalled reading it is
 *    three-way: within `list`, T14 (doing, no run — stalled) must also differ from T15
 *    (assigned), because a `doing` task nobody is on rendered identically to a plain
 *    assignment is the orchestrator's orphaned-task report — three orphanings, nothing on the
 *    board — restated as CSS.
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
 * own pause and resume are bare marks — `pause` and `play` — so a label carrying a word is the
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
/*
 * The scope buttons carry a word beside their mark, and the row buttons carry the mark alone.
 * That distinction is the thing being asserted, so it survived the move from characters to
 * drawn marks — only the way it is spelled changed. An SSR'd `<Icon>` has no text node, so the
 * mark's name comes off `data-icon` (see `icons/Icon.tsx`) and the button's *text* is now just
 * the word.
 */
const RESUME_ALL = 'Resume'
const PAUSE_ALL = 'Pause'

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
      'with a mark in the dot column — an empty cell reads as a rendering fault',
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
   * ===== The source badge, which is a claim about a file cide did not write. =============== (M30)
   *
   * Three assertions, and the third is the one that makes the first two mean anything.
   *
   * A badged row exists at all. **Both** Claude scopes badge, with the *same* text — so the mark
   * is keyed on the family and not on one directory, which is what `scopeBadge`'s doc promises
   * and what a single-row story could not distinguish. And the cide role standing beside them
   * carries **none**: without that, a chip drawn unconditionally on every row would pass.
   */
  {
    const d = a('role-claude-code')
    eq(d.roles?.length, 3, 'a cide role and both Claude Code scopes, in one story')
    const badged = (d.roles ?? []).filter((r) => r.badge !== '')
    eq(
      badged.map((r) => r.id),
      ['code-reviewer', 'researcher'],
      'exactly the two subagents are badged',
    )
    eq(
      badged.map((r) => r.badgeScope),
      ['claudeProject', 'claudeGlobal'],
      'and each names the scope it came from, which is what the Settings screen opens the ' +
        'right file from',
    )
    eq(
      [...new Set(badged.map((r) => r.badge))],
      ['Claude Code'],
      'one mark for both, because which of the two a subagent is in matters when you go to ' +
        'edit it and not when you are reading a roster',
    )
    eq(
      (d.roles ?? []).find((r) => r.id === 'developer')?.badge,
      '',
      'and cide’s own role carries NO badge — a chip on every row would say nothing, which is ' +
        'the assertion the other two depend on',
    )
    // A badged row is an ordinary row in every other respect. If it were not, this is where it
    // would show: the subagents dispatch, take Configure, and refuse nothing.
    for (const role of badged) {
      ok(
        role.buttons.includes('Dispatch') && role.buttons.includes('Configure'),
        `${role.id}: a subagent row is dispatchable and configurable like any other`,
      )
      eq(role.reason, '', `${role.id}: and refuses nothing`)
    }
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
    ok(
      d.glyphs?.includes('circle-slash'),
      'the unknown phase gets the fallback mark — a "no such thing" sign rather than nothing',
    )
    ok(
      d.roles?.some((r) => r.glyph === 'circle-slash' && r.status === 'Unknown'),
      'and its role summarises as Unknown rather than as nothing',
    )
    ok(
      !d.glyphs?.some((g) => g === '' || g.includes('function')),
      'and never an empty cell or a stringified `Object.prototype.constructor`, which is what ' +
        'a `?? fallback` over a prototype key hands back',
    )
  }

  /*
   * The running mark **turns**, and nothing else does.
   *
   * `PHASE_GLYPH` picks a three-quarter arc for `running` *because* it reads as turning — its own
   * comment says `idle`'s ringed dot is only distinguishable from it on that reading. It animated
   * nowhere: not here, not on the task list's chip, not on the card's run strip. So for a
   * milestone and a half the single signal that an assigned agent was working was a static shape
   * beside the state the shape was chosen to contrast with, and it was reported as exactly that.
   *
   * Read off the **class**, because the icon name was always right — a digest of names passed
   * every one of those renders. `check:agents` pins the other half, that the rule and the glyph
   * table name the same mark.
   */
  {
    const spinning = new Set()
    const still = new Set()
    for (const story of Object.keys(agents)) {
      for (const entry of agents[story].glyphSpins ?? []) {
        const [glyph, how] = entry.split('|')
        ;(how === 'spin' ? spinning : still).add(glyph)
      }
    }
    ok(spinning.size > 0, 'some story renders a running run at all — else this proves nothing')
    eq([...spinning], ['loader-circle'], 'and only the spinner mark is ever drawn turning')
    ok(!still.has('loader-circle'), 'the spinner is never drawn still')
    ok(still.has('circle-dot'), 'while idle — the mark it must be told apart from — sits still')
  }

  {
    const d = a('role-paused')
    ok(d.icons?.includes('play'), 'a paused run offers Resume')
    ok(!d.icons?.includes('pause'), 'and not Pause as well — the row draws one or the other')
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
      d.icons?.filter((i) => i === 'pause').length,
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

    /*
     * The stalled reading, in the same pair of stories. T14 is `doing` and assigned, and in
     * `list` no run exists at all — the orphaned state the orchestrator hit three times with
     * nothing visible on the board. Its chip must carry attention while T15's (assigned, not
     * started on anything the tracker claims is moving) stays plain — and the two class sets
     * must actually differ, because tone is only real if a stylesheet spends it.
     */
    eq(
      idle.chipTone,
      ['attention', 'assigned', 'assigned'],
      'with no runs, the doing task’s chip is the STALLED one — the task claims work is under ' +
        'way and no run is on it — and the merely-assigned chips stay plain',
    )
    eq(
      live.chipTone,
      ['live', 'assigned', 'assigned'],
      'one working run flips the same chip to live: stalled and working are the same board one ' +
        'run apart, exactly like assigned and working',
    )
    const classSet = (attr) => JSON.stringify([...new Set((attr ?? '').split(/\s+/))].sort())
    ok(
      classSet(idle.chipClasses?.[0]) !== classSet(idle.chipClasses?.[1]),
      'the stalled chip’s classes differ from the assigned chip’s IN THE SAME STORY — "nobody ' +
        'is coming" painted like "assigned, quietly" is the invisibility that let a task be ' +
        'orphaned three times unnoticed',
    )
    ok(
      classSet(idle.chipClasses?.[0]) !== classSet(live.chipClasses?.[0]),
      'and from the lit chip’s — stalled must not read as working, which is the original ' +
        'most-important assertion extended to the third rendering',
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
    /*
     * And its mark **turns**.
     *
     * `PHASE_GLYPH` picks a three-quarter arc for `running` *because* it reads as turning — its
     * own comment says `idle`'s ringed dot is distinguishable from it only on that reading. It
     * animated nowhere, so for a milestone and a half the one signal that an assigned agent was
     * working was a static shape sitting next to the state it was chosen to contrast with. Read
     * off the class rather than the icon name, because the icon name was always right.
     */
    eq(live.runStripSpin, true, 'the run strip\u2019s running mark carries the class that spins it')
    ok(live.buttons?.includes('Open'), 'and the strip can attach a pane to it')

    /*
     * ----- Markdown, drawn and written. (M27) --------------------------------------------
     *
     * Reported as: comments and the body need markdown rendering, and styling tools when
     * writing. Two halves, and each has a failure only markup can show. Rendering: the parser
     * and the renderer both existed and compiled long before anything called them — "built and
     * reachable from nothing" is this repository's oldest failure class. Tools: the toolbar
     * rides `MentionTextarea`'s `tools` prop, which every call site must *pass*, and a site
     * that forgot it renders a working textarea with no tools and nothing else wrong.
     */
    const md = t('card-markdown')
    ok(
      (md.md?.strong ?? 0) >= 1 && (md.md?.em ?? 0) >= 1,
      'a fixture that wrote **strong** and *emphasis* produced real <strong> and <em> elements',
    )
    ok((md.md?.headings ?? 0) >= 1, 'and `## Scope` is a heading element, not a line of hashes')
    ok((md.md?.code ?? 0) >= 2, 'code spans render in the body and in the comment alike')
    ok((md.md?.fences ?? 0) >= 1, 'a fenced block is a <pre>, uncoloured on purpose')
    ok((md.md?.items ?? 0) >= 4, 'both lists render as list items — the body’s two and the comment’s two')
    ok((md.md?.links ?? 0) >= 1, 'a link renders marked — inert by decision, but visibly a destination')
    ok(
      !md.text?.includes('**') && !md.text?.includes('```') && !md.text?.includes('## '),
      'and the syntax was consumed: no marker survives into the prose the user reads',
    )
    ok(
      !JSON.stringify(md).includes('dangerouslySetInnerHTML'),
      'rendered through the AST as React elements — never an HTML string handed to the DOM',
    )
    /*
     * The plain stories still flatten to the same prose they always did — a markdown renderer
     * over text with no markdown in it must be invisible. `fieldValues` and `comments` above
     * are the pin; this line states why they did not have to change.
     */
    eq(
      rest.comments?.length,
      3,
      'the plain card still digests all three comments through the markdown road',
    )

    /*
     * The styling tools. Eight per markdown-bearing textarea — the composer’s row is always on
     * the card, and a field editor brings its own. Counted, not looked for: "some textareas
     * have tools" is the drift the count exists to make visible.
     */
    eq(rest.mdTools, 8, 'at rest the comment composer carries the eight formatting tools')
    eq(
      body.mdTools,
      16,
      'with the body in edit its editor carries eight more — the tools follow the textarea, ' +
        'so a field editor cannot lose them while the composer keeps its own',
    )
    eq(t('list').mdTools, 0, 'the list has no textarea and therefore no toolbar')

    /*
     * ----- The status log. (M27) ----------------------------------------------------------
     *
     * Asked for in as many words: *a log of status change (by whom and when and in what
     * status), collapsed by default*. The collapsed assertion is the load-bearing one — a
     * stray `open` on the `<details>` would ship every card with its audit trail unfolded,
     * and nothing but markup can see that.
     */
    eq(rest.historyDrawn, true, 'a task that has moved draws its status log')
    eq(
      rest.historyOpen,
      false,
      'COLLAPSED by default — one quiet summary line until it is asked for',
    )
    eq(
      rest.history,
      ['Todo → Doing|Orchestrator', 'Doing → Review|You', 'Review → Doing|You'],
      'oldest first from a fixture that arrives scrambled, each row naming who moved it — ' +
        'the automatic dispatch hop reads as the Orchestrator’s, which is who dispatched',
    )
    eq(
      bare.historyDrawn,
      false,
      'a task that never moved draws no disclosure — a control that opens onto nothing',
    )
    ok(
      (rest.times?.length ?? 0) >= 6 &&
        rest.times.every((stamp) => /^\d{2}:\d{2}:\d{2} \(.+ ago\)$/.test(stamp)),
      'every timestamp on the card — three comments, three history rows — is the wall clock ' +
        'plus the age: `HH:MM:SS (… ago)`, because two `4m ago`s are indistinguishable and a ' +
        'bare clock time is ambiguous across days',
    )
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
    /*
     * The links row rides its own hook — `taskComposeLinkRow`, never `taskComposeRow` — exactly
     * as the spec row does and for its stated reason, so the three above stay three with the
     * row on screen. (M30)
     */
    {
      const linking = t('compose-linking')
      eq(
        linking.composeControls,
        3,
        'the count the paragraph above argues is UNMOVED by the links row — its selects are ' +
          'under their own hook, which is the honest fix the spec row already made',
      )
      eq(linking.composeLinkRow, true, 'the row draws when the board offers targets')
      eq(
        linking.composeLinkChips,
        ['blockedBy|t-14'],
        'a picked edge renders as a chip in the draft — choosing the target IS the add, the ' +
          'assignee select’s choosing-is-the-commit argument',
      )
      eq(empty.composeLinkRow, false, 'and no `tasks` prop is no row at all — the pre-M30 claim')
      eq(filled.composeLinkRow, false, 'in both of the stories whose digests predate it')
    }
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

    eq(
      empty.mdTools,
      8,
      'the body box carries the eight formatting tools — "styling tools when creating" is the ' +
        'half of the markdown report that names this dialog',
    )

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
   * ===== The assignee options, the hint, and the @mention popup. ==========================
   *
   * The dropdown shipped empty for the whole life of one milestone because the host passed `{}`
   * where the roster's map belonged, and no gate saw it: `check:agents` proved `assignableRoles`
   * correct over a populated map, and these stories rendered the populated path while the app
   * ran the empty one. The host wiring is source-grepped below with the other seams; what THIS
   * block pins is the two on-screen halves — a populated roster becomes options, and an empty
   * one comes with the sentence saying why, so the legitimate state and the bug cannot look
   * alike again.
   */
  {
    eq(
      t('card-editing-assignee').assigneeOptions,
      ['Unassigned', 'Developer', 'QA'],
      'a populated roster is on offer, labels in sorted-id order behind the Unassigned row',
    )
    eq(
      t('card-editing-assignee').assigneeHint,
      '',
      'and a ready roster needs no excuse under it',
    )

    const noRoles = t('card-assignee-no-roles')
    eq(
      noRoles.assigneeOptions,
      ['Unassigned', 'developer'],
      'an empty roster still folds the current assignee in — `assignableRoles`’ honesty rule, ' +
        'now visible: without it the select would silently show Unassigned and reassign the ' +
        'task to nobody the moment it was touched',
    )
    ok(
      noRoles.assigneeHint.startsWith('Subagents are off for this project.'),
      `the sentence saying why the list is short is on screen: ${noRoles.assigneeHint}`,
    )

    const compose = t('compose-no-roles')
    eq(compose.assigneeOptions, ['Unassigned'], 'the dialog over an empty roster offers only Unassigned')
    ok(
      compose.assigneeHint.startsWith('Subagents are off for this project.'),
      'with the same sentence under it',
    )
    eq(t('compose-filled').assigneeHint, '', 'and none when the roster is real')

    /*
     * The popup, in its open state — which only these stories can render: the component mounts
     * it from DOM events a server render cannot fire. The options came through the real
     * `mentionOptions`, so the order on screen IS the scored order.
     */
    const open = t('mention-open')
    eq(
      open.mentionRows,
      ['@code-reviewer Code Reviewer|false', '@developer Developer|true', '@qa QA|false'],
      'the open popup lists the roster id-first with the label beside it, in scored order',
    )
    eq(
      open.mentionRows.filter((row) => row.endsWith('|true')).length,
      1,
      'exactly one row is aria-selected — the highlight the arrows move',
    )
    eq(
      t('mention-filtered').mentionRows,
      ['@developer Developer|true'],
      'a narrowing query narrows the rows',
    )
    for (const name of ['mention-open', 'mention-filtered']) {
      eq(t(name).unclassed, 0, `${name}: names only classes its stylesheet defines`)
    }
  }

  /*
   * The `check-problems.mjs` regression, on the tasks side.
   */
  {
    const d = t('rogue-status')
    eq(d.rows?.length, 4, 'a status nobody recognises does not drop the task from the tracker')
    eq(d.meta, '4', 'nor zero the header count')
    ok(
      d.rows?.some((r) => r.includes('|circle-slash|t-18|')),
      'the unknown status gets the fallback mark rather than an empty marker cell',
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

    /* ----------------------------------------------- the OpenSpec block's three states (M28) */

    /*
     * A change costs four `openspec` invocations, each of them a node process — about six tenths
     * of a second now that they run at once, and two and a half before that. The card has to say
     * something for that whole time, and it said nothing: the block was behind `spec != null`
     * for *both* states, so a task with a change opened looking exactly like a task without one
     * and then grew a whole section out of nowhere.
     *
     * That shipped despite the prop's own doc saying `null` "draws the chip and a pending
     * progress row rather than nothing", because none of the three states had a story. These are
     * those stories, and the pair below is the assertion the doc was making.
     */
    eq(
      t('card-spec-reading').specBlock,
      'reading',
      'a linked task says its change is being read, from the moment the card opens',
    )
    eq(
      t('card-spec-reading').specChip,
      'add-dark-mode',
      'and names which change, off the task itself — the card carries the name before the read ' +
        'answers, so there is no reason to withhold it for six tenths of a second',
    )
    /*
     * The read finished and there is nothing — most often the change was archived, which is a
     * thing the user did. Its pair with `card-spec-reading` is the whole point: the same `null`
     * means two different things, and collapsing them is a spinner that never stops. The host had
     * no `.catch` at all, so a rejection went nowhere and the card sat on "Reading…" for ever.
     */
    eq(t('card-spec-failed').specBlock, 'failed', 'a read that came back with nothing says so')
    ok(
      (t('card-spec-failed').text ?? '').includes('archived'),
      'and names the likeliest reason rather than showing a spinner that never stops',
    )
    ok(
      !(t('card-spec-reading').text ?? '').includes('archived'),
      'while the one still reading claims nothing about why',
    )

    /*
     * And the optionality claim, restated as an assertion rather than as prose: a task with no
     * change draws no block and no chip, so its markup is what it was before M28.
     */
    eq(t('card').specBlock, null, 'a task with no change draws no OpenSpec block at all')
    eq(t('card').specChip, null, 'and no chip')

    /* ------------------------------- handed to a conversation, and the button that must go (M28) */

    /*
     * Choosing *New Claude session* started the session, typed the task in — and left the card
     * still offering **Approve & dispatch** over a conversation that was already working.
     * Pressing it again would open the picker and invite a second dispatch of the same task.
     *
     * It shipped because the card only ever read `assignee`, and `TaskStore::edit` **clears**
     * `Task::agent` when a session is set: the one gesture that hands work to a conversation left
     * every field the button was derived from exactly as it found them. So this pair — the row
     * appearing and the button going — is one claim and is asserted as one.
     */
    eq(t('card-spec-ready').specAction, 'approve', 'a change nobody has dispatched offers approval')
    eq(t('card-spec-ready').specSession, null, 'and draws no session row')

    eq(
      t('card-spec-session').specAction,
      null,
      'a change whose work is with a conversation draws no Approve & dispatch — the gesture has ' +
        'been made, and a second press would dispatch the same task twice',
    )
    eq(
      t('card-spec-session').specSession,
      'working',
      'what stands in its place is the session row, saying the conversation is working',
    )
    eq(
      t('card-spec-session-awaiting').specSession,
      'awaiting',
      'and saying so when it has finished a turn and is waiting',
    )
    /*
     * `Task::session` records where the work went and deliberately survives the pane closing, so
     * *closed* is a state the card must be able to draw. Three and not two: it is not a quieter
     * shade of "not waiting", it is work with nowhere to continue.
     */
    eq(
      t('card-spec-session-closed').specSession,
      'closed',
      'a conversation whose pane is gone says so rather than reading as idle',
    )
    /*
     * **A closed conversation keeps every control**, and that is the fix rather than an
     * oversight. Open was withheld once the pane was gone, on the reasoning that a control
     * opening onto nothing is what this card refuses — which left a closed conversation with no
     * way to proceed except handing the work somewhere else, throwing away everything it had
     * already worked out.
     *
     * The premise was wrong. A closed conversation is not nothing: its transcript is on disk
     * under the id cide passed to `--session-id`, and `claude --resume <id>` brings all of it
     * back under the same id — so the task's own record of where the work went keeps naming the
     * right conversation.
     */
    eq(
      t('card-spec-session-closed').specSessionControls,
      ['specSessionResume', 'specSessionOpen', 'specSessionElsewhere'],
      'a closed conversation can still be resumed and reopened — withholding those left the ' +
        'only road forward as starting again somewhere else',
    )
    eq(
      t('card-spec-session').specSessionControls,
      ['specSessionResume', 'specSessionOpen', 'specSessionElsewhere'],
      'and a live one draws the same three, so the row does not rearrange itself under the ' +
        'pointer when a pane is closed in another window',
    )
    ok(
      (t('card-spec-session-closed').text ?? '').includes('--resume'),
      'and the hint names the mechanism rather than leaving a grey chip to be interpreted',
    )

    /*
     * ------------------------------------------ and then the change is archived (M31)
     *
     * What shipped, on a card that drew both the archive line and the session row:
     *
     *     Archived
     *     archived as 2026-08-27-add-todo-list
     *     Working                  Conversation a7d4fd80   Resume  Open  Hand it elsewhere
     *     It is working. The checklist above ticks as it goes.
     *
     * Three separate false claims — a live-work chip, a sentence about a checklist this arm
     * draws no bar for, and two controls that dispatch against a change directory `openspec
     * archive` has moved. `primaryAction` had refused the arm since M28; this row was the second
     * door onto the same gesture and carried none of that reasoning.
     *
     * Asserted as a **pair** against `card-spec-session`, which is the identical open,
     * not-awaiting conversation. Without the pair the archived assertions would pass just as
     * happily if the row had gone quiet for some unrelated reason, and the claim is specifically
     * that the archive is what changed it.
     */
    const archivedRow = t('card-spec-archived-session')
    eq(t('card-spec-session').specSession, 'working', 'the live conversation reads as working')
    eq(
      archivedRow.specSession,
      'archived',
      'and the identical conversation under an archived change does not',
    )
    eq(
      archivedRow.specSessionControls,
      ['specSessionOpen'],
      'only Open survives: Resume hands the task back and Hand it elsewhere dispatches it anew, ' +
        'and both act on a change that is merged into openspec/specs/ and no longer there',
    )
    ok(
      !(archivedRow.text ?? '').includes('It is working'),
      'and nothing on the card claims the work is still happening',
    )
    ok(
      !(archivedRow.buttons ?? []).some((label) => label === 'Hand it elsewhere'),
      'the dispatch road is gone from the row, not merely restyled',
    )

    /*
     * And the role road is untouched, which is the other half of the fix. A task assigned to a
     * role has no session — Rust clears one when the other is set — so no row is drawn and the
     * run strip reports the subagent exactly as it did.
     */
    eq(t('card-with-live-run').specSession, null, 'a run on a role draws no session row')

    /*
     * == Typed links. (M30) ================================================================
     *
     * The Links section is a section and not a fifth field — `check-agents.mjs` pins the field
     * vocabulary at four with the argument — so the assertions here are about what only markup
     * can settle: which chips draw, which of them carry a remove, and that none of it moves the
     * read-only-at-rest claim one pixel.
     */
    {
      const linked = t('card-linked')
      eq(
        linked.linkChips,
        [
          'blockedBy|out|t-14|false',
          'subtaskOf|out|t-16|false',
          'related|out|t-17|false',
          'blockedBy|out|t-99|true',
          'blockedBy|in|t-41|false',
        ],
        'five chips: the three stored kinds in order, the dangling t-99 MARKED rather than ' +
          'hidden (a reference that silently vanished is the task-leaves-the-tracker failure, ' +
          'one edge over), and the derived `blocks` reading from t-41’s own stored edge — ' +
          'stored once, read from both ends',
      )
      ok(
        linked.buttons?.includes('Blocked by t-14'),
        'a chip is a BUTTON — it navigates to its target — not a decorated span',
      )
      ok(
        linked.buttons?.includes('Blocked by t-99 (gone)'),
        'and the dangling one says so in words as well as in the attribute',
      )
      eq(
        linked.linkRemoves,
        4,
        'a remove on each of the four outgoing chips and NONE on the derived directed one: ' +
          'that edge belongs to the other task, and its chip is the road there',
      )
      eq(
        linked.fields,
        ['title|rest', 'status|live', 'assignee|rest', 'body|rest'],
        'the four fields stand exactly as `card` draws them — links moved nothing',
      )
      eq(
        linked.fieldControls,
        0,
        'and the card is still read-only at rest: the link controls live outside the field ' +
          'rows, which is the markup half of links-are-a-section',
      )
      eq(linked.linkAddControls, 0, 'the picker is shut until asked for')
      ok(linked.buttons?.includes('Link…'), 'and the affordance that opens it is drawn')

      const adding = t('card-link-adding')
      eq(
        adding.linkAddControls,
        2,
        'open, the picker is a kind `<select>` and a target search `<input>` — the reported ' +
          'gesture is "find the task by its key or summary", which a native dropdown answers ' +
          'with scroll — and both are still outside `fieldControls`, whose zero stands',
      )
      eq(adding.fieldControls, 0, 'stated as its own line because it is the claim that matters')
      ok(
        !adding.buttons?.includes('Add link'),
        'and there is NO Add button: picking a row IS the commit, the assignee select\u2019s ' +
          'choosing-is-the-commit argument — a confirm after the pick would have nothing left ' +
          'to do',
      )
      ok(adding.buttons?.includes('Cancel'), 'the way out is the header affordance')

      /*
       * The search popup itself, drawn open through the exported `LinkTargetList` — the same
       * arrangement the mention popup uses, because the stateful input only ever opens it from
       * DOM events a server render cannot fire. The rows are the model's tier order, so this is
       * the markup half of "the key reaches by prefix, the summary by word".
       */
      const targetsOpen = t('link-target-open')
      eq(
        targetsOpen.linkTargetRows.length,
        4,
        'the empty query offers the whole target list (four tasks fit under the cap)',
      )
      eq(
        targetsOpen.linkTargetRows.filter((row) => row.endsWith('|true')).length,
        1,
        'exactly one row is aria-selected — the highlight the arrows move',
      )
      eq(
        t('link-target-filtered').linkTargetRows,
        ['t-14 Add the retry bar Doing|true'],
        'a query reaches by the SUMMARY: `retry` finds t-14 through its title, with the status ' +
          'on the row because which tasks are already done is half of choosing a blocker',
      )

      /* The pre-M30 claim, as an assertion rather than as prose. */
      eq(t('card').linkChips, [], 'a card handed no links prop draws no chips')
      eq(t('card').linkRemoves, 0, 'no removes')
      ok(
        !(t('card').text ?? '').includes('Links'),
        'and no Links heading at all — its markup is what it was before M30',
      )
    }

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
      'and the card itself is NOT in this view’s markup: it is mounted by `TaskDetailHost` ' +
        'beside it, which is what keeps the portal out of this render',
    )
  }

  /*
   * ===== The search box. ===================================================================
   */
  {
    const all = t('list')
    eq(all.search, true, 'a ready board with tasks in it draws the search box')
    eq(all.searchValue, '', 'empty until the user types')
    eq(all.searchClear, false, 'and with nothing to clear, no clear control')

    const hit = t('list-searched')
    eq(hit.rows?.length, 1, 'a query narrows the list')
    ok(
      hit.rows?.[0]?.includes('t-14'),
      'to the tasks whose text contains it — and the story types `Retry` over a lowercase ' +
        'title, so the case-insensitivity is pinned in markup as well as in the model',
    )
    for (const id of ['t-15', 't-16', 't-17']) {
      ok(
        !hit.text?.includes(id),
        `${id} is ABSENT from the markup rather than merely dimmed — the filter’s rule, ` +
          'because a search that left the rows on screen would be a highlight, which is a ' +
          'different feature',
      )
    }
    eq(
      hit.meta,
      all.meta,
      'the header figure does not follow the search, for the filter’s reason: it names the ' +
        'tracker, not the slice being read',
    )
    eq(hit.filters?.length, 5, 'the filter row is still drawn — the two narrowings compose')
    eq(hit.searchValue, 'Retry', 'the box holds what was typed')
    eq(hit.searchClear, true, 'and with text in it, the clear control exists')

    const none = t('list-search-no-match')
    eq(none.rows, [], 'a query can match nothing at all')
    eq(none.noMatch, true, 'and that is its own screen rather than a blank body')
    ok(
      /quaternion/.test(none.claim ?? ''),
      'whose sentence names the QUERY that is hiding them — the filter’s empty screen names a ' +
        'status, and the way out of this one is clearing what was typed instead',
    )
    ok(none.text?.includes('3 tasks'), 'and counts the tasks that do exist, so the number argues')
    ok(none.buttons?.includes('Clear search'), 'with the way out in the sentence’s own row')
    ok(
      !none.buttons?.includes('Show all tasks'),
      'and no Show all beside it: with no filter on there is nothing that button would undo, ' +
        'and a control that changes nothing teaches the user to ignore the one beside it',
    )
    eq(
      none.search,
      true,
      'the box itself is still drawn — a search that hid its own control when it matched ' +
        'nothing would leave the user no way to clear what they typed',
    )

    eq(t('empty').search, false, 'an empty tracker draws no search box: there is nothing to find')
    eq(t('absent').search, false, 'nor does a project with no tracker file')
    eq(t('unreadable').search, false, 'nor an unreadable one, which offers only Reveal and Retry')
  }

  /*
   * ===== The recency order. ================================================================
   */
  {
    const d = t('list-recent-first')
    eq(
      d.rows?.map((r) => r.split('|')[2]),
      ['t-33', 't-32', 't-31'],
      'rows come last-touched-first. The fixture writes them to the file oldest-first — the ' +
        'order an append-only tracker accretes — so this passing is the sort and not the ' +
        'fixture: a panel drawing the array as written would print the exact reverse',
    )
    eq(
      d.groups,
      ['Todo|3'],
      'all three in one group, so the order above is within-group order rather than ' +
        'GROUP_ORDER doing the work',
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
    const card = src('../src/sidebar/TasksPanel/TaskDetailHost.tsx')
    const app = src('../src/App.tsx')
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
      /<TasksPanelView\b/.test(host) && !/<TaskDetailModal\b/.test(host),
      'the panel host mounts the list and no longer the card — the card left with the ' +
        'Agents-panel task links, which open it without switching the sidebar to Tasks',
    )
    ok(
      /<TaskDetailModal\b/.test(card),
      'the card is mounted by `TaskDetailHost` instead, which holds the wiring the panel host ' +
        'used to: the field-in-edit draft, the card’s armed delete, and the 30 s comment clock',
    )
    ok(
      /\{taskSelected !== null &&[\s\S]{0,400}<TaskDetailHost\b/.test(app),
      'and `App.tsx` mounts that host OUTSIDE every `sidebar.view` branch, gated on the ' +
        'selection — which is the whole feature: a task opened from the Agents panel appears ' +
        'over that panel rather than yanking the sidebar to Tasks first',
    )
    ok(
      /<PanelBoundary name="Task"[\s\S]{0,400}<TaskDetailHost\b/.test(app),
      '...wrapped in a `PanelBoundary` whose Close clears the selection, so a throw in the ' +
        'card cannot empty the window and the unmount is what re-arms the boundary',
    )
    ok(
      !/onShowTasks/.test(src('../src/sidebar/AgentsPanel/AgentsPanelHost.tsx')),
      'and the Agents panel no longer takes an `onShowTasks` — a task link selects the task ' +
        'and nothing else, because switching panels was the gesture the user asked to remove',
    )
    ok(
      !/<TaskDetail\b/.test(panel),
      'and the view no longer renders the card in place. If it did, the portal inside it would ' +
        'take every board state, every group and both empty screens out of this file’s reach ' +
        'along with it — the stories above would not render at all',
    )

    /*
     * The join with the agents store — the seam the empty-dropdown bug lived in, and the one
     * neither render above can reach: the host reads stores and calls IPC, so it is source only
     * here. The stories rendered the populated path for a whole milestone while the app ran the
     * empty one; these lines are what make un-wiring it a failure instead of a regression.
     */
    for (const [name, source] of [['panel host', host], ['card host', card]]) {
      ok(
        /import \{[^}]*\buseAgents\b[^}]*\} from '@\/sidebar\/agentsStore'/.test(source),
        `the ${name} imports the agents store — the roster is where \`roles\` and \`runs\` ` +
          'come from now',
      )
      ok(
        /useAgents\(\(s\) => s\.roster\)/.test(source),
        `and the ${name} subscribes its roster`,
      )
      ok(
        /rosterRoles\(roster\)/.test(source),
        `the ${name} derives the roles map through \`rosterRoles\` — the join in model form, ` +
          'which `check:agents` tests over every roster arm',
      )
      ok(
        !source.includes('NO_ROLES'),
        `no hardcoded empty roles constant in the ${name}. It was the bug: the header promised ` +
          '"two lines change when the store lands", the store landed, and nothing changed',
      )
    }
    eq(
      [...host.matchAll(/roles=\{roles\}/g)].length,
      2,
      'the derived roles reach both of the panel host’s mounts — the list and the compose dialog',
    )
    eq(
      [...card.matchAll(/roles=\{roles\}/g)].length,
      1,
      '...and the card host’s one — the card, whose assignee dropdown is where the ' +
        'empty-forever bug lived',
    )
    ok(
      [...host.matchAll(/runs=\{runs\}/g)].length >= 1 &&
        [...card.matchAll(/runs=\{runs\}/g)].length >= 1,
      'and the derived runs reach the list and the card',
    )

    /*
     * The @mention popup, at its call sites. Counted as source because which textareas offer
     * mentions is a decision, not a rendering: a "cleanup" that swapped one back to a plain
     * `<textarea>` would still render, still pass every digest, and silently drop the feature
     * from that field.
     */
    const compose = src('../src/sidebar/TasksPanel/TaskCompose.tsx')
    ok(
      [...detail.matchAll(/<MentionTextarea\b/g)].length >= 2,
      'the card offers mentions in the body editor and the comment composer at least',
    )
    eq(
      [...compose.matchAll(/<MentionTextarea\b/g)].length,
      1,
      'and the compose dialog in its body — one each, the title is one line and mentions nobody',
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

  /* ----------------------------- the dispatch row at the foot of the card (M28) */

  {
    /*
     * **This is the assertion whose absence let the control ship invisible.**
     *
     * Until `card-spec-ready` no story anywhere carried a *loaded* spec — only "still reading"
     * and "could not be read" — so the card's most consequential control had never been rendered
     * by any check, and a version of it that drew nothing passed every one of them.
     */
    const ready = tasks['card-spec-ready']
    eq(ready.specPrimary, 'approve|on', 'a proposed change offers Approve & dispatch, live')
    ok(
      ready.buttons.includes('Approve & dispatch'),
      `and the words are on it: ${JSON.stringify(ready.buttons)}`,
    )

    /*
     * Approve immediately before Delete, in one row. They were two bordered blocks stacked, each
     * drawing its own rule — so a card with no change still showed the second rule above a lone
     * right-aligned Delete, which reads as a stray line rather than as a control.
     */
    const at = ready.buttons.indexOf('Approve & dispatch')
    eq(
      ready.buttons[at + 1],
      'Delete task',
      'Approve sits beside Delete in the same row, with nothing between them',
    )

    // Approve opens the picker; it does not assign on the spot. That is the whole gesture.
    eq(ready.dispatchKinds, [], 'the picker is closed until asked for')
    eq(
      tasks['card-spec-picking'].dispatchKinds,
      ['role', 'session', 'fresh'],
      'and offers all three kinds — a role dispatches through the queue, a conversation already ' +
        'open is typed into, and the last one makes a pane first',
    )

    // A task with no change has no dispatch control at all, rather than a disabled one.
    eq(tasks['card'].specPrimary, null, 'an ordinary task offers nothing to approve')
    eq(tasks['card-spec-reading'].specPrimary, null, 'and neither does one still being read')

    /*
     * ------------------------------------------------------ the accept, while it is running
     *
     * `Integrate & Archive` merges a branch, runs `openspec archive`, re-validates and closes the
     * task. It shipped drawing **nothing at all** while that happened: the card sat exactly as it
     * was, which is indistinguishable from a click that missed — and a second press would have
     * started a second merge.
     *
     * Three things, and each is a different way the fix could be half-done: the control goes
     * inert, it says something else, and what it says is not the resting label with a spinner
     * bolted on.
     */
    const accepting = tasks['card-spec-accepting']
    eq(accepting.specPrimary, 'accept|off|busy', 'inert and marked busy while the call runs')
    ok(
      accepting.buttons.includes('Integrating…'),
      `and it says so: ${JSON.stringify(accepting.buttons)}`,
    )
    ok(
      !accepting.buttons.includes('Integrate & Archive'),
      'a button that still reads Integrate & Archive looks pressable and is not',
    )
    /*
     * And the mark **turns**. Checked by the class that animates it and not by the icon's name,
     * because the first version drew exactly the right icon and nothing in the app animated it:
     * a static three-quarter arc, reported as *"loader was not spinning"*. It also sat in a
     * run-strip class with no layout of its own, so the button collapsed around a
     * baseline-aligned svg — `.actionBusy` is the other half.
     */
    eq(accepting.specBusyMark, 'spinning', 'the in-flight mark carries the class that turns it')
    eq(tasks['card-spec-ready'].specBusyMark, null, 'and there is no mark when nothing is running')
    eq(
      tasks['card-spec-ready'].specPrimary,
      'approve|on',
      'and nothing is busy when nothing was pressed',
    )

    /*
     * ------------------------------------------------ the accept that merges nothing (M31)
     *
     * The same control, over work that is already on the user's own branch. A task handed to a
     * Claude conversation carries no role — `TaskEdit::SetSession` clears `Task::agent` — so
     * `plan_accept` finds no branch and `spec_accept` merges nothing; the press is a plain
     * archive. The button nonetheless read *Integrate & Archive*, naming a step it would not
     * take, over a conversation that had already committed everything.
     *
     * Asserted through the rendered **text**, and as a pair against `card-spec-accepting`: the
     * claim is only worth anything if the two stories differ, because a label that always said
     * *Archive* would pass a one-sided check and lie the other way — over a run whose branch
     * really is waiting to be merged.
     */
    const archiveOnly = tasks['card-spec-archive-only']
    eq(archiveOnly.specPrimary, 'accept|on', 'still the accept gesture, and still pressable')
    ok(
      archiveOnly.buttons.includes('Archive'),
      `and it says what it will do: ${JSON.stringify(archiveOnly.buttons)}`,
    )
    ok(
      !archiveOnly.buttons.some((label) => label.includes('Integrate')),
      'never Integrate, because there is no branch to integrate',
    )

    /*
     * -------------------------------------------------- the road *into* OpenSpec (M28)
     *
     * The compose dialog used to offer *New change from this task*, which scaffolded a stub — no
     * delta specs, no checklist — so the task was born linked to a change `openspec validate`
     * refuses and whose row reads `0/0` for ever. Writing a proposal needs the codebase; it is a
     * conversation's job now, and this button starts one on it.
     *
     * The pair is the claim: drawn when the host passes a handler, and **absent** when it does
     * not — a project with no `openspec/`, or with no propose command installed, sees the card it
     * saw before M28.
     */
    eq(tasks['card-propose'].specPropose, true, 'a task with no change can start a proposal')
    ok(
      tasks['card-propose'].buttons.includes('Make a proposal'),
      `with words that say what it does: ${JSON.stringify(tasks['card-propose'].buttons)}`,
    )
    eq(
      tasks['card'].specPropose,
      false,
      'and no handler is no button — the optionality claim, made structurally',
    )
    eq(
      tasks['card-spec-ready'].specPropose,
      false,
      'nor is it offered on a task that already implements a change',
    )

    /*
     * ------------------------------------------------- the change, after it was archived (M28)
     *
     * The card said *"add-dark-mode could not be read. It may have been archived."* here, from
     * the moment the work landed onwards: `openspec archive` **moves** the directory and no CLI
     * command reads the result, so the task that did the work lost its record at exactly the
     * point the record was worth keeping. cide reads the directory itself now.
     *
     * Two of the three assertions are about what must **not** be drawn, and they are the ones
     * that would fail quietly. An archive cannot answer the checklist or the verdict — which
     * file is the checklist is the schema's business, and `openspec validate` cannot see a moved
     * directory — so both come back neutral: `0/0` and vacuously clean. Rendered as they stand
     * that is an empty progress bar and a green **Valid** over finished, merged work.
     */
    const archived = tasks['card-spec-archived']
    eq(archived.specBlock, 'archived', 'the block says which of the two reads this was')
    eq(archived.specBarPct, null, 'no progress bar at all, rather than one sitting empty at 0')
    eq(
      archived.specValidity,
      'unchecked|Archived',
      'and never `ok|Valid`: nothing validated this and nothing can',
    )
    ok(
      (archived.specProgressLabel ?? '').includes('2026-08-27-add-dark-mode'),
      `the line names the directory instead of counting steps: ${archived.specProgressLabel}`,
    )
    eq(archived.specPrimary, null, 'nothing prominent left to press on merged work')
    eq(
      archived.specDeltas,
      ['added|dark-mode'],
      'and the requirements are still drawn — recovered from the archived delta file by the same ' +
        'byte-range scanner the write path uses, which is the whole point of reading the ' +
        'directory rather than refusing',
    )
    // The pair: the same card before it was archived still counts steps and still validates.
    eq(tasks['card-spec-ready'].specBarPct, '33', 'a live change keeps its bar')
    eq(tasks['card-spec-ready'].specValidity, 'ok|Valid', 'and its verdict')

    /*
     * Delete is red before it is armed, not only after. The colour is what tells somebody
     * scanning the foot of the card which of the two buttons destroys something — and by the
     * time it is armed they have already pressed it.
     */
    for (const name of ['card', 'card-spec-ready']) {
      eq(tasks[name].deleteDanger, true, `${name}: Delete carries the danger colour unarmed`)
    }
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
