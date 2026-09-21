/**
 * Checks the remote surface's frontend half — the Settings section that grants another machine
 * access to this one, and the generated protocol the phone is written against. (M72)
 *
 * # Comments are stripped before anything is searched, and here that is mandatory
 *
 * This project's house style is to name the failure a rule prevents, so the words this script
 * looks for are exactly the words the code's own documentation contains. `RemoteSection.tsx`
 * explains at length why no token is ever rendered — using the word "token" five times. A check
 * that grepped the raw source would pass on its own explanation, which has already happened
 * three times in this repository (`check:diff-render` on a package name in prose, `check:docker`
 * twice in one sitting).
 *
 * # What is pinned, and what each failure would look like unpinned
 *
 *  * **No credential reaches a screen.** `RemoteDevice` has no field that could carry one, so
 *    this is a check on the type staying that way and on nothing under `settings/` reaching for
 *    a name that would only exist on a secret.
 *  * **The grant is stated where it is given.** The switch hands another machine the ability to
 *    read and type into every session on this one. A toggle whose label said only "remote
 *    access" would be a permission dialog that does not say what it permits.
 *  * **Every frame the phone can send is accounted for.** `contract/protocol.ts` is generated
 *    from Rust, so a new `ClientBody` variant appears there silently; this script holds the list
 *    and fails until somebody decides whether the app should send it.
 *  * **The status readout branches on all three arms.** `Refused` rendered as an empty address
 *    list reads as "starting…", which is the one wrong answer that costs somebody an evening.
 *
 * Run: `pnpm --dir ui run check:remote`
 */
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

const here = dirname(fileURLToPath(import.meta.url))
const ui = join(here, '..')
const root = join(ui, '..')
let failed = 0

const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}
const ok = (actual, what) => eq(actual, true, what)

/**
 * Source with every comment removed.
 *
 * One character scanner rather than two regular expressions, because the two-pass version is
 * wrong in a way that looks right: a `/*` inside a string literal makes a lazy block-comment
 * pattern swallow everything up to the next real close, and what it takes is code. This file
 * found that on its own first attempt — the nav entry it was looking for had been eaten.
 */
const stripped = (path) => {
  const raw = readFileSync(path, 'utf8')
  let out = ''
  let quote = null
  let block = false
  for (let i = 0; i < raw.length; i++) {
    const c = raw[i]
    const next = raw[i + 1]
    if (block) {
      if (c === '*' && next === '/') {
        block = false
        i++
      }
      continue
    }
    if (quote) {
      out += c
      if (c === '\\') {
        out += next ?? ''
        i++
      } else if (c === quote) {
        quote = null
      }
      continue
    }
    if (c === '"' || c === "'" || c === '`') {
      quote = c
      out += c
      continue
    }
    if (c === '/' && next === '*') {
      block = true
      i++
      continue
    }
    if (c === '/' && next === '/') {
      while (i < raw.length && raw[i] !== '\n') i++
      out += '\n'
      continue
    }
    out += c
  }
  return out
}

const section = stripped(join(ui, 'src/settings/RemoteSection.tsx'))
const sections = stripped(join(ui, 'src/settings/sections.tsx'))
const client = stripped(join(ui, 'src/ipc/client.ts'))
const css = stripped(join(ui, 'src/settings/RemoteSection.module.css'))
const protocol = readFileSync(join(root, 'contract/protocol.ts'), 'utf8')
const generated = readFileSync(join(ui, 'src/ipc/generated.ts'), 'utf8')

// --- no credential reaches a screen -------------------------------------------------------

for (const secret of ['tokenHash', 'sealKey', 'apiKey']) {
  ok(!section.includes(secret), `the remote section never names \`${secret}\``)
}
// `token` and `key` on their own are common words, so the check is that neither the section nor
// the client namespace ever *reads a property* by those names.
ok(!/\.token\b/.test(section), 'the remote section never reads a `.token` off anything')
ok(!/\.key\b/.test(section), 'the remote section never reads a `.key` off anything')
const namespace = client.slice(
  client.indexOf('export const remote ='),
  client.indexOf('export const events ='),
)
for (const secret of ['token', 'sealKey', 'seal_key']) {
  ok(!namespace.includes(secret), `the remote client namespace never carries a ${secret}`)
}

// --- and the protocol itself carries no credential after pairing --------------------------
//
// The sealed transport's strongest property, and the one worth pinning: a device is identified
// in the handshake and authenticated by being able to seal a frame this cide can open, so the
// only moment a key crosses the wire is inside the channel that pairing established. `hello`
// carrying a token again would undo that without breaking anything visible.
const helloFields = protocol.match(/"t":\s*"hello"([^}]*)\}/)
ok(Boolean(helloFields), 'the protocol declares a hello frame')
if (helloFields) {
  for (const secret of ['token', 'key', 'secret']) {
    ok(!helloFields[1].includes(secret), `the hello frame carries no ${secret}`)
  }
}

// --- the grant is stated where it is given ------------------------------------------------

const grant = section.match(/const GRANT\s*=\s*([\s\S]*?)\n\n/)
ok(Boolean(grant), 'the section declares the sentence that states the grant')
const grantText = grant ? grant[1] : ''
for (const capability of ['sessions', 'permission prompts', 'dispatch', 'tasks']) {
  ok(grantText.includes(capability), `the grant sentence names ${capability}`)
}
ok(section.includes('hint={GRANT}'), 'the grant sentence is attached to the switch that grants it')

// --- the status readout branches on every arm ---------------------------------------------

// From `generated.ts` and not from `protocol.ts`: this type belongs to the *local* IPC — it is
// what the Settings screen asks this process — and the remote protocol correctly has no idea it
// exists. Reading it from the wrong file is how the first draft of this check failed.
const statusType = generated.match(/export type RemoteStatus = ([\s\S]*?);\n/)
ok(Boolean(statusType), 'generated.ts declares RemoteStatus')
const arms = statusType
  ? [...statusType[1].matchAll(/"state":\s*"([a-z]+)"/g)].map((m) => m[1])
  : []
eq(
  [...new Set(arms)].sort(),
  ['listening', 'off', 'refused'],
  'RemoteStatus still has exactly the three arms this section draws',
)
for (const arm of ['off', 'refused']) {
  ok(
    section.includes(`status.state === '${arm}'`),
    `the readout has a branch for a \`${arm}\` status`,
  )
}

// --- every frame the phone can send is accounted for --------------------------------------

/**
 * The client frames this app knows about.
 *
 * Generated from Rust, so a new one appears in `contract/protocol.ts` with nothing on this side
 * changing. Listing them here is what turns that into a decision somebody has to make.
 */
const KNOWN_CLIENT_FRAMES = [
  'acknowledge',
  'answerPrompt',
  'dispatch',
  'hello',
  'input',
  'pair',
  'paste',
  'ping',
  'runPause',
  'runResume',
  'runStop',
  'scroll',
  'scrollbackPage',
  'subscribe',
  'taskEdit',
  'taskGet',
  'taskNew',
  'unwatchScreen',
  'watchScreen',
]
const clientBody = protocol.match(/export type ClientBody = ([\s\S]*?);\n/)
ok(Boolean(clientBody), 'contract/protocol.ts declares ClientBody')
const frames = clientBody ? [...clientBody[1].matchAll(/"t":\s*"([a-zA-Z]+)"/g)].map((m) => m[1]) : []
eq([...new Set(frames)].sort(), KNOWN_CLIENT_FRAMES, 'every client frame is one this app knows about')

ok(/export const PROTOCOL_VERSION = \d+/.test(protocol), 'the protocol announces its version')
for (const forbidden of ['Workspace', 'Settings', 'LlmProvider', 'ProxySettings']) {
  ok(
    !protocol.includes(`export type ${forbidden} `),
    `the protocol cannot name \`${forbidden}\`, which carries credentials`,
  )
}

// --- the section is reachable -------------------------------------------------------------

ok(sections.includes("id: 'remote'"), 'the nav has a row for the remote section')
ok(sections.includes("case 'remote':"), 'the switch renders the remote section')

// --- no QR dependency crept in ------------------------------------------------------------
//
// The pairing code is typed, which is the documented fallback for a QR and costs no dependency
// on either side. If a QR arrives it should come from a Rust-generated matrix drawn as SVG, not
// from an npm package pulled into the window's bundle.
const pkg = JSON.parse(readFileSync(join(ui, 'package.json'), 'utf8'))
const deps = Object.keys({ ...pkg.dependencies, ...pkg.devDependencies })
eq(
  deps.filter((name) => /qr/i.test(name)),
  [],
  'no QR package is in the frontend bundle',
)
ok(section.includes('invite.grouped'), 'the pairing window shows the grouped, readable code')

// --- the QR is drawn, and drawn so it can actually be read --------------------------------
//
// Three properties, each of which produces a code that *looks* right and scans badly or not at
// all — which is the worst shape of failure available here, because the person holding the
// phone has no way to tell a bad symbol from a bad camera angle and will simply try again.
ok(section.includes('matrix.modules['), 'the QR is drawn from the Rust matrix')
ok(
  section.includes('crispEdges'),
  'the QR disables antialiasing — a greyed module edge is a threshold a scanner may read either way',
)
ok(
  /quiet\s*=\s*4/.test(section),
  'the QR carries the four-module quiet zone the specification asks for',
)
ok(
  section.includes("fill=\"#ffffff\"") && section.includes("fill=\"#000000\""),
  'the QR states both its colours rather than inheriting the theme — it must stay dark-on-light',
)
// The fallback is the reason `qr` is optional at all, so it is asserted rather than assumed:
// an encoder that refused must not be able to take the typed code down with it.
ok(
  section.includes('invite.qr ?'),
  'a pairing with no QR still offers the typed code',
)

// --- the short authentication string ---------------------------------------------------------
//
// The typed pairing road's only defence against somebody relaying the connection, and it is
// defeated by presentation alone: a number nobody can read, or one a person is trained to wave
// through, is the same as no number at all.
ok(section.includes('attempt.sas'), 'the panel draws the digits the connection derived')
ok(
  section.includes('attempt.addr'),
  'the digits are shown beside the address they belong to — "a device is pairing" must be disbelievable',
)
ok(
  /tabular-nums/.test(css),
  'the digits are tabular, or a proportional 1 makes two identical numbers look different',
)
// There is deliberately no button on this side. cide cannot tell whether the digits matched —
// only the person can, and the tap that withholds the code is on the device. A control here
// would change nothing, and the habit of pressing it is the habit that defeats the check.
ok(
  !/Confirm|Accept|It matches/i.test(section),
  'the panel offers no confirm button: the answer belongs to the device, which is what holds the code',
)

if (failed) {
  console.error(`\n${failed} failure(s)`)
  process.exit(1)
}
console.log(
  `remote: ok (${KNOWN_CLIENT_FRAMES.length} client frames, 3 status arms, no credential on any screen)`,
)
