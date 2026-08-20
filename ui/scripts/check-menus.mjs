/**
 * The context menu's decisions, checked without a DOM.
 *
 * `src/menus/model.ts` is import-free on purpose so the TypeScript already in `node_modules`
 * can compile it standalone — same harness as `check-key-gate.mjs` and `check-theme.mjs`.
 * What is proved here is the whole of what a menu *decides*:
 *
 *   - which lines can be clicked, and what a greyed-out one says about itself;
 *   - where the box lands when the pointer is near an edge of an **undecorated window that
 *     is not the screen**, which is the case this app is always in;
 *   - what the arrow keys do, including over separators and disabled lines;
 *   - whether a right-click gets our menu or the webview's.
 *
 * What this does NOT cover, and nothing here should be read as claiming: that the box paints,
 * that `getBoundingClientRect` returns what `placeMenu` is told, that focus really returns to
 * the element it came from, or that WebKitGTK honours a `preventDefault` issued in the
 * capture phase. There is no browser in this process. Those seams live in `ContextMenu.tsx`
 * and `native.ts`, which is exactly why every decision was moved out of them.
 *
 * Run: `pnpm --dir ui run check:menus`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-menus-'))
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

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/menus/model.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      // The project sets both, and this module is written for them.
      '--noUncheckedIndexedAccess',
      '--exactOptionalPropertyTypes',
    ],
    { stdio: 'inherit' },
  )

  const {
    MENU_MARGIN,
    NATIVE_MENU_ATTR,
    NO_ACTION_REASON,
    activeItem,
    anchorToRect,
    focusReturnPlan,
    isEmptyMenu,
    isInertMenu,
    moveFocus,
    placeMenu,
    placeSubmenu,
    resolveMenu,
    wantsNativeMenu,
  } = await import(`file://${join(out, 'model.js')}`)

  // =====================================================================================
  // 1. Enablement — what a user can click, and what a dead line tells them
  // =====================================================================================

  const noop = () => {}
  // A shipped-shape menu: a live item with a binding, one with an unbound command, an
  // explicitly-disabled one, a toggle, and one whose author forgot the handler.
  const chipFor = (command) =>
    ({ 'file.save': '⌃S', 'pane.close': '⌃W' })[command] ?? null

  const menu = resolveMenu(
    [
      { id: 'save', label: 'Save', command: 'file.save', run: noop },
      { id: 'reveal', label: 'Reveal in explorer', command: 'file.reveal', run: noop },
      { kind: 'separator' },
      { id: 'wrap', label: 'Word wrap', checked: true, run: noop },
      { id: 'minimap', label: 'Minimap', checked: false, run: noop },
      { kind: 'separator' },
      {
        id: 'rename',
        label: 'Rename',
        command: 'file.save',
        disabledReason: 'The file is outside the project',
        run: noop,
      },
      { id: 'unwired', label: 'Compare with…' },
      { id: 'delete', label: 'Delete', command: 'pane.close', danger: true, run: noop },
    ],
    { chipFor },
  )

  eq(
    menu.map((e) => e.id),
    ['save', 'reveal', 'sep-0', 'wrap', 'minimap', 'sep-1', 'rename', 'unwired', 'delete'],
    'every entry survives resolution, separators included and numbered',
  )
  eq(
    menu.filter((e) => e.kind === 'item').map((e) => e.enabled),
    [true, true, true, true, false, false, true],
    'a stated reason disables, and so does a missing handler',
  )
  eq(
    menu.find((e) => e.id === 'unwired').reason,
    NO_ACTION_REASON,
    'an item with no `run` is disabled by construction rather than trusted to the author — '
      + 'a menu line that does nothing when clicked is how the `+ row` buttons presented',
  )
  eq(
    menu.find((e) => e.id === 'unwired').run,
    null,
    'and it carries no callback, so a caller that ignores `enabled` still cannot fire it',
  )
  eq(
    menu.find((e) => e.id === 'rename').reason,
    'The file is outside the project',
    'the author’s sentence is kept verbatim',
  )
  eq(
    menu.find((e) => e.id === 'rename').run,
    null,
    'a disabled item drops its handler even though one was supplied',
  )

  // --- the hint is the keymap's, or it is nothing ----------------------------------------

  eq(
    menu.filter((e) => e.kind === 'item').map((e) => e.hint),
    ['⌃S', null, null, null, null, null, '⌃W'],
    'hints come from `chipFor` and only from it; an unbound command shows nothing rather '
      + 'than its own id',
  )
  eq(
    menu.find((e) => e.id === 'rename').hint,
    null,
    'a disabled item shows its reason in that slot instead — the two never collide because '
      + 'an item is one or the other',
  )
  eq(
    resolveMenu([{ id: 'a', label: 'A', command: 'file.save', run: noop }]).map((e) => e.hint),
    [null],
    'and with no keymap at all nothing is invented',
  )
  // The drift this design exists to prevent, stated as a test: rebind, and the chip moves.
  eq(
    resolveMenu([{ id: 'a', label: 'Save', command: 'file.save', run: noop }], {
      chipFor: (c) => (c === 'file.save' ? '⌘⇧S' : null),
    })[0].hint,
    '⌘⇧S',
    'a user who rebinds `file.save` sees the new chord in the menu, with nothing to update',
  )

  // --- toggles ---------------------------------------------------------------------------

  eq(
    menu.filter((e) => e.kind === 'item').map((e) => e.checked),
    [null, null, true, false, null, null, null],
    '`null` is "not a toggle" and is distinct from an unchecked one — a check gutter is '
      + 'drawn for the second and not the first',
  )
  eq(menu.find((e) => e.id === 'delete').danger, true, 'danger survives resolution')

  // --- separator hygiene ------------------------------------------------------------------
  //
  // Callers build menus with `...(cond ? [item] : [])`, so leading, trailing and doubled
  // rules are the normal output of correct caller code, not a caller mistake.

  const hygiene = resolveMenu(
    [
      { kind: 'separator' },
      { kind: 'separator' },
      { id: 'a', label: 'A', run: noop },
      { kind: 'separator' },
      { kind: 'separator' },
      { kind: 'separator' },
      { id: 'b', label: 'B', run: noop },
      { kind: 'separator' },
    ],
  )
  eq(
    hygiene.map((e) => e.id),
    ['a', 'sep-0', 'b'],
    'leading rules are dropped, a run collapses to one, and the trailing rule never lands',
  )
  eq(isEmptyMenu(hygiene), false, 'that menu has items')
  eq(
    resolveMenu([{ kind: 'separator' }, { kind: 'separator' }]),
    [],
    'a menu of nothing but rules resolves to nothing at all',
  )
  eq(
    isEmptyMenu(resolveMenu([{ kind: 'separator' }])),
    true,
    'and reads as empty, so the hook declines to open an empty box at the pointer',
  )
  eq(isEmptyMenu(menu), false, 'a real menu is not empty')
  eq(
    isInertMenu(resolveMenu([{ id: 'x', label: 'Rename', disabledReason: 'Read-only' }])),
    true,
    'all-disabled is *inert*, not empty — it still opens, because a reason is an answer',
  )
  eq(isInertMenu(menu), false, 'and a menu with one live line is not inert')

  // =====================================================================================
  // 1b. Submenus — a row that opens a list instead of doing something
  // =====================================================================================
  //
  // The editor's *Send this to Claude ▸* is the only caller, and what it lists is the
  // project's Claude conversations by the names their user gave them. The three things that
  // can go wrong here are all decisions, so all three are decided in `model.ts`: a parent must
  // not be treated as wired-to-nothing, a parent must not also fire a `run`, and a disabled
  // parent must not hand out a live builder.

  let built = 0
  const rows = () => {
    built++
    return [
      { id: 'one', label: '1: agents', run: noop },
      { id: 'two', label: '2: git-details', run: noop },
    ]
  }

  const withSub = resolveMenu(
    [
      { id: 'send', label: 'Send line 12 to Claude', submenu: rows },
      { id: 'off', label: 'Send this file to Claude', disabledReason: 'No Claude here', submenu: rows },
      { id: 'both', label: 'Confused', submenu: rows, run: () => { fired = 'both' } },
    ],
    { chipFor },
  )

  eq(
    withSub.find((e) => e.id === 'send').enabled,
    true,
    'a row with a submenu and no `run` is NOT the wired-to-nothing shape `NO_ACTION_REASON` '
      + 'names — it does something when clicked, namely open a list',
  )
  eq(
    withSub.find((e) => e.id === 'send').run,
    null,
    'and it never runs: one click cannot mean two things, so the list wins and the caller '
      + 'cannot leave a second meaning half-wired',
  )
  ok(
    typeof withSub.find((e) => e.id === 'send').submenu === 'function',
    'the builder survives resolution as a function, so the rows are built when the submenu '
      + 'opens rather than when its parent did — which is what keeps a list of live sessions '
      + 'from being a snapshot of the moment the menu was right-clicked',
  )
  eq(
    built,
    0,
    'and resolving the parent has not called it: a submenu nobody hovers costs nothing, and '
      + 'a builder that ran at parent-resolve time would be exactly the stale snapshot',
  )
  eq(
    withSub.find((e) => e.id === 'both').run,
    null,
    'a caller that supplies both gets the submenu and no handler — stated rather than left '
      + 'to whichever the component happens to check first',
  )
  eq(
    [withSub.find((e) => e.id === 'off').enabled, withSub.find((e) => e.id === 'off').submenu],
    [false, null],
    'a disabled parent hands out no builder, for the reason a disabled item hands out no '
      + '`run`: an enablement the caller ignores must not leave a live handle behind',
  )
  eq(
    resolveMenu(rows(), { chipFor }).map((e) => e.label),
    ['1: agents', '2: git-details'],
    'and the rows themselves resolve like any other menu — one model, one set of rules, so '
      + 'a submenu cannot grow its own idea of what disabled means',
  )
  eq(built, 1, 'that last call is the only one this section made')

  // =====================================================================================
  // 2. Geometry — the flip, in a window that is not the screen
  // =====================================================================================

  eq(MENU_MARGIN, 8, 'the gutter kept between the box and the window edge')

  // A detached-pane window: 420 x 300. Nothing about this is screen-sized, which is the
  // whole point — `screen.availHeight` would report room this webview cannot paint into.
  const win = { width: 420, height: 300 }
  const box = { width: 180, height: 120 }

  const middle = placeMenu({ x: 100, y: 100 }, box, win)
  eq(
    [middle.x, middle.y, middle.flippedX, middle.flippedY],
    [100, 100, false, false],
    'with room on both sides the box hangs down and right from the pointer exactly',
  )
  eq(middle.maxHeight, 192, 'and may grow to the space below it: 300 - 8 - 100')

  const right = placeMenu({ x: 400, y: 100 }, box, win)
  eq(
    [right.x, right.flippedX],
    [220, true],
    'near the right edge it opens leftwards — 400 - 180 — rather than being shoved back, '
      + 'which would put the pointer in the middle of the items',
  )
  const bottom = placeMenu({ x: 100, y: 280 }, box, win)
  eq(
    [bottom.y, bottom.flippedY, bottom.maxHeight],
    [160, true, 272],
    'near the bottom the box’s *bottom* edge lands on the pointer: 280 - 120',
  )
  const corner = placeMenu({ x: 415, y: 295 }, box, win)
  eq(
    [corner.x, corner.y, corner.flippedX, corner.flippedY],
    [232, 172, true, true],
    'the bottom-right corner flips both axes, and the clamp then trims the last few pixels '
      + 'so the far edges land on the margin rather than 3px off the window',
  )

  // Flip only when the other side is genuinely roomier — not merely because the preferred
  // side is short. A menu that jumped above the pointer whenever it did not quite fit below
  // would be the more surprising of the two behaviours.
  const short = placeMenu({ x: 100, y: 100 }, { width: 180, height: 200 }, win)
  eq(
    [short.y, short.flippedY, short.maxHeight],
    [92, false, 200],
    'below the pointer there are 192px and above only 92: a 200px box fits in neither, so '
      + 'it stays down — the common direction — and is clamped up off the bottom margin. Its '
      + 'budget is then measured from where it *landed* (300 - 8 - 92), not from the pointer: '
      + 'billing it the 192px below the pointer would put a scrollbar on a box that the clamp '
      + 'had just made room for.',
  )
  const even = placeMenu({ x: 100, y: 150 }, { width: 180, height: 200 }, win)
  eq(
    [even.y, even.flippedY],
    [92, false],
    'and a tie stays down too: the flip has to be strictly better to be worth the surprise',
  )
  const barely = placeMenu({ x: 100, y: 190 }, box, win)
  eq(
    [barely.y, barely.flippedY, barely.maxHeight],
    [70, true, 182],
    'but where the box genuinely does not fit below and does fit above, it flips — bottom '
      + 'edge on the pointer at 190 - 120',
  )

  // Clamping, and the pathological sizes.
  const topLeft = placeMenu({ x: 2, y: 1 }, box, win)
  eq(
    [topLeft.x, topLeft.y, topLeft.maxHeight],
    [8, 8, 284],
    'a pointer inside the margin is pushed off it, and the budget is the window less both '
      + 'margins — not the 291px "below the pointer" that would run off the bottom',
  )
  const tall = placeMenu({ x: 100, y: 150 }, { width: 180, height: 900 }, win)
  eq(
    [tall.y, tall.flippedY, tall.maxHeight],
    [8, false, 284],
    'a menu taller than the window pins to the top margin and scrolls inside the whole '
      + 'window less both margins — not inside the 142px that were below the pointer it is no '
      + 'longer next to, which would leave half the window blank under a scrolling box',
  )
  /*
   * The invariant the two cases above are instances of, stated once so a future edit to the
   * clamp cannot quietly reintroduce either half: a box that opens **downward** budgets from
   * where it landed to the bottom margin — exactly, in both directions. Under-budgeting is
   * the bug fixed above (a scrollbar with empty window beneath it); over-budgeting is the
   * mirror image, a box whose `max-height` lets it run off the bottom edge.
   *
   * Only the downward case. A flipped box is budgeted against the room above the pointer, not
   * the room below its own top — see the assertion under this loop for why.
   */
  for (const pointerY of [0, 1, 8, 50, 150, 200, 291, 299]) {
    for (const height of [40, 120, 200, 284, 285, 900]) {
      const p = placeMenu({ x: 100, y: pointerY }, { width: 180, height }, win)
      if (p.flippedY) continue
      eq(
        Math.round(p.y + p.maxHeight),
        win.height - MENU_MARGIN,
        `a ${height}px box at y=${pointerY} budgets from where it landed to the bottom `
          + 'margin — no scrollbar while the window still has room under it, and no '
          + '`max-height` that would let it run past the edge',
      )
    }
  }
  /*
   * The flipped branch is the one deliberate exception, asserted rather than left to be
   * rediscovered: a box that flipped is budgeted by the room *above the pointer*, because the
   * whole point of the flip is that its bottom edge sits on the pointer, and growing it down
   * to the bottom margin instead would undo the placement it just asked for.
   *
   * That budget is loose when the box is short — a 40px menu flipped at y=291 sits at y=251
   * and is told it may reach 283px, which would end past the window. It is a ceiling, not a
   * height, and a 40px box stays 40px, so nothing paints out of bounds; it is recorded here as
   * known slack rather than tightened, because tightening it to `at.y - y` would set
   * `max-height` to the measured height on every flipped menu and hand a scrollbar to anything
   * that rounded up by half a pixel.
   */
  const flippedTall = placeMenu({ x: 100, y: 200 }, { width: 180, height: 285 }, win)
  eq(
    [flippedTall.flippedY, flippedTall.maxHeight],
    [true, 192],
    'a flipped box keeps its bottom edge on the pointer and scrolls inside the 192px above '
      + 'it, rather than growing down past the thing it was opened on',
  )
  const wide = placeMenu({ x: 100, y: 100 }, { width: 900, height: 120 }, win)
  eq(
    [wide.x, wide.flippedX],
    [8, false],
    'a box wider than the window pins to the left margin rather than off the right one — '
      + 'the inverted clamp keeps the readable end',
  )
  eq(
    placeMenu({ x: 100, y: 100 }, box, win, 0).maxHeight,
    200,
    'the margin is a parameter, and zero means flush',
  )

  eq(
    anchorToRect({ left: 40, bottom: 66 }),
    { x: 40, y: 66 },
    'a keyboard-invoked menu hangs off the element’s bottom-left, having no pointer to sit at',
  )

  // --- and a submenu, which hangs off a row rather than a point ---------------------------
  //
  // The distinction is the whole reason this is not `placeMenu` with a cleverer anchor: a
  // pointer has no width, so `placeMenu` flips *through* its anchor; a row does, and flipping
  // through its right edge would lay the submenu over the parent menu and hide the row the
  // user is hovering.

  const row = { left: 100, right: 200, top: 100, bottom: 126 }
  const sub = placeSubmenu(row, box, win)
  eq(
    [sub.x, sub.y, sub.flippedX, sub.flippedY],
    [200, 100, false, false],
    'with room to the right it opens at the row’s right edge, top-aligned with the row, so '
      + 'the first submenu item sits beside the row that opened it',
  )
  eq(sub.maxHeight, 192, 'and budgets from where it landed to the bottom margin: 300 - 8 - 100')

  const tight = placeSubmenu({ left: 200, right: 380, top: 100, bottom: 126 }, box, win)
  eq(
    [tight.x, tight.flippedX],
    [20, true],
    'with 32px to the right and 192 to the left it flips round the row to `left - width` — '
      + '200 - 180 — rather than through `right - width`, which would be 200 and would sit '
      + 'exactly on top of the parent menu',
  )
  const low = placeSubmenu({ left: 100, right: 200, top: 290, bottom: 300 }, box, win)
  eq(
    [low.y, low.flippedY, low.maxHeight],
    [172, false, 120],
    'a row near the bottom pulls its submenu up until the box fits — 300 - 8 - 120 — rather '
      + 'than flipping it, because a submenu grows downwards from a placed top edge and there '
      + 'is nothing for a flip to mean',
  )
  const huge = placeSubmenu(row, { width: 180, height: 900 }, win)
  eq(
    [huge.y, huge.maxHeight],
    [8, 284],
    'a submenu taller than the window pins to the top margin and scrolls inside the window '
      + 'less both margins, exactly as a top-level menu does',
  )
  const noRoom = placeSubmenu({ left: 8, right: 20, top: 100, bottom: 126 }, { width: 900, height: 120 }, win)
  eq(
    [noRoom.x, noRoom.flippedX],
    [8, false],
    'and a submenu wider than the window pins to the left margin — the inverted clamp keeps '
      + 'the readable end, the same choice `placeMenu` makes',
  )
  /*
   * The one case that overlaps the parent, recorded rather than pretended away. A row whose
   * right edge leaves less room than the submenu needs, on a side that is still roomier than
   * the other, is placed rightwards and then clamped back off the margin — which lands it on
   * top of the row it came from. This is a 420px-wide detached-pane window with a 180px
   * submenu; on any ordinary window it does not arise, and the alternatives are worse: flip
   * anyway (over the parent, on the side with *less* room) or paint off the window edge.
   */
  const squeezed = placeSubmenu({ left: 100, right: 280, top: 100, bottom: 126 }, box, win)
  eq(
    [squeezed.x, squeezed.flippedX],
    [232, false],
    'and when neither side can hold it the box is pinned to the right margin, overlapping '
      + 'the row — the honest last resort, since 420px of window cannot show a 180px row and '
      + 'a 180px submenu side by side',
  )

  // =====================================================================================
  // 3. Keyboard — arrows over a menu with holes in it
  // =====================================================================================

  // indices:  0 save  1 reveal  2 SEP  3 wrap  4 minimap  5 SEP  6 rename(off)
  //           7 unwired(off)  8 delete
  eq(moveFocus(menu, null, 'next'), 0, 'Down from nothing focused lands on the first item')
  eq(moveFocus(menu, null, 'prev'), 8, 'Up from nothing wraps to the last')
  eq(moveFocus(menu, 1, 'next'), 3, 'Down steps over the separator')
  eq(moveFocus(menu, 3, 'prev'), 1, 'and Up steps back over it')
  eq(moveFocus(menu, 4, 'next'), 8, 'Down skips both disabled lines and the rule between them')
  eq(moveFocus(menu, 8, 'prev'), 4, 'Up skips them too')
  eq(moveFocus(menu, 8, 'next'), 0, 'and the end wraps to the start')
  eq(moveFocus(menu, 0, 'prev'), 8, 'as does the start to the end')
  eq(moveFocus(menu, null, 'first'), 0, 'Home')
  eq(moveFocus(menu, null, 'last'), 8, 'End')
  eq(
    moveFocus(menu, 6, 'next'),
    0,
    'an index that is no longer focusable — the menu re-resolved under a moving selection — '
      + 'is treated as nowhere rather than as a position to step from',
  )
  eq(moveFocus(menu, 2, 'prev'), 8, 'a separator index likewise')
  eq(moveFocus(menu, 99, 'next'), 0, 'and an out-of-range one')

  const inert = resolveMenu([
    { id: 'x', label: 'Rename', disabledReason: 'Read-only' },
    { kind: 'separator' },
    { id: 'y', label: 'Delete', disabledReason: 'Read-only' },
  ])
  eq(
    [moveFocus(inert, null, 'next'), moveFocus(inert, null, 'first'), moveFocus(inert, 0, 'prev')],
    [null, null, null],
    'in a menu where nothing can run, the arrows move nothing rather than settling on a '
      + 'line that would refuse Enter',
  )

  // --- and what Enter does at that index --------------------------------------------------

  let fired = null
  const armed = resolveMenu([
    { id: 'go', label: 'Go', run: () => { fired = 'go' } },
    { kind: 'separator' },
    { id: 'no', label: 'No', disabledReason: 'nope', run: () => { fired = 'no' } },
  ])
  eq(activeItem(armed, 0).id, 'go', 'Enter on a live item finds it')
  activeItem(armed, 0).run()
  eq(fired, 'go', 'and the handler that runs is the caller’s')
  eq(activeItem(armed, 1), null, 'Enter on a separator does nothing')
  eq(activeItem(armed, 2), null, 'Enter on a disabled item does nothing')
  eq(activeItem(armed, null), null, 'Enter with nothing focused does nothing')
  eq(activeItem(armed, 7), null, 'nor does Enter past the end')
  eq(fired, 'go', 'and none of those fired anything')

  // =====================================================================================
  // 4. Ours or the webview's
  // =====================================================================================

  eq(NATIVE_MENU_ATTR, 'data-native-menu', 'the attribute a surface opts back in with')

  const chain = (...els) => els
  const div = { tag: 'div' }
  const input = { tag: 'input', type: 'text' }

  eq(
    wantsNativeMenu(chain(div, div), true),
    false,
    'the ordinary case — a tree row, a tab, a pane — gets no webview menu, so no '
      + '"Inspect element" anywhere the user was right-clicking',
  )
  eq(
    wantsNativeMenu(chain(input, div), true),
    true,
    'a plain text input keeps it, because it is the only thing offering cut/copy/paste '
      + 'there and losing paste is a regression nobody asked for',
  )
  eq(
    wantsNativeMenu(chain({ tag: 'textarea' }, div), true),
    true,
    'so does a textarea',
  )
  eq(
    wantsNativeMenu(chain({ tag: 'span', contentEditable: true }, div), true),
    true,
    'and a contenteditable — the target there is whatever span the text sits in',
  )
  eq(
    wantsNativeMenu(chain(div, { tag: 'div', contentEditable: true }), true),
    true,
    'which is why the whole chain is inspected and not just the target',
  )
  eq(
    wantsNativeMenu(chain(input, div), false),
    false,
    'the default is one switch: `nativeInTextInputs: false` suppresses everywhere, for the '
      + 'day this app has its own paste',
  )

  for (const type of ['checkbox', 'radio', 'range', 'file', 'color', 'button', 'date']) {
    eq(
      wantsNativeMenu(chain({ tag: 'input', type }, div), true),
      false,
      `an <input type=${type}> has no selection, so a native cut/copy/paste menu on it is `
        + 'three items that do nothing',
    )
  }
  eq(
    wantsNativeMenu(chain({ tag: 'input' }, div), true),
    true,
    'an <input> with no type attribute is a text input, as HTML says',
  )
  eq(
    wantsNativeMenu(chain({ tag: 'input', type: 'TEXT' }, div), true),
    true,
    'and the type is matched case-insensitively',
  )
  eq(
    wantsNativeMenu(chain({ tag: 'input', type: 'text', disabled: true }, div), true),
    false,
    'a disabled input cannot be selected in, so it gets nothing',
  )

  // --- the explicit attribute beats everything -------------------------------------------

  eq(
    wantsNativeMenu(chain({ tag: 'div', nativeMenu: 'true' }, div), true),
    true,
    'a surface can opt back in',
  )
  eq(
    wantsNativeMenu(chain({ tag: 'div', nativeMenu: '' }, div), true),
    true,
    'a bare `data-native-menu` reads as an empty string and means yes, like every other '
      + 'boolean attribute in HTML',
  )
  for (const off of ['false', 'FALSE', '0', 'off']) {
    eq(
      wantsNativeMenu(chain({ tag: 'input', type: 'text', nativeMenu: off }, div), true),
      false,
      `\`data-native-menu="${off}"\` takes it away from an input that would otherwise keep it`,
    )
  }
  eq(
    wantsNativeMenu(chain(input, { tag: 'div', nativeMenu: 'false' }), true),
    false,
    'a pane can opt its whole subtree out, inputs included — which is how a surface that '
      + 'ships its own edit menu turns the webview’s off',
  )
  eq(
    wantsNativeMenu(
      chain({ tag: 'input', type: 'text', nativeMenu: 'true' }, { tag: 'div', nativeMenu: 'false' }),
      true,
    ),
    true,
    'and one field inside it can opt back in: the innermost statement wins',
  )
  eq(
    wantsNativeMenu(chain({ tag: 'div', nativeMenu: 'false' }, { tag: 'div', nativeMenu: 'true' }), false),
    false,
    'an explicit `false` is still false when text inputs are globally suppressed',
  )
  eq(
    wantsNativeMenu(chain({ tag: 'div', nativeMenu: 'true' }, div), false),
    true,
    'and an explicit `true` survives it — the escape hatch is not the same switch',
  )
  eq(wantsNativeMenu([], true), false, 'an event with no element target gets nothing')

  // =====================================================================================
  // 4b. Handing focus back — the reported "the buffer scrolls to the top" jump
  // =====================================================================================
  //
  // *"Right-click in the editor, move the pointer over the menu, close it — the buffer is at
  // line 1. Without touching the menu it does not happen."* Every step of that is WebKit's
  // (`FocusController::setFocusedElement` → `clearSelectionIfNeeded`, then
  // `Element::updateFocusAppearance` inventing and revealing a selection on a root editable
  // element), and the one line of ours in the chain is the plain `previous.focus()` the menu
  // performed on the way out. The decision that replaces it is `focusReturnPlan`, here rather
  // than in the hook precisely so it can be run.

  eq(
    focusReturnPlan({ hasCustom: false, previousConnected: true }),
    ['default'],
    'an ordinary surface just gets its element focused back',
  )
  eq(
    focusReturnPlan({ hasCustom: false, previousConnected: false }),
    [],
    'and a menu whose action deleted the row it hung off focuses nothing rather than a '
      + 'detached node — focus falls to <body>, which the next Tab can start from',
  )
  eq(
    focusReturnPlan({ hasCustom: true, previousConnected: true }),
    ['custom', 'default'],
    'the surface\'s own restore is tried FIRST: `preventScroll` suppresses WebKit\'s reveal '
      + 'and not the `setSelection` before it, so on a CodeMirror buffer the plain DOM step '
      + 'still collapses the caret to the top — only `EditorView.focus()` restores both',
  )
  ok(
    focusReturnPlan({ hasCustom: true, previousConnected: true }).length === 2,
    'and `default` stays behind it as a fallback: `restoreFocus` may decline (the view was '
      + 'destroyed by the action), and a declined custom step with nothing after it is a '
      + 'window left with no focus at all',
  )
  eq(
    focusReturnPlan({ hasCustom: true, previousConnected: false }),
    ['custom'],
    'a surface that rebuilt its DOM under the menu is still asked — it is exactly the surface '
      + 'that knows where focus should go',
  )

  // The call sites. Every bug in this section is a *missing call*, which is the one shape a
  // test of the surrounding code passes straight over.
  {
    const hook = readFileSync('src/menus/useContextMenu.tsx', 'utf8')
    ok(
      /focusReturnPlan\(/.test(hook),
      'useContextMenu consults focusReturnPlan — a `close` that decided for itself would pass '
        + 'every assertion above while shipping the jump',
    )
    ok(
      /previous\?\.focus\(\{ preventScroll: true \}\)/.test(hook),
      'and its default step passes `preventScroll`, which is what maps to '
        + 'SelectionRevealMode::DoNotReveal and kills both the editable reveal and '
        + 'scheduleScrollToFocusedElement for every other surface',
    )

    const menu = readFileSync('src/menus/ContextMenu.tsx', 'utf8')
    /*
     * The focus effect, which was a no-op for the life of the component.
     *
     * `useLayoutEffect(..., [])` fired in the same commit as the measure effect, before React
     * had flushed its `setPlacement` — so the box was still `visibility: hidden` and WebKit
     * refuses focus on such an element (`hasFocusableStyle`). The whole keyboard model behind
     * `onKeyDown` was therefore reachable only after the pointer touched an item, and a
     * Shift+F10 menu could not be driven at all.
     *
     * Matched on the dependency array, because that is the whole defect: the body was already
     * right.
     */
    const focusEffect = menu.slice(menu.indexOf('if (placement === null) return'))
    ok(
      /box\.current\?\.focus\(\{ preventScroll: true \}\)/.test(focusEffect.slice(0, 400)),
      'the box focuses itself only once a placement is committed — keyed on `[]` it ran while '
        + 'the box was still `visibility: hidden`, which WebKit refuses, so the arrows, Escape '
        + 'and Enter were reachable from nothing until the pointer moved',
    )
    ok(
      /\}, \[placement === null\]\)/.test(focusEffect.slice(0, 500)),
      'and on `placement === null` rather than on `placement`: re-measuring a moved anchor '
        + 'must not yank focus off an item the user has already arrowed to',
    )
    ok(
      /item\.focus\(\{ preventScroll: true \}\)/.test(menu)
        && /item\.scrollIntoView\(\{ block: 'nearest' \}\)/.test(menu),
      'hover/arrow focus scrolls the box deliberately instead of letting the browser do it — '
        + 'see the scroll listener below',
    )
    ok(
      /const onScroll = \(ev: Event\) => \{\s*if \(inside\(ev\.target\)\) return/.test(menu),
      'and a scroll INSIDE the menu does not dismiss it: `.menu` is `overflow-y: auto` under a '
        + 'computed maxHeight, and the listener is in capture (scroll does not bubble), so '
        + 'without this arrowing past the fold of a long menu closes it under the user',
    )

    const code = readFileSync('src/editor/codeMenu.tsx', 'utf8')
    ok(
      /restoreFocus:/.test(code) && /live\.focus\(\)/.test(code.slice(code.indexOf('restoreFocus:'))),
      'and the code pane supplies `restoreFocus` calling EditorView.focus() — the generic '
        + 'DOM step cannot restore a CodeMirror selection, only its scroll',
    )
  }

  // =====================================================================================
  // 5. The component is wired to the model, and to the tokens
  // =====================================================================================
  //
  // Everything above is the model in isolation, which says nothing about whether the box on
  // screen consults it. These are source assertions and worth being exact about what that is
  // worth: they prove the imports and the tokens are there, not that a pixel moved.

  const component = readFileSync('src/menus/ContextMenu.tsx', 'utf8')
  for (const fn of ['placeMenu', 'placeSubmenu', 'moveFocus', 'activeItem']) {
    ok(
      new RegExp(`\\b${fn}\\(`).test(component),
      `ContextMenu.tsx calls \`${fn}\` — a box that placed itself would pass everything above `
        + 'while flipping the wrong way on screen',
    )
  }
  ok(
    /createPortal\(/.test(component),
    'and renders through a portal, without which a menu in an `overflow: hidden` pane — '
      + 'which is most panes here — is clipped by it',
  )

  /*
   * The submenu box is the *same* component's, in the *same* portal, and the dismissal check
   * asks about both boxes. This is the one submenu rule that cannot be moved into `model.ts`,
   * and getting it wrong is not subtle: a nested `ContextMenu` would answer "is the pointer
   * inside my box" for its own box only, so the first click on a submenu row would read as an
   * outside click on the parent, close it, and unmount the row under the pointer.
   */
  ok(
    /box\.current\?\.contains\(target\) === true \|\| subBox\.current\?\.contains\(target\) === true/
      .test(component),
    'the dismissal check covers the submenu box as well as the main one — the submenu is a '
      + 'portal *sibling*, so asking only the main box makes every click on a submenu row an '
      + 'outside click that closes the menu before the row can run',
  )
  ok(
    /aria-haspopup=/.test(component) && /aria-expanded=/.test(component),
    'and a parent row says it has a popup and whether it is open, which is the only signal a '
      + 'screen reader gets that the row is not an ordinary one',
  )
  // `includes("'blur'")` was not enough: deleting the `addEventListener` leaves the matching
  // `removeEventListener` behind, and the grep passed over a menu that never dismissed on
  // blur at all. Match the registration, and match the removal beside it so the pair cannot
  // drift into a leaked listener either.
  for (const listener of ['pointerdown', 'scroll', 'resize', 'blur']) {
    ok(
      component.includes(`addEventListener('${listener}'`),
      `it dismisses on ${listener}`,
    )
    ok(
      component.includes(`removeEventListener('${listener}'`),
      `and takes the ${listener} listener back off on unmount`,
    )
  }

  // Both themes, checked the way `check-theme.mjs` checks the terminal's slice: every colour
  // the menu names must be defined in the light block *and* the dark one. Light is the
  // default and is white, and it is the one a dark-first author forgets.
  const css = readFileSync('src/menus/ContextMenu.module.css', 'utf8')
  const used = [...new Set([...css.matchAll(/var\((--[a-z0-9-]+)/g)].map((m) => m[1]))].sort()
  ok(used.length >= 8, `the menu is built from tokens (${used.length} of them), not literals`)
  eq(
    [...css.matchAll(/#[0-9a-fA-F]{3,8}\b/g)].map((m) => m[0]),
    [],
    'and names no colour literal at all, so neither theme can be the one it was written for',
  )

  // `.item:hover:not(.disabled)` is three selectors wide and sets `color`, so a bare
  // `.danger` loses to it and the destructive line goes the same colour as every other line
  // in the instant the pointer reaches it. The fix is a specificity match, and it is the kind
  // of thing that is silently deleted by a later tidy-up, so it is pinned here.
  for (const state of [':hover:not(.disabled)', ':focus-visible', '.active']) {
    ok(
      css.includes(`.item.danger${state}`),
      `a danger item keeps \`--red\` under \`${state}\` — it outranks the hover rule rather `
        + 'than merely coming after it, which would not have been enough',
    )
  }

  const tokens = readFileSync('src/styles/tokens.css', 'utf8')
  const blockOf = (selector) => {
    const start = tokens.indexOf(selector)
    if (start < 0) throw new Error(`no ${selector} block in tokens.css`)
    const open = tokens.indexOf('{', start)
    const close = tokens.indexOf('\n}', open)
    return tokens.slice(open, close)
  }
  // `:root` is light, deliberately — see the header of tokens.css.
  const light = blockOf(':root,')
  const dark = blockOf("[data-theme='dark']")
  /*
   * Tokens that are measurements rather than colours, and therefore live in the second,
   * theme-independent `:root` block — a light and a dark theme have no separate opinion about
   * a font stack or a type scale. `--shadow` is in the set and is deliberately *not* exempt
   * below: it is a colour wearing a dimension's name, and the two themes really do differ.
   *
   * The chrome type scale joins them as a family rather than as fourteen entries: `--ui-scale`
   * is the chrome font size's multiplier and `--fs-ui-<n>` are the rungs built out of it, all
   * declared once beside `--font-ui`. Matched by shape so a new rung does not have to be added
   * here as well as to `tokens.css` — `check-ui-scale.mjs` is what holds that ladder to its
   * readers, and it is the script that would notice a rung nobody uses.
   */
  const dimensions = new Set(['--font-ui', '--font-mono', '--shadow'])
  const scaleToken = (token) => token === '--ui-scale' || token.startsWith('--fs-ui-')
  for (const token of used) {
    if (scaleToken(token)) continue
    if (dimensions.has(token) && token !== '--shadow') continue
    ok(light.includes(`${token}:`), `${token} is defined in the light theme, which is default`)
    ok(dark.includes(`${token}:`), `${token} is defined in the dark theme`)
  }

  // The check script must not silently stop covering a file that grows a new decision.
  eq(
    readdirSync('src/menus').sort(),
    [
      'ContextMenu.module.css',
      'ContextMenu.tsx',
      'index.ts',
      'menuState.ts',
      'model.ts',
      'native.ts',
      'useContextMenu.tsx',
    ],
    'the module’s file list — a new file here is a prompt to ask whether its decisions '
      + 'belong in `model.ts`, where this script can hold them',
  )

  if (failed === 0) console.log('context menus: ok')
  else console.error(`\n${failed} failure(s)`)
} finally {
  rmSync(out, { recursive: true, force: true })
}

process.exit(failed > 0 ? 1 : 0)
