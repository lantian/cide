/**
 * The New project wizard's decisions, checked without a DOM. (M97)
 *
 * `src/chrome/newProject/wizardModel.ts` is import-free so the TypeScript in `node_modules` can
 * compile it standalone — `check-menu-model.mjs`'s harness. What is proved here is what the wizard
 * *decides*: which steps each road walks, when Continue is allowed, that subagents force `git
 * init` (enabling them on a directory that is not a repository is refused by Rust, so a wizard
 * that let the box be unticked would promise a checklist line it knew would fail), that the
 * request carries only what its road asked for, and that the checklist lists the steps in the
 * order `cmd::new_project::project_new` runs them.
 *
 * Plus the two entry points the request named, read out of the sources with comments stripped
 * first — house-style comments name the thing they are about, so an unstripped grep would find
 * the needle in the prose: `project.new` is a registry command, and `dispatch.ts` handles it.
 *
 * What this does NOT cover: that the card paints, that the picker opens, or that the console
 * acts on its brief. Those need a display and a `claude`, and `docs/journal.md` says which of
 * them were seen.
 *
 * Run: `pnpm --dir ui run check:new-project`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-new-project-'))
let failed = 0

const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}
const ok = (cond, what) => {
  if (!cond) {
    console.error(`FAIL ${what}`)
    failed++
  }
}

/** Block and line comments out, strings left alone — good enough for the two needles below. */
const uncomment = (text) => text.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '')

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/chrome/newProject/wizardModel.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      '--noUncheckedIndexedAccess',
      '--exactOptionalPropertyTypes',
    ],
    { stdio: 'inherit' },
  )

  const m = await import(`file://${join(out, 'wizardModel.js')}`)
  const form = (over) => ({ ...m.EMPTY_FORM, ...over })
  const probe = (over) => ({
    path: '/p',
    exists: false,
    entries: 0,
    hasGit: false,
    hasOpenspec: false,
    hasCide: false,
    openAlready: false,
    ...over,
  })

  // --- 1. the roads -------------------------------------------------------------------------
  eq(m.steps(null), ['kind', 'location', 'create'], 'before a choice, the shortest road')
  eq(m.steps('empty'), ['kind', 'location', 'create'], 'Empty: type, place, create')
  eq(
    m.steps('spec'),
    ['kind', 'location', 'openspec', 'agents', 'create'],
    'OpenSpec: OpenSpec, then optionally subagents',
  )
  eq(
    m.steps('tasks'),
    ['kind', 'location', 'agents', 'brief', 'create'],
    'Tasks: subagents, then the brief the console plans from',
  )
  for (const kind of ['empty', 'spec', 'tasks']) {
    for (const id of m.steps(kind)) ok(typeof m.STEP_LABEL[id] === 'string', `${id} has a label`)
  }

  // --- 2. when Continue is allowed ----------------------------------------------------------
  ok(m.blocked('kind', form({}), null, '') !== null, 'no type chosen, no Continue')
  eq(m.blocked('kind', form({ kind: 'empty' }), null, ''), null, 'a type chosen, Continue')
  ok(m.blocked('location', form({ kind: 'empty' }), null, '') !== null, 'no path, no Continue')
  ok(
    m.blocked('location', form({ kind: 'empty', path: '/p/ab' }), probe({}), '/p/a') !== null,
    'a probe for another path decides nothing — a slow answer for ~/a cannot bless ~/ab',
  )
  eq(
    m.blocked('location', form({ kind: 'empty', path: ' /p ' }), probe({}), '/p'),
    null,
    'a current probe with no problem allows Continue (the path is compared trimmed)',
  )
  eq(
    m.blocked('location', form({ kind: 'empty', path: '/f' }), probe({ problem: 'a file' }), '/f'),
    'a file',
    'a probe problem is the reason shown',
  )

  // --- 3. subagents force git ---------------------------------------------------------------
  eq(m.gitInit(form({ kind: 'empty', gitInit: false })), false, 'Empty may skip git')
  eq(m.gitInit(form({ kind: 'tasks', gitInit: false })), true, 'Tasks always has git')
  eq(m.gitInit(form({ kind: 'spec', gitInit: false, agents: true })), true, 'Spec + agents: git')
  eq(m.gitInit(form({ kind: 'spec', gitInit: false, agents: false })), false, 'Spec alone may skip')
  eq(m.agentsOn(form({ kind: 'empty', agents: true })), false, 'Empty never enables subagents')

  // --- 4. the request carries only its road's fields ----------------------------------------
  eq(
    m.request(form({ kind: 'empty', path: ' /p ', specContext: 'x', brief: 'y' })),
    { path: '/p', kind: 'empty', gitInit: true, agents: false },
    'Empty sends no context and no brief, whatever was typed on another road',
  )
  eq(
    m.request(form({ kind: 'spec', path: '/p', specContext: 'Rust', agents: true })),
    { path: '/p', kind: 'spec', gitInit: true, agents: true, specContext: 'Rust' },
    'Spec sends its context',
  )
  eq(
    m.request(form({ kind: 'tasks', path: '/p', brief: '   ' })),
    { path: '/p', kind: 'tasks', gitInit: true, agents: true },
    'a blank brief is absent — the console then asks',
  )
  eq(
    m.request(form({ kind: 'tasks', path: '/p', brief: 'a habit tracker' })).brief,
    'a habit tracker',
    'Tasks sends its brief',
  )

  // --- 5. the checklist, in project_new's order ---------------------------------------------
  eq(
    m.plannedSteps(form({ kind: 'empty', gitInit: false }), null),
    ['folder', 'open'],
    'Empty without git',
  )
  eq(
    m.plannedSteps(form({ kind: 'spec', agents: true }), probe({})),
    ['folder', 'git', 'openspec', 'agents', 'open'],
    'Spec with subagents',
  )
  eq(
    m.plannedSteps(form({ kind: 'spec' }), probe({ hasOpenspec: true })),
    ['folder', 'git', 'open'],
    'OpenSpec already there is not promised again',
  )
  eq(
    m.plannedSteps(form({ kind: 'tasks' }), probe({})),
    ['folder', 'git', 'agents', 'open', 'brief'],
    'Tasks briefs the console last, after the open',
  )
  for (const id of ['folder', 'git', 'openspec', 'agents', 'open', 'brief']) {
    ok(typeof m.CREATE_LABEL[id] === 'string', `${id} has a checklist label`)
  }

  // --- 6. the location verdict --------------------------------------------------------------
  eq(m.locationNote(probe({})).tone, 'ok', 'a missing folder is fine: it will be created')
  eq(m.locationNote(probe({ exists: true, entries: 3 })).tone, 'warn', 'a full folder warns')
  ok(
    m.locationNote(probe({ exists: true, entries: 1 })).text.includes('1 item)'),
    'and counts in the singular for one',
  )
  eq(m.locationNote(probe({ openAlready: true })).tone, 'info', 'an open project is said so')
  eq(m.KINDS.map((k) => k.kind), ['empty', 'spec', 'tasks'], 'the three types, in order')

  // --- 7. the two entry points ----------------------------------------------------------------
  const registry = uncomment(readFileSync('../crates/cide-core/src/commands.rs', 'utf8'))
  ok(
    /Command::new\("project\.new",\s*"New project…"/.test(registry),
    'project.new is a registry command, so the palette lists it',
  )
  const dispatch = uncomment(readFileSync('src/keys/dispatch.ts', 'utf8'))
  ok(
    /case 'project\.new':\s*return void requestNewProject\(\)/.test(dispatch),
    'dispatch.ts raises the wizard for project.new',
  )
  const app = uncomment(readFileSync('src/App.tsx', 'utf8'))
  eq(
    (app.match(/<NewProjectWizard \/>/g) ?? []).length,
    2,
    'the wizard is mounted in both window kinds, like PushDialog',
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`check:new-project — ${failed} failure(s)`)
  process.exit(1)
}
console.log('check:new-project — ok')
