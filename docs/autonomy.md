# Running a project autonomously

For people who want cide to move a project forward while they are not watching it: roles
working a milestone, a planner filling the gaps, reviews and merges happening without you. This
page describes how that works since M132 and how to set a project up so it moves the product
rather than the board.

It is written from two real projects that did not move. terrastrike and selfcraft are Godot games
that ran for weeks on cide's loop before M132, and neither product moved much. The executor models
were not the bottleneck. The process spent their capacity:

- **Ceremony per micro-task.** Every task paid for a worktree, an import, a verify and an Opus
  reviewer session. That came to 153 reviewer sessions on terrastrike and 443 on selfcraft, each
  costing more than the change it reviewed.
- **Gates that were quality proxies.** A gate that measures something near the product can be
  satisfied without moving the product.
- **The inbox leaking into milestones.** Planners pulled from the inbox, so things a run happened
  to notice became the plan.
- **Parallel writers on the same files.** On selfcraft eight sprite tasks started together on
  `tools/spritegen/*`, and all but one had to rebase. In this loop a conflict costs about a whole
  second task: the merge refuses, the role rebases, verify runs again, and a reviewer reads it
  again.
- **Runs narrating in comments.** selfcraft's tracker held 6 MB of comments against 1.3 MB of
  game code. Every later reader of a task paid for all of it.
- **Models making taste decisions.** The user then rejected the art and UI they produced.
- **A planner that invented work** once nothing concrete was left.

Everything below is shaped by those failures. For what concurrent runs share on the machine, see
[`worktree-isolation.md`](worktree-isolation.md).

## The model

**You own intent and taste; cide runs concrete work between your check-ins.** Plan to check in
once or twice a day. At each check-in you:

1. look at *Waiting for you*: accept or send back visual work, answer questions;
2. glance at the inbox and promote what matters, or leave it;
3. with the console, update the active milestone's plan when it is done or wrong.

Between check-ins the roles work the milestone's tasks and a batch reviewer merges what is done.
The planner fills gaps **from the plan you approved** and stops with a question instead of
inventing work. Nothing a model noticed becomes work until you promote it. Nothing whose quality
is a matter of taste is called done until you have looked at it.

The loop is not built to run for a week unattended with an open brief. When the plan runs out it
stops and asks. That is the design: the failure it replaces is a planner that kept a project busy
with work nobody wanted.

## Setting up a project

### Roles: few, and narrow in concurrency

- **Two to four roles.** Use a feature builder, a mechanical worker (content, data, refactors,
  tests) and optionally a tooling role. Each extra role is another prompt to keep true, and the
  planner has to choose between roles on every task.
- **`max-concurrent` 1–2 per role** (role front matter), and **`agents.maxConcurrent` 2–3** for
  the project. More parallelism mostly buys conflicts and a machine divided into slices too thin
  to run a game's tests. `CIDE_CPUS` (below) is the machine's cores divided by this number.
- **The expensive model on the feature builder, cheap ones on mechanical work.** The review is
  batched and no longer re-runs checks, so the reviewer can be a mid-tier model too.

### `.cide/config.json`

```json
{
  "agents": {
    "enabled": true,
    "maxConcurrent": 3,
    "review": "batch",
    "reviewBatch": 3,
    "reviewAfterSecs": 600,
    "commentLimit": 1500,
    "isolateEnv": ["XDG_DATA_HOME", "XDG_CACHE_HOME", "TMPDIR"],
    "isolateEnvShare": ["godot/export_templates"]
  },
  "milestones": {
    "verify": "tools/ci/verify.sh",
    "items": [
      { "id": "combat", "title": "Combat loop playable", "gate": "tools/ci/gate.sh combat" },
      { "id": "look", "title": "First visual pass", "gate": "tools/ci/gate.sh look" }
    ]
  }
}
```

- **`review: "batch"`** is the default. One reviewer handles several finished tasks and merges
  them with one verify on the combined result. See [Batch reviews](#batch-reviews).
- **`commentLimit`** caps what a run, a worker or a tab cide opened may write in one comment.
  The default is 1500 characters, and 0 turns the cap off. The console is you and is never
  capped.
- **`isolateEnv`** gives each worktree its own XDG data, cache and temp directories. It is
  required if two runs can start the game or its tests at once. See
  [Parallel game instances](#parallel-game-instances).
- **A fast `verify`, under a minute.** Verify runs on every finished branch and again on every
  batch's combined head. A fifteen-minute verify makes every task cost fifteen minutes of wall
  clock and a slot. Keep verify to the build plus fast tests, and leave the slow playthrough to the
  gate.
- **Gates are build + fast tests + a boot smoke** (the game starts, loads the main scene, and
  exits clean). A gate says the milestone's work did not break the game. It is not where quality
  is measured. The gates on terrastrike and selfcraft that tried to measure quality (counts of
  items, frame-time budgets, screenshot diffs) were met by work that satisfied the number and
  not the player. Taste is judged by you, through `acceptance: user`.
- Prompts you leave at cide's defaults are **not written into the file**, so later versions of
  cide's prompts reach your project. Settings → Agents has **Reset to cide's prompt** for each
  one you edited. The review mode, batch size, review-after, comment limit and batch review
  prompt are there too.

### Milestones: concrete, and one of them visual

A milestone is a player-visible result with a plan you can read in five minutes. "Combat loop
playable: melee, one ranged enemy, death and respawn" is a milestone. "Improve the game" is not,
because a planner cannot tell when it is done, and so it never stops planning.

Keep **one visual milestone** separate from the feature milestones. Feature work uses
placeholders. The art, the UI look and the feel are the visual milestone's tasks, each with
`acceptance: user`. See [Visual work](#visual-work).

## Planning a milestone with the console

The plan lives in **the milestone's goal task body**, and the planner reads nothing else to
decide what to create. Write it with the console (your own Claude pane) rather than by hand:
describe the milestone, let it draft, correct it, and approve.

A good plan lists **tasks**, and each task is one **coherent thread** with ordered steps in its
body. "Ranged enemy: AI state machine, projectile, hit reaction, spawn in test arena" is one
task, even with four steps. Before M132 that would have been four tasks, each paying for its own
worktree, verify and reviewer, and each seeing a quarter of the context. Each task also carries:

- **`touches`**: the files and directories it will change (`game/enemies/`,
  `game/combat/projectile.gd`, `art/enemies/**`). cide never runs two tasks whose touches
  overlap. **A task with no touches is read as the whole repository and runs alone**, so write
  them. Precise touches are what let several runs work at once.
- **`acceptance: "user"`** when its quality is a matter of taste: art, the look of a screen, the
  feel of a control. cide merges such a task when its verify is green, and it waits for you.
- **`subtaskOf`** the goal task.

Ask the console to show you the whole plan and the tasks before it assigns anything. Approving
the plan is the moment you decide the milestone's contents. After that, the roles and the
planner work inside it.

The task card has Touches, Acceptance and Question rows. The New task dialog does not set them
yet, so set them on the card or through the console.

## Running

### What the planner does and never does

The planner is a Claude tab cide opens by itself when the project goes quiet (`agents.autoSpin`).
It is one short turn:

- It reads the active milestone's goal task (the plan) and the facts cide appends. Those facts
  name the milestone and its gate, plus **a delta**: which tasks changed status and which commits
  landed since the last plan. It does not read the whole board and does not run checks.
- It creates what **the plan names and the board lacks**, with steps, `touches`, `acceptance`
  and `subtaskOf`, and assigns a few at a time.
- It brings the plan in the goal body up to date: what is done, what is next.
- **It never touches the inbox.** It does not move a task out, and it does not link an inbox task
  into a milestone. The tracker refuses a planner that tries.
- **Tasks in review are not its concern.** The batch reviewer has them.
- **When the plan is done but the gate still fails, or the next step needs a decision about
  taste, scope or design, it stops.** It asks with a `question` on the milestone's task and ends
  its turn. You find the question in *Waiting for you*.

The planner timer does not wake it again while nothing changed. A latch compares the branch's
HEAD and every non-goal task's (id, status, has-a-question) against the last plan. For the timer,
open work is todo and doing tasks without a question. Work in review or waiting on you does not
count as a reason to plan. Answering the active milestone's question releases the latch and
restarts the quiet dwell. The next plan receives the answer in its changes since the last plan;
the planner's own goal edits still do not cause repeated planning turns.

An idle console that has not reported a hook does not hold off the timer unless its current
child was given an opening prompt. This matters for a restored Codex console: it can wait at
its composer without sending `SessionStart`. A newly opened planner or reviewer does hold off
planning while its opening prompt starts. The log's `planner eligibility changed` entries name
the current reason for waiting and the relevant runs and console states.

Closing a task also submits its assigned dependents to the queue once every live blocker is
Done, when the user or an orchestrator made that completion. Both the completed blocker and
dependent must belong to the same active milestone. Review still blocks dependencies, and
tasks with a question wait for the answer. This does not plan new work or require the idle
timer; the normal pause, concurrency and duplicate-run checks still apply. Link removal,
deletion, restart and milestone activation do not sweep for ready tasks, so previously stranded
work needs reassignment or an explicit dispatch once.

### The queue: "waiting: t-N holds …"

A queued run whose task's `touches` overlap what another task holds waits. A task holds its
declared touches plus the files its branch or checkout has actually changed. It keeps holding them
while a run is on it and while its branch has unmerged work. The run row says why, for example
`waiting: t-41 holds game/enemies/`. Admission moves past a path-blocked entry to the role's next
queued run, so one blocked task does not stall a role's other work.

Overlap is decided by path prefix and is deliberately conservative: `art/**/*.png` and
`art/**/*.json` count as overlapping. Waiting wrongly costs a few minutes, while starting wrongly
costs a conflict, which in this loop is nearly a whole task.

### Batch reviews

When a run finishes a turn on a task, verify runs on its branch when configured. Red goes back to the run itself
(up to `agents.verifyRetries`). Green joins the pending batch. Missing or blank verify satisfies
the gate immediately, without running checks or consuming retries. A review tab opens in the project
root when:

- `reviewBatch` (3) tasks are waiting, or
- the oldest has waited `reviewAfterSecs` (600), or
- nothing in the project is working any more, so the last task of a burst is not held for
  company that is not coming.

A new batch never opens while the previous batch's reviewer is mid-turn, because two reviewers
merging into one branch is the race batching exists to avoid.

The reviewer reads each task's report and diff and judges whether it did what the task asked.
**It does not re-run checks**: it uses the verify result when configured, and proceeds without
checks when verify is absent. It merges every task it accepts in
**one** `cide_agent_integrate` call with `tasks: [{task, agent}, …]`:

1. cide composes the merges **in memory**, moving no ref and touching no file. A branch that
   conflicts with the chain so far is skipped and named, and the rest still go.
2. When configured, cide verifies the combined head **once**, in a scratch checkout
   (`.cide/worktrees/batch-verify`).
3. If verification passes or is not configured, cide advances your branch to it, with a safe checkout. The move is
   refused if the branch moved in the meantime.
4. If the combined verify is red, nothing moves. cide then merges the branches one by one, each
   on its own verify, so the good ones still land and the answer names which road each task
   took.

Tasks with `acceptance: user` never go to the reviewer at all (see below). Set `review: "each"`
to go back to one reviewer per finished task.

An idle agent remains alive while a pane shows its conversation. Once its task is done,
closing the last view retires its process and moves the run to History. A completed idle
conversation does not block automatic planning, even while its view remains open.

## Waiting for you

The Tasks panel's **Waiting** tab ("Waiting for you") lists everything cide will not move until
you do:

- **Work you accept.** A task with `acceptance: user` whose branch went green: cide merged it
  itself, with no reviewer session, and commented `Merged at <sha>. Waiting for the user's
  acceptance`. It sits in review. Run the game, since the work is already on your branch, and
  then:
  - **Accept** sets it done. Only you can: the MCP tools refuse a reviewer, a run, a worker or a
    tab cide opened that tries to set it done. Your own console may, because that is you typing.
  - **Send back** takes a one-line note. The same role continues **in its own conversation** on
    the task, so the run that made the sprite reads why it was sent back with the context that
    made it.
  - If the merge conflicts, the task goes back to the run with the paths and never reaches this
    list until it merges.
- **Questions.** A task with an open `question`, such as the planner's "the plan is done but the
  gate fails: X or Y?", or a run's "which of these two layouts?". A task with an open question is
  not dispatched (autodispatch and an explicit dispatch both refuse), is not counted as open work
  by the planner timer, and is skipped by batch review. Choose from the supplied answers or
  enter custom text, then press **Answer**. A question takes priority over approval: the task
  appears only under Questions. Answering clears the question, returns review work to doing
  (todo without an assigned role), and continues the same role. Approval can be requested
  later when the work is finished.

Questions can include any number of choices with a title, optional description, and optional
image. The agent specifies single-select or multi-select; custom text is always available,
alone or alongside selections. **Compare choices** opens a wider view; **View image** shows a
render full size without losing your selections. A failed submission keeps your answer for retry.

Agents ask through `cide_task_update`. A plain string still asks a free-text question; a structured
question supplies choices:

```json
{
  "id": "t-17",
  "question": {
    "text": "Which renders do you prefer?",
    "selection": "multiple",
    "options": [
      { "id": "wood", "title": "Wooden finish", "description": "Warm lighting", "image": "renders/wood.png" },
      { "id": "metal", "title": "Metal finish", "image": "renders/metal.png" }
    ]
  }
}
```

Selection defaults to `single`. Option IDs must be unique and titles nonempty. Image paths are
relative to the calling agent's working directory (absolute paths also work); cide copies them
into the task's attachments. An existing image attachment ID can be reused. Files use the normal
32 MiB attachment limit; there is no limit on the number of choices. Ask a question or request
approval in a hand-back, never both.

Before M132 both of these were comments in a log. A run would ask "which of these two sprites?"
in a card nobody had open, and the project sat idle behind it with nothing on screen saying why.

## The inbox

The inbox is where **noticed** work waits for you. It is outside every milestone.

- Roles, reviewers and the planner file what they notice there (`cide_task_create`, which puts a
  run's task in the inbox anyway). They do not fix it inside the task they are on, because that
  makes a branch nobody can review.
- **An inbox task is never inside a milestone goal's `subtaskOf` tree.** Linking one in is
  refused, in the MCP tools and in the UI alike. Creating an inbox task whose parent is inside a
  milestone stores the link as `related` instead. Moving a milestone task to the inbox detaches it
  in the same way (`subtaskOf` becomes `related`), so the connection stays on record.
- **Only you move a task out of the inbox**: from the UI, from the phone, or from your own
  console. Tabs cide opened (the planner and reviewers), workers and runs are refused.
  Telling the console "take t-123 into the milestone", or asking it to fix something that is
  already in the inbox, **is** you promoting it. The console moves it, links it under the goal,
  and the roles pick it up.
- **Triage weekly.** Promote what matters, close what does not, and leave the rest. An inbox of a
  hundred rows holds nothing up: it is not work anywhere, and it never keeps a milestone open.

Boards from before M132 may have inbox rows still linked under a goal. From M99 to M132 such a row
was a "decision owed" that held the milestone open and kept the planner planning. Now it is simply
*not part of the milestone*. The Milestones tab's **Detach inbox** button turns those links into
`related`, and so does `cide_milestones` with `action: "detachInbox"` from the console (the console
only).

## Visual work

Models are bad at taste, and on both games the user rejected the art and UI they made. So:

- **Feature work uses placeholders.** A feature task builds the mechanic with a coloured box, a
  stock sound and a debug font. Its reviewer judges the mechanic, not the look.
- **The real art is a task in the visual milestone, with `acceptance: "user"`.** It still goes
  through verify (it must build and boot). cide then merges it and it waits for you.
- **You look in the running game**, not at a PNG in a card: the scene, the scale, the palette
  next to everything else. Accept, or send back with one line ("too saturated next to the
  terrain", "the outline is lost at 1×"). The same role continues.
- A task sent back three times for the same reason is a brief problem. Rewrite the task (see
  [Troubleshooting](#troubleshooting)) rather than sending it back a fourth time.

## Parallel game instances

Two runs starting the game at once used to collide on the host: a shared `user://`, a shared
display, a test script that took half the machine each. Projects added global host locks to cope,
and the locks serialised the runs cide was trying to parallelise. With M132 none of that is
needed:

- **`agents.isolateEnv`** (`XDG_DATA_HOME`, `XDG_CACHE_HOME`, `TMPDIR`) gives each worktree its
  own directories. The same directories apply to the run, the verify of its branch, and any pane
  opened in the checkout. Godot's `user://` is `$XDG_DATA_HOME/godot/app_userdata/<project>`, so
  it lands under the isolated directory automatically. Share installed export templates with
  `isolateEnvShare: ["godot/export_templates"]`.
- **`CIDE_CPUS`** is exported to every run in a worktree, its verify, and sessions in a checkout.
  It is this machine's cores divided by `agents.maxConcurrent`, at least 1. Use it for `--jobs`
  and shard counts instead of `nproc`.
- **A private X display per instance**, with `Xvfb -displayfd`. The server picks a free display
  number itself and writes it back, so two runs never race for `:99`.

[`worktree-isolation.md`](worktree-isolation.md#running-several-game-instances-at-once) has the
script pattern. Once all three are in place, **remove global host locks**, such as a `flock` on
`/tmp/game.lock` around the test script. They are what kept a busy board at one game at a time.

## Reports and cost

A run reports **once per turn**. Its preamble tells it not to comment a plan, progress, logs or
measurements while it works; it commits each coherent step instead, because the branch's commits
are the record if the run dies. When it stops it leaves **one** report of about ten lines: the
result, what changed (paths), how it verified (the command and pass or fail, no pasted output),
what is left, and anything noticed.

- A second comment from the same run in the same turn marks its earlier one **superseded**. The
  log stays append-only: nothing is rewritten, the card collapses superseded comments, and an
  agent's `cide_task_get` hides them and says how many it hid.
- A comment over `commentLimit` from anything but your console is refused with the report recipe,
  so the next call is shorter rather than absent.
- A run's brief is not rewritten under it: MCP callers cannot change the body or title of a task
  with a live run. To correct a running task, comment the correction or re-dispatch.

Where the cost goes after M132: one executor session per coherent task, one verify per branch,
one reviewer and one verify per batch, and no reviewer for work you accept yourself. If a day's
spend is dominated by reviewer sessions, check that `review` is `batch` and that verify is fast
enough for batches to form.

## Troubleshooting

- **The queue is stuck on paths.** Look at the run rows' notes. `this task declares no
  touches, so it runs alone` or `it declares no touches, so nothing runs beside it`: give the
  tasks touches. `waiting: t-N holds <path>` where t-N is in review: its branch still holds those
  files until it merges, so review or merge it. Where t-N is abandoned: close it, or let the run
  finish. Touches that are too wide (`game/`) serialise everything, so narrow them to the
  directories the task really changes.
- **The same task is sent back again and again.** The brief is wrong or the taste is unwritten.
  Answer it properly: rewrite the body with the console (when no run is live on it), attach a
  reference image, or split the "look" part into its own `acceptance: user` task.
- **A red branch after a batch.** A batch cannot move your branch to a red head: the combined
  verify gates the move, and a red one moves nothing. If the combination was red, the per-branch
  fallback merges each branch on its own verify. That is the same guarantee `review: each` gives,
  so two branches green alone and red together can still both land on that road. The batch
  answer says when it fell back, and the next verify or gate is where it shows. Fix it
  as a task.
- **The planner stopped.** It asked a question. Look in *Waiting for you*. Answer it, or update
  the plan in the goal body and press **Plan tasks**. Answering the active goal's question
  releases the latch; the timer wakes the planner after the project has been quiet for its
  configured dwell. Other automatic wakes require the board or HEAD to move.
- **Tasks sit in review after a restart.** The batch pending set is in memory. After a restart
  the tasks stay in review until something announces them again. Ask the console to review
  them (it has `cide_agent_integrate` with `tasks` too), or dispatch the role again.
- **Going back.** `review: "each"` restores one reviewer per finished task, and `commentLimit: 0`
  removes the cap. Clearing `touches` everywhere makes every task run alone, which is the safe
  default for a board from before M132.

## Reference

### `.cide/config.json`, `agents`

| key | default | what it does |
| --- | --- | --- |
| `review` | `"batch"` | `batch`: one reviewer per batch, one combined verify. `each`: one reviewer per finished task (before M132) |
| `reviewBatch` | `3` | how many finished tasks open a batch review |
| `reviewAfterSecs` | `600` | the oldest finished task waits at most this long for company |
| `batchReviewPrompt` | cide's | the batch reviewer's prompt, with a `{tasks}` placeholder. Written only when changed |
| `commentLimit` | `1500` | the most characters one comment may carry from a run, a worker or a tab cide opened. `0` is off |
| `maxConcurrent` | `2` | the project's concurrent runs, and the divisor of `CIDE_CPUS` |
| `isolateEnv`, `isolateEnvShare` | off | per-worktree XDG and temp directories ([`worktree-isolation.md`](worktree-isolation.md)) |
| `autoSpinPrompt`, `reviewPrompt` | cide's | written only when changed. Settings → Agents → *Reset to cide's prompt* |

### Task fields

| field | set by | what it does |
| --- | --- | --- |
| `touches` | the plan (console, planner) | file globs the task will change. Overlapping tasks never run together. Empty is the whole repository |
| `acceptance: "user"` | the plan (console, planner). Never a run or a worker | merged when green, with no reviewer, and set done only by you |
| `question` | the planner, a run | holds the task (no dispatch, no batch review, not open work) until you answer it |

### Environment

| variable | where | value |
| --- | --- | --- |
| `CIDE_CPUS` | runs in a worktree, their verify, sessions in a checkout | cores ÷ `agents.maxConcurrent`, at least 1 |

A tracker that uses `touches`, `acceptance` or a text `question` is written as schema 4. Structured
questions require schema 5 while present. An older cide
refuses that board rather than silently dropping those fields when it writes the file back. The
same happened with schema 3 for the inbox (M83). Open such a project with a build from M132 on.
