/**
 * Checks `src/sidebar/treeDrag.ts` — what a file-tree drag picks up, where a drop lands, and
 * every refusal it has to say before the user lets go.
 *
 * > *"drag selected elements to some folder"*
 *
 * This is the most dangerous gesture in the panel: a drop **moves** files, and `cide_fs::ops`
 * has no undo — the trash is the closest thing and a move does not go through it. Every rule
 * below has a wrong version that type-checks and is invisible in a screenshot, and one of them
 * (a folder dropped into its own descendant) destroys a directory tree if it is merely
 * "unlikely" rather than refused. So the rules live in a pure module and this script runs them.
 *
 * The tail reads `FileTree.tsx`, `useTreeDrag.ts` and `fileClipboard.ts` as *source*, because
 * the wiring cannot be executed here: there is no DOM and no Tauri. Ugly, and it is the
 * difference between a claim that can fail and one that cannot — every rule above could pass
 * with the drag reaching no row, the ghost drawing nothing, and the drop calling nothing.
 *
 * Run: `pnpm --dir ui run check:tree-drag`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-tree-drag-'))
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

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/sidebar/treeDrag.ts',
      '--outDir', out,
      '--rootDir', 'src',
      /*
       * CommonJS and `node10`, exactly like `check-fs-clipboard.mjs` and for the same reason.
       * `treeDrag.ts` imports **values** from four modules beside it — and that is the point of
       * the feature rather than an inconvenience: the drop reuses `targetFor` (where *New File…*
       * and *Paste* land), `isInside` ("into itself"), `mutationRefusal`/`creationRefusal` (what
       * the menu greys) and `rowVerbs` (what a row kind answers). A drag with its own copies of
       * those is a second set of answers to questions the user asks two ways. The app compiles
       * under `bundler` resolution, where extensionless specifiers are normal; ESM output would
       * keep them extensionless and node would refuse to load it, so `require` resolves them the
       * old way.
       */
      '--module', 'commonjs',
      '--moduleResolution', 'node10',
      '--target', 'es2022',
      '--strict',
      // Both flags the app's own tsconfig sets. `noUncheckedIndexedAccess` is what makes
      // `moving[0]` a `string | undefined`, which is the difference between a ghost reading
      // `Move “main.rs”` and one reading `Move “undefined”`.
      '--noUncheckedIndexedAccess',
      '--exactOptionalPropertyTypes',
      '--lib', 'es2023',
      '--skipLibCheck',
    ],
    { stdio: 'inherit' },
  )

  const require = createRequire(import.meta.url)
  const d = require(join(out, 'sidebar', 'treeDrag.js'))

  // ---------------------------------------------------------------- the world

  const ROOT = '/home/u/work/cide'
  const ROOTS = [ROOT]
  /** The scratch drawer: outside every root, and still somewhere cide writes. */
  const DRAWER = '/home/u/.local/state/cide/scratches/4f2a9c7b'
  const WRITABLE = [ROOT, DRAWER]
  /** A dependency source under `External Libraries` — absolute, and outside everything. */
  const CRATE = '/home/u/.cargo/registry/src/index.crates.io-6f17d22bba15001f/serde-1.0.229'

  const dir = (path, extra = {}) => ({
    path,
    kind: 'dir',
    name: path.slice(path.lastIndexOf('/') + 1),
    expanded: false,
    hasChildren: true,
    ...extra,
  })
  const file = (path) => ({
    path,
    kind: 'file',
    name: path.slice(path.lastIndexOf('/') + 1),
    expanded: false,
    hasChildren: false,
  })
  const header = (name) => ({
    path: 'cide://group/externalLibraries',
    kind: 'group',
    name,
    expanded: true,
    hasChildren: true,
  })
  const note = (name) => ({
    path: 'cide://note/x',
    kind: 'note',
    name,
    expanded: false,
    hasChildren: false,
  })
  /**
   * *Project Notes* — a pinned row. It **opens**, which is what makes it worth asserting here:
   * it is the first synthetic row with a live gesture on it, and "openable" must not leak into
   * "draggable" or "a folder you can drop onto". Its path is the same non-absolute sentinel a
   * header carries, so both refusals fall out of rules that already existed.
   */
  const pin = (name) => ({
    path: 'cide://group/projectNotes',
    kind: 'pin',
    name,
    expanded: false,
    hasChildren: false,
  })

  const MAIN = `${ROOT}/src/main.rs`
  const LIB = `${ROOT}/src/lib.rs`
  const SIDEBAR = `${ROOT}/ui/src/sidebar`
  const SRC = `${ROOT}/src`

  const pick = (row, carried = []) => d.grab(new Set(carried), row, ROOTS, WRITABLE)
  const drop = (drag, target) => d.dropOutcome(drag, target, ROOTS, WRITABLE)

  // ---------------------------------------------------------------- what a grab carries

  eq(
    pick(file(MAIN))?.paths,
    [MAIN],
    'a press on an unselected row carries that row alone',
  )
  eq(
    pick(file(MAIN), [MAIN, LIB, SRC])?.paths,
    [MAIN, LIB, SRC],
    'a press on a row that IS in the selection carries the WHOLE selection — the request was '
      + '"drag selected elements", and a drag that quietly narrowed to one row is the feature '
      + 'not existing',
  )
  eq(
    pick(file(MAIN), [MAIN])?.paths,
    [MAIN],
    'a one-row selection carries that row and does not count as a widening',
  )
  eq(pick(file(MAIN), [MAIN, LIB]).widened, true, 'a widened grab says so, for the ghost')
  eq(pick(file(MAIN)).widened, false, 'and an unwidened one does not')
  eq(
    pick(file(LIB), [MAIN, SRC])?.paths,
    [LIB],
    'a press OUTSIDE the selection carries itself alone and changes nothing — the press that '
      + 'preceded it has already collapsed the selection',
  )

  eq(pick(header('External Libraries')), null, 'a group header is not a drag source at all')
  eq(pick(note('cargo is not on PATH')), null, 'and neither is a note')
  eq(
    pick(pin('Project Notes')),
    null,
    'a pin is openable and is still not a drag source: `grab` gates on `actionable`, so a '
      + 'Ctrl+X or a drag would otherwise carry a sentinel that names no file',
  )

  eq(pick(file(MAIN)).label, 'main.rs', 'the ghost calls one file by its name')
  eq(pick(dir(SRC)).label, 'src/', 'and one folder with the slash that says it is one')
  eq(pick(file(MAIN), [MAIN, LIB]).label, '2 items', 'and a widened load by its count')
  eq(d.plural(1), '1 item', 'one item is not "1 items"')
  eq(d.plural(4), '4 items', 'the vocabulary is the menu’s — Cut 3 Items, Move 3 items to Trash')

  // ---------------------------------------------------------------- the source refusals

  eq(
    pick(dir(ROOT)).refusal,
    'A project root is closed, not moved',
    'a project root is picked up and REFUSED with the sentence Cut already shows — not silently '
      + 'unpickable, which is indistinguishable from a broken drag',
  )
  ok(
    pick(file(`${CRATE}/src/lib.rs`)).refusal !== null,
    'a dependency source under External Libraries is refused: `check_within` would reject it, and '
      + 'moving a crate out of the registry breaks every project on the machine',
  )
  eq(
    pick(file(`${DRAWER}/scratch.rs`)).refusal,
    null,
    'a SCRATCH is draggable: it is in the writable set, which is the whole difference between the '
      + 'two groups, and dragging one into the project is the useful half of the drawer',
  )
  eq(
    pick(file(MAIN), [MAIN, ROOT]).refusal,
    'A project root is closed, not moved',
    'ONE root anywhere in the load refuses the whole load — a partial drag is a gesture whose '
      + 'result the user cannot predict from what is highlighted',
  )
  eq(
    drop(pick(dir(ROOT)), dir(SIDEBAR)).kind,
    'refuse',
    'and the refusal survives to the drop, over a folder that would otherwise accept it',
  )
  eq(
    drop(pick(dir(ROOT)), dir(SIDEBAR)).hint,
    'A project root is closed, not moved',
    'source refusals are reported before target refusals: one answer over every row',
  )

  // ---------------------------------------------------------------- where a drop lands

  const one = pick(file(MAIN))
  eq(
    drop(one, dir(SIDEBAR)).destDir,
    SIDEBAR,
    'a drop on a FOLDER lands in that folder',
  )
  eq(
    drop(one, file(`${SIDEBAR}/FileTree.tsx`)).destDir,
    SIDEBAR,
    'a drop on a FILE lands in its parent — the same rule New File… and Paste follow, because '
      + '"inside the thing I clicked" has no meaning for a file',
  )
  eq(
    drop(one, file(`${SIDEBAR}/FileTree.tsx`)).mark,
    SIDEBAR,
    'and the ring goes on the PARENT, not on the file under the pointer: ringing the file would '
      + 'claim the file was the target',
  )
  eq(
    drop(one, dir(SIDEBAR)).paths,
    [MAIN],
    'the move carries the paths that actually move',
  )
  eq(
    drop(one, dir(SIDEBAR)).hint,
    'Move “main.rs” to “sidebar”',
    'and the ghost says exactly what will happen, naming both ends',
  )
  eq(
    drop(pick(file(MAIN), [MAIN, LIB]), dir(SIDEBAR)).hint,
    'Move 2 items to “sidebar”',
    'a widened drag counts on the ghost from the first pixel of movement',
  )

  // ---------------------------------------------------------------- the target refusals

  eq(
    drop(one, null).kind,
    'refuse',
    'NOTHING under the pointer refuses. `targetFor(null)` answers the first root, which is right '
      + 'for New File… from empty space and would turn every drag abandoned over the editor into '
      + 'a move to the top of the project',
  )
  eq(drop(one, null).mark, null, 'and there is no row to ring')

  eq(drop(one, header('External Libraries')).kind, 'refuse', 'a group header refuses')
  eq(
    drop(one, header('External Libraries')).hint,
    '“External Libraries” is a heading, not a folder',
    'and names the heading, because its path is a cide:// sentinel that means nothing to a user',
  )
  eq(drop(one, note('cargo is not on PATH')).kind, 'refuse', 'a note refuses')
  eq(
    drop(one, pin('Project Notes')).kind,
    'refuse',
    'and so does a pin: its path is a sentinel, so `targetFor` would hand back the "parent" of '
      + 'a string that names nothing and Rust would refuse the move after the drop',
  )

  const intoCrate = drop(one, dir(`${CRATE}/src`))
  eq(intoCrate.kind, 'refuse', 'a dependency source directory refuses as a DESTINATION too')
  ok(
    intoCrate.hint.includes('read-only'),
    'and says so in the word that matters — those files are outside every root and `check_dest` '
      + 'would reject the command',
  )
  eq(
    drop(one, file(`${CRATE}/src/lib.rs`)).kind,
    'refuse',
    'and dropping on a FILE inside one is the same refusal, since it resolves to that directory',
  )

  const intoDrawer = drop(one, dir(DRAWER))
  eq(
    intoDrawer.kind,
    'refuse',
    'the SCRATCH drawer refuses a drop although Rust would accept it: New File in… and Paste are '
      + 'both off inside it, and two gestures answering one question differently is how a user '
      + 'learns the app is guessing',
  )
  ok(
    !intoDrawer.hint.includes('read-only'),
    'and it is refused as the drawer, not as an out-of-project path — the containment test runs '
      + 'first so only the drawer reaches `creationRefusal`',
  )

  // ---------------------------------------------------------------- into itself

  const folder = pick(dir(SRC))
  eq(
    drop(folder, dir(SRC)).kind,
    'refuse',
    'a folder dropped on ITSELF is refused',
  )
  eq(
    drop(folder, dir(SRC)).hint,
    '“src” cannot be moved into itself',
    'in the same words the Paste item uses for the same mistake',
  )
  eq(
    drop(folder, dir(`${SRC}/cmd/inner`)).kind,
    'refuse',
    'a folder dropped into its own DESCENDANT is refused — THE one that destroys a tree if it is '
      + 'merely unlikely rather than refused',
  )
  eq(
    drop(folder, file(`${SRC}/cmd/fs.rs`)).kind,
    'refuse',
    'including through a file inside it, which resolves to a directory inside it',
  )
  eq(
    drop(folder, dir(`${ROOT}/srcx`)).kind,
    'move',
    'but a SIBLING whose name merely starts with the same letters is a perfectly good target — '
      + 'the string version of the containment test refuses this one',
  )
  eq(
    drop(pick(dir(SRC), [SRC, MAIN]), dir(`${SRC}/cmd`)).kind,
    'refuse',
    'one folder in a multi-row load is enough: the whole drop is refused, not narrowed',
  )

  // ---------------------------------------------------------------- already there

  eq(
    drop(one, dir(SRC)).kind,
    'noop',
    'dropping a file back in the folder it is already in does nothing',
  )
  eq(
    drop(one, dir(SRC)).hint,
    'Already in “src”',
    'and says so — Rust treats it as a no-op too, but a silent success reads as a broken drag',
  )
  eq(
    drop(one, file(LIB)).kind,
    'noop',
    'and the same through a sibling file, which resolves to the same folder',
  )
  const mixed = pick(file(MAIN), [MAIN, `${SIDEBAR}/x.ts`])
  eq(
    drop(mixed, dir(SRC)).paths,
    [`${SIDEBAR}/x.ts`],
    'a load where some rows are already there sends only the ones that move',
  )
  eq(
    drop(mixed, dir(SRC)).hint,
    'Move “x.ts” to “src”',
    'and the ghost counts what moves, not what was picked up',
  )

  // ---------------------------------------------------------------- across roots

  const TWO = [ROOT, '/home/u/work/other']
  eq(
    d.dropOutcome(d.grab(new Set(), file(MAIN), TWO, [...TWO, DRAWER]), dir('/home/u/work/other'), TWO, [
      ...TWO,
      DRAWER,
    ]).kind,
    'move',
    'a drop across two roots of one project is allowed — both sides pass `check_within`',
  )

  // ---------------------------------------------------------------- what is dimmed and tinted

  const flight = new Set([SRC])
  ok(d.inDrag(flight, `${SRC}/main.rs`), 'dragging a folder puts every row under it in flight')
  ok(d.inDrag(flight, `${SRC}/cmd/deep/x.rs`), 'however deep')
  ok(d.inDrag(flight, SRC), 'including the folder itself')
  ok(!d.inDrag(flight, `${ROOT}/srcx/a.rs`), 'and not a sibling with a shared prefix')
  ok(
    !d.inDrag(flight, ROOT) && !d.inDrag(flight, '/'),
    'and never an ANCESTOR of what is being dragged — the ancestor walk must stop at `/` rather '
      + 'than asking the set about the empty string',
  )
  ok(d.inDropBand(`${SIDEBAR}/a.ts`, SIDEBAR), 'a row inside the destination takes the band')
  ok(
    !d.inDropBand(SIDEBAR, SIDEBAR),
    'the destination itself does not — it has the ring, and both would be two claims',
  )
  ok(!d.inDropBand(`${ROOT}/ui/src/sidebarx/a.ts`, SIDEBAR), 'component-wise here too')

  // ---------------------------------------------------------------- spring-loaded folders

  const accepted = drop(one, dir(SIDEBAR))
  eq(
    d.springTarget(dir(SIDEBAR), accepted),
    SIDEBAR,
    'resting over a closed folder unfolds it — you cannot drop into a folder you cannot see',
  )
  eq(
    d.springTarget(dir(SIDEBAR, { expanded: true }), accepted),
    null,
    'an open one is left alone',
  )
  eq(
    d.springTarget(dir(SIDEBAR, { hasChildren: false }), accepted),
    null,
    'and an empty one, whose expansion would move nothing on screen',
  )
  eq(d.springTarget(file(MAIN), accepted), null, 'a file has nothing to unfold')
  eq(
    d.springTarget(header('External Libraries'), drop(one, header('External Libraries'))),
    null,
    'and a GROUP HEADER is never sprung: expanding External Libraries is what launches '
      + '`cargo metadata`, and resting the pointer over a heading the drop can never land on '
      + 'must not spawn a toolchain',
  )
  eq(
    d.springTarget(dir(`${CRATE}/src`), intoCrate),
    null,
    'nor anything under a refused drop',
  )

  // ---------------------------------------------------------------- the wiring, as source

  const tree = readFileSync('src/sidebar/FileTree.tsx', 'utf8')
  const hook = readFileSync('src/sidebar/useTreeDrag.ts', 'utf8')
  const clipboard = readFileSync('src/sidebar/fileClipboard.ts', 'utf8')
  const css = readFileSync('src/sidebar/FileTree.module.css', 'utf8')

  /*
   * The rules above are dead if nothing calls them. This project's signature defect is a feature
   * that is complete, correct and reachable from nothing — fifteen cases so far — and a drag is
   * exactly the shape that hides it, because no check in this repo can move a pointer.
   */
  ok(
    /const drag = useTreeDrag\(\{/.test(tree) && /onMove: runMove,/.test(tree),
    'the panel builds the drag AND passes `onMove` — without it the hook refuses to start a '
      + 'gesture, which is deliberate: a drag that lands nowhere is the silent failure dressed '
      + 'as the feature',
  )
  ok(
    /onPointerDown=\{\(e\) => \{[\s\S]{0,120}?onPick\(e, row\)/.test(tree)
      && /onPick=\{drag\.onPointerDown\}/.test(tree),
    'and every row carries the press that can become one',
  )
  ok(
    /onPointerDown=\{\(e\) => e\.stopPropagation\(\)\}/.test(tree),
    'the twisty stops the press: it is a control, and a hand that shifts three pixels while '
      + 'folding a folder must not pick the row up instead',
  )
  ok(
    /'data-drag': ''/.test(tree) && /'data-drop': drop/.test(tree)
      && /'data-drop-band': dropBand/.test(tree),
    'the row draws all three feedback channels — without `data-drag` a drag of four files looks '
      + 'exactly like a drag of one',
  )
  /*
   * And the set it is drawn from holds the load. The first version of this stopped at the three
   * attributes above and passed with the memo building `new Set<string>()` — every row correctly
   * asked, and every answer `false`, which is the same screen as no `data-drag` at all.
   */
  ok(
    /new Set\(drag\.state\.drag\.paths\)/.test(tree)
      && /inFlight !== null && inDrag\(inFlight, row\.path\)/.test(tree),
    'and the dim is asked against the paths actually in flight, built once per drag rather than '
      + 'rescanned per row per frame',
  )
  ok(
    /\[data-drag\]/.test(css) && /\[data-drop='move'\]/.test(css)
      && /\[data-drop='refuse'\]/.test(css) && /\[data-drop-band='move'\]/.test(css),
    'and the stylesheet actually draws them',
  )
  ok(
    /\.scroll\[data-dragging\] \.row:not\(\.rowSelected\):hover/.test(css),
    'the hover wash is suppressed during a drag WITHOUT out-specifying the selection band — the '
      + 'naive version erases the very set the drag is carrying',
  )
  ok(
    /drag\.state !== null && <TreeDragGhost state=\{drag\.state\} \/>/.test(tree)
      && /state\.outcome\.hint/.test(tree),
    'the ghost is mounted and draws the verdict — the refusal is a sentence, and a colour alone '
      + 'says "no" to everyone who can see it and nothing to everyone who cannot',
  )
  ok(
    /'data-dragging': ''/.test(tree),
    'and the scroller says a drag is in flight, which is what the CSS above keys on',
  )

  /*
   * The drop must go through the SAME confirmation the paste does, and must not touch either
   * clipboard on the way. `take()` writes the in-app clip *and* the system clipboard, so a drop
   * routed through the clipboard store would silently discard whatever the user had copied.
   */
  ok(
    /const runMove = useCallback\(\s*\(sources: readonly string\[\], destDir: string\) => \{[\s\S]{0,400}?startTransfer\(\{ sources, destDir, mode: 'cut', fromClip: false \}\)/.test(
      tree,
    ),
    'a drop is a CUT into the destination, sent through the one transfer path',
  )
  ok(
    /void planEntries\(project, transfer\.sources, transfer\.destDir, transfer\.mode\)[\s\S]{0,400}?setPendingPaste\(\{ transfer, ask: startAsk\(collisions\) \}\)/.test(
      tree,
    ),
    'and it is PLANNED first, so a collision raises `PasteConfirm` instead of overwriting — the '
      + 'user asked for a confirmation on exactly this hazard, and one hazard gets one dialog',
  )
  ok(
    /commitTransfer\(project, transfer, \[\]\)/.test(tree),
    'a transfer with nothing in its way sends NO decisions, which is the only shape `fs_paste` '
      + 'cannot overwrite anything with',
  )
  ok(
    !/take\(/.test(tree.slice(tree.indexOf('const runMove'), tree.indexOf('const cancelPaste'))),
    'and the drop never calls `take()` — that would overwrite the in-app clipboard and the system '
      + 'clipboard for a gesture that has nothing to do with either',
  )
  ok(
    /export function planEntries/.test(clipboard)
      && /export async function pasteEntries/.test(clipboard),
    'the two calls a drop needs are free functions, reachable without going near the clipboard',
  )
  ok(
    /const result = await pasteEntries\([\s\S]{0,300}?if \(clip\.mode === 'cut'\) set\(\{ clip: null \}\)/.test(
      clipboard,
    ),
    'and the STORE still owns consuming the clip on a cut — the drag must not inherit that, or a '
      + 'drop would clear a clipboard it never read',
  )
  /*
   * Pinned as the *conditional*, not as the identifier. The first version of this assertion was
   * `/moveCancelledNote/.test(tree)`, and it passed with the call site reverted to
   * `setNote(cancelledNote())` — because the import line at the top of the file still spells the
   * name. An assertion that a symbol is imported is not an assertion that it is used.
   */
  ok(
    /setNote\(fromClip \? cancelledNote\(\) : moveCancelledNote\(\)\)/.test(tree),
    'a cancelled drop says "Move cancelled" and a cancelled paste says "Paste cancelled" — a '
      + 'message that names a gesture the user did not make is how people learn to stop reading '
      + 'them',
  )
  ok(
    /const fromClip = pendingPaste\?\.transfer\.fromClip \?\? true/.test(tree),
    'and it reads that from the transfer being cancelled, not from whichever one ran last',
  )

  // The hook's own contract, as source: the parts a missing line makes silently wrong.
  ok(
    /window\.addEventListener\('keydown', key, true\)/.test(hook) && /'Escape'/.test(hook),
    '⎋ cancels a drag in flight, in the capture phase so nothing behind the panel also answers it',
  )
  ok(
    /window\.addEventListener\('pointercancel', cancel\)/.test(hook),
    'a cancelled pointer ends the drag rather than leaving it in flight for ever',
  )
  ok(
    /useEffect\(\(\) => \(\) => finish\(false\), \[finish\]\)/.test(hook),
    'and an unmount mid-drag takes the window listeners and the spring timer with it',
  )
  ok(
    /if \(live\.current\.onMove === undefined\) return/.test(hook),
    'the hook refuses to start when there is nothing to land on',
  )
  ok(
    /Math\.abs\(ev\.clientX - active\.from\.x\) > THRESHOLD/.test(hook),
    'a press is not a drag until it has moved, or every click in the tree would be one',
  )
  ok(
    !/preventDefault\(\)/.test(hook.slice(hook.indexOf('const onPointerDown'), hook.indexOf('const move ='))),
    'and the press is NOT defaultPrevented: rows are not focusable here, so that would stop the '
      + 'scroller — the tree’s single tab stop — from ever taking the keyboard',
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`\ncheck-tree-drag: ${failed} failure(s)`)
  process.exit(1)
}
console.log('check-tree-drag: ok')
