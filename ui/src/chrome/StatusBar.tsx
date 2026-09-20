/**
 * The 24px status bar along the bottom of the window.
 *
 * # The four counters are gone
 *
 * The left group used to carry `⊕ — ⊖ — ✗ 0 ⚠ 2` beside the branch: a working-tree diff
 * stat and the diagnostics pair. The diff half never had a source at all — there is no
 * working-tree line count on the wire — so it spent every launch printing a dash, and the
 * diagnostics half is a second rendering of a figure the ⚠ Problems rail button already
 * badges two rows up. The user asked for the row back, and both slots went with it. What is
 * *not* lost is the snapshot behind them: `App.tsx` still derives `diagCounts` and still feeds
 * the rail's badge, so the count has one renderer instead of two.
 *
 * Every field is a prop with a placeholder default, and *this component* reads nothing from the
 * store — so it stays a pure render target that a screenshot test can drive directly.
 *
 * # Two children are the documented exception, and the rule is about this file
 *
 * `<BranchSelector />` and, since M65, `<GitOpIndicator />` each own a module store and subscribe
 * to it themselves. That is not a hole in the rule above, it is the shape the rule takes for
 * state `App.tsx` does not hold: the branch is data the selector must already have to draw its
 * popup, and a running push is started from `keys/dispatch.ts`, the branch popup and the Git
 * panel, none of which `App.tsx` sits on the path of. Threading either through as a prop would
 * put a producer's plumbing in a component with no other use for it — and the property that
 * actually catches bugs, rules living in a module a check script can compile, is kept by both
 * (`chrome/branchModel.ts`, `chrome/gitOpModel.ts`).
 *
 * What the rule still forbids is *this file* growing a subscription. Each such child is one line
 * here and is separately renderable, so `chrome/auditFixture.ts` keeps its handle on the bar.
 *
 * The open file is the one thing that does not arrive as a prop. It comes from
 * `editor/statusReadout.ts`, which the editor pane feeds, and it arrives in two pieces
 * because they change at different rates:
 *
 * * `crates › cide-core › src › lib.rs` — the trail, in React state. It moves when the user
 *   switches file, which is rare, and it wants real markup: one element per segment, so the
 *   `›` separators can be their own colour, each segment can carry its own bidi direction
 *   inside a box that clips from the left, and — since M16 — a segment that leads somewhere can
 *   be a control while one that does not is plain text with no handler at all.
 * * `Rust · UTF-8 · LF · Ln 7, Col 48` — written straight into a DOM node this component
 *   owns. It moves on every caret move, which under a held arrow key is 30 times a second,
 *   and no version of re-rendering the bar that often is worth having.
 *
 * Both used to be a 28px row above every buffer. See `statusReadout.ts` for why they moved,
 * and `editor/EditorSurface.tsx` for the row that is no longer there.
 *
 * # Where they sit, and when they say anything (M34)
 *
 * The trail is in the **left** group, after the branch: `⑂ master` and `src › lib.rs` answer the
 * same question, and the right group is then only the readouts a user watches — the caret line
 * and the Claude session. It arrived in the right group when the breadcrumb row was deleted and
 * stayed there by inertia.
 *
 * Both file slots are gated on `editorFocused`, because the claim stack behind them never
 * demotes: a terminal pane holds no editor, claims nothing, and so left the previous buffer's
 * path and caret standing on the bar while the user typed somewhere else entirely. See that
 * prop for why this hides rather than releases.
 */
import { useEffect, useRef, useState } from 'react'
import {
  sameTrail,
  statusReadout,
  subscribeStatusReadout,
  type ReadoutLine,
} from '@/editor/statusReadout'
import { crumbTargets } from '@/sidebar/rowPaths'
import { BranchSelector } from './BranchSelector'
import { GitOpIndicator } from './GitOpIndicator'
import { Icon } from '@/icons/Icon'

import styles from './StatusBar.module.css'

export interface StatusBarProps {
  /*
   * No `branch` prop. It was here, defaulted to `'—'`, and nothing ever passed one — so the
   * slot printed a dash forever. `BranchSelector` reads the repository itself; a prop would be
   * a second source of truth for a value it already holds to draw its list.
   */
  /*
   * No `added`/`removed` and no `diagnostics` either, for the same reason and one more.
   *
   * The diff pair had no producer anywhere in the workspace — there is no working-tree line
   * count on the wire, so the slot could only ever draw a dash — and the diagnostics pair had
   * one but shared it with the rail's ⚠ badge, which is nearer the eye and already there. See
   * the header.
   */
  claude?: string | undefined
  /**
   * Every path the file tree can hang a row from — the project's roots plus each synthetic
   * group's top-level children, from `fs_reveal_roots` by way of `sidebar/treeStore.ts`. (M16)
   *
   * What it decides is which segments of the trail below are clickable, through the pure
   * `crumbTargets`. It is a *list of paths* rather than a predicate because that keeps this
   * component a render target: the rule lives in a module a check script compiles and runs, not
   * in a callback nothing can reach.
   */
  revealRoots?: readonly string[] | undefined
  /**
   * Select a crumb's path in the file tree, or absent when this window has no file tree.
   *
   * `App.tsx` routes it to `runCommand('file.reveal', { path })` — the same command the ⌃⇧E
   * chord, the palette row, the Explorer's ⌖ button and a Ctrl+click on a directory in terminal
   * output all run. A second reveal path with its own preconditions is how five gestures come to
   * behave in five ways, and this one would additionally have to know about External Libraries.
   *
   * **Absent means no crumb is revealable at all**, not "clicks are ignored": `dispatch.ts`
   * refuses `file.reveal` in a window with no sidebar, with a notice, so a detached-tab window
   * must draw the whole trail inert rather than offer a control that explains itself afterwards.
   * The same rule, for the same reason, that `App.tsx` gives for withholding `onRevealPath` from
   * a detached pane's terminal.
   */
  onRevealSegment?: ((path: string) => void) | undefined
  /**
   * Whether the pane the user is actually in is an editor. (M34)
   *
   * The bar's two file slots — the trail and `Rust · UTF-8 · LF · Ln 7, Col 48` — are fed by
   * `editor/statusReadout.ts`'s claim stack, and that stack deliberately **never demotes**: it
   * is moved by an editor mounting, by a tab coming forward and by DOM focus, and a Claude or
   * bash pane is none of those because it holds no editor and so claims nothing. So the last
   * buffer's path used to stand on the bar for as long as the user typed in a terminal, naming
   * a file they had walked away from. That was the report this prop answers.
   *
   * `App.tsx` derives it from the `focused` pane it already computes for the key context —
   * `focused?.pane.kind === 'editor'`, one derivation, which is what that const exists for.
   * `PaneKind::Editor` is exactly the right grain: it is what `panes/PaneBody.tsx` keys the
   * editor branch on, so it covers `EditorSurface` **and** `ImagePane`, the only two surfaces
   * that ever claim the slot.
   *
   * **Hiding, not releasing.** The claim stack is untouched — it keeps holding the editor's
   * line, so focus coming back restores the trail from state this component already has, with
   * no round trip, no re-claim and no frame of blankness. Releasing on blur would additionally
   * have to re-take the claim in a position the stack cannot express: an unfocused half of a
   * split still owns its place in it.
   *
   * Absent means *no opinion* — draw whatever was claimed, which is this bar's behaviour up to
   * M34 and what a fixture rendering it standalone wants. The default is deliberately the
   * visible one: a call site that forgets to pass this loses the feature loudly rather than
   * blanking two slots for a reason nobody can find.
   */
  editorFocused?: boolean | undefined
}

const CLAUDE_PENDING = 'Session readout arrives from the Claude statusline hook.'

/** Shown until a real readout exists, and the value the pending title keys off. */
const CLAUDE_PLACEHOLDER = 'claude · —'

/** The crumb half of the readout, which is the half that goes through React state. */
type Crumbs = Pick<ReadoutLine, 'trail' | 'pathCount' | 'file'>

const NO_CRUMBS: Crumbs = { trail: [], pathCount: 0, file: '' }

/**
 * The three fields together, because they are drawn together.
 *
 * `sameTrail` alone is not enough any more and the gap is silent: two buffers can produce the
 * *same* segments — `src › lib.rs` under two different roots is the ordinary case in this
 * repository — so a switch between them would keep the previous `file`, and every crumb would
 * reveal a row in the wrong tree.
 */
function sameCrumbs(a: Crumbs, b: Crumbs): boolean {
  return a.file === b.file && a.pathCount === b.pathCount && sameTrail(a.trail, b.trail)
}

/**
 * A drag that ended on a crumb is a selection, not a click.
 *
 * The trail became selectable text in M15 (`0033f33`, "a notice's text can be selected and
 * copied"), and copying a path out of the bar is a thing people do. Without this, releasing the
 * mouse at the end of that drag also fires the reveal.
 */
function isSelecting(): boolean {
  const selection = typeof window === 'undefined' ? null : window.getSelection()
  return selection !== null && !selection.isCollapsed
}

export function StatusBar({
  claude = CLAUDE_PLACEHOLDER,
  revealRoots,
  onRevealSegment,
  editorFocused = true,
}: StatusBarProps) {
  const readoutRef = useRef<HTMLSpanElement | null>(null)
  const [claimed, setClaimed] = useState<Crumbs>(NO_CRUMBS)

  /*
   * Read by the subscription below, which runs once and therefore closes over the prop's
   * first value for ever. A ref is the only way that callback can see the current one.
   */
  const focusedRef = useRef(editorFocused)
  focusedRef.current = editorFocused

  /**
   * What the bar draws: the claim, or nothing at all while the user is in a terminal.
   *
   * Named `crumbs` because everything below is about what is *on screen* — the claim keeps
   * its own name above. Gating here rather than at the subscription is what makes the trail
   * come back instantly when focus returns: the state was never thrown away.
   *
   * `NO_CRUMBS` is an empty trail, so `.path` renders empty and the stylesheet's `.path:empty`
   * takes the box **and its 12px gap** out of the row. That is the whole hiding mechanism —
   * a JSX branch here would leave `.path:empty` looking like dead CSS.
   */
  const crumbs = editorFocused ? claimed : NO_CRUMBS

  // Returns the unsubscriber directly, and runs once: the subscription fires immediately
  // with whatever the current editor is showing, so a bar that mounts after an editor —
  // StrictMode's second pass, a hot reload — is filled in rather than blank until the next
  // keystroke.
  useEffect(
    () =>
      subscribeStatusReadout((line) => {
        const el = readoutRef.current
        // The same gate as `crumbs` above, and it has to be applied here as well as in the
        // effect below: a readout arriving *while* a terminal is focused — a background pane's
        // outline landing, a mirrored buffer being edited — would otherwise write over the
        // slot the effect just cleared.
        if (el !== null) el.textContent = focusedRef.current ? line.detail : ''
        // Returning the previous object is what keeps a caret move out of React: the state
        // setter bails on an identical value, and these three move only when the user changes
        // file or crosses a member boundary. Without the guard this would re-render the bar on
        // every keystroke — the whole reason `detail` above goes straight into the DOM.
        setClaimed((prev) => (sameCrumbs(prev, line) ? prev : line))
      }),
    [],
  )

  /*
   * The other half of the gate, for the slot React does not own.
   *
   * `detail` is written straight into the DOM node, so nothing repaints it when a prop
   * changes — and focus moving between panes produces no readout event at all. Without this,
   * clicking into a bash pane would leave `Rust · UTF-8 · LF · Ln 7, Col 48` standing until
   * the next caret move in a buffer nobody is looking at, which is the reported bug with an
   * extra step. Reading `statusReadout()` on the way back in is what refills the slot without
   * waiting for one either.
   */
  useEffect(() => {
    const el = readoutRef.current
    if (el === null) return
    el.textContent = editorFocused ? statusReadout().detail : ''
  }, [editorFocused])

  /*
   * Which crumbs lead somewhere. One call per render of the bar, which happens when the file or
   * the caret's member changes and at no other time — the `Ln 7, Col 48` half deliberately never
   * gets here.
   *
   * The rule is `sidebar/rowPaths.ts`, which is pure and import-free and is compiled and driven
   * by two check scripts. That is not a style preference: the four bugs this project has paid
   * most for were all rules written inside a hook or an event handler, where nothing can compile
   * them. The import goes chrome → sidebar, which is the wrong direction for a component but
   * not for this: `rowPaths.ts` is pure, importless data, so the bar stays a render target that
   * pulls in no store and no React tree. (It is the last such import here — the problems
   * model's no-source sentence went with the diagnostics counters.)
   */
  const targets = crumbTargets(
    crumbs.file,
    crumbs.trail.length,
    crumbs.pathCount,
    revealRoots ?? [],
  )

  return (
    <div className={styles.bar} data-audit="statusBar">
      <div className={styles.left}>
        {/*
          * The live control, not a label. This slot used to be a `<span>` printing a `branch`
          * prop that the app never passed — so it read `⑂ —` on every launch and did nothing
          * when clicked, which is what the user reported.
          *
          * `BranchSelector` fetches its own data and owns its popup, so the bar hands it
          * nothing: a prop would be a second source of truth for a value the selector already
          * has to hold to render the list.
          */}
        <BranchSelector />

        {/*
          * `◜ Pushing…` while a push, pull, fetch or merge is in flight, and nothing at all
          * otherwise. (M65)
          *
          * Here, between the branch and the trail, because it is the branch's news: a push is
          * the thing `⑂ master` is about to stop being true of. It is also the only slot on the
          * bar that appears and disappears, so it goes next to the one item that is `flex: none`
          * and ahead of the one that gives way — the trail shrinks to make room for it, which is
          * what `.left > .path` re-grants the shrink for.
          *
          * It owns its own store, like `<BranchSelector />` above and unlike everything else
          * here. Its header says why, and `chrome/gitOpModel.ts` holds the rules a check drives.
          */}
        <GitOpIndicator />

        {/*
         * `crates › cide-core › src › lib.rs › Workspace › open_project`. Empty — and gone, along
         * with its gap — when no editor is open at all. This is the only thing on the bar allowed
         * to give way when the window narrows, and it gives way from the *left*, so the innermost
         * symbol is the last segment to go.
         *
         * # M34: beside the branch, and only while the user is in a file
         *
         * It sat in the right group until M34, and the note here used to end *"clicking into a
         * terminal leaves the last buffer's trail standing, which is what it means"*. It does not
         * mean that. The claim stack has no way to demote — a Claude or bash pane holds no editor,
         * so it claims nothing and cannot displace what is there — so the bar went on naming a
         * file the user had walked away from for as long as they typed in the terminal. That is
         * what `editorFocused` answers, and it answers it for the `Rust · UTF-8 · LF · Ln 7, Col
         * 48` slot below in the same breath: half a fix leaves a caret position on the bar for a
         * buffer nobody is in.
         *
         * The move to the left group is the other half of the same report — `where am I` belongs
         * next to `which branch`, and the right group is left holding the two readouts that are
         * actually watched. It is still the one slot that shrinks; `.left > .path` in the
         * stylesheet is what re-grants that, because the left group's blanket `flex: none`
         * would otherwise take it away silently.
         *
         * # M12: the tail is now symbols, not only path
         *
         * `EditorSurface` appends the caret's `mod › impl › fn` chain to the same array. The
         * tooltip therefore joins with `›` rather than `/`: `src/main.rs/impl Parser/parse` reads
         * as a path to a file that does not exist.
         *
         * # M16: the path half is clickable, and only the part of it that leads somewhere
         *
         * The M12 note here said this component *"deliberately does not know where the path ends
         * and the symbols begin… no crumb is clickable yet"*. It knows now — `pathCount` rides on
         * the readout claim — and that was the whole of what was missing, because without it
         * `parse` would be classified as `…/lib.rs/impl Parser/parse`, come out "inside the
         * project root", and be drawn as a live control that reports a file which does not exist.
         *
         * **An inert crumb is drawn inert, and says nothing when clicked**, because it is not
         * clickable: the user ruled out the "not in this project's file tree" notice by name for
         * this gesture, and a control that looks identical and does nothing is the defect this
         * project has now found nineteen times. So the difference is in the paint —
         * `.crumbLive` is `--dim` against the trail's `--faint`, and underlines under the pointer
         * — and `crumbTargets` is what decides which is which. On a dependency source that means
         * `home › u › .cargo › registry › src › index.crates.io-…` reads as context and
         * `serde-1.0.229 › src › de › mod.rs` reads as live, which is exactly the shape of the
         * report.
         *
         * No tab stops. `BranchSelector` is this bar's one focusable control, and six to ten new
         * focus stops for a path trail would cost more than the gap they close — the keyboard
         * route to the same place already exists and is bound (⌃⇧E, *Select Opened File*).
         * `role="link"` so the affordance is at least announced.
         */}
        <div className={styles.path} data-audit="editorPath" title={crumbs.trail.join(' › ')}>
          {crumbs.trail.map((segment, i) => {
            // No handler means no file tree in this window, and therefore no revealable crumb —
            // not a swallowed click. See `onRevealSegment`.
            const target = onRevealSegment === undefined ? null : (targets[i] ?? null)
            return (
              // Keyed by position as well as text: a path can repeat a segment
              // (`src/cide/src`), and the text alone would collide.
              <span key={`${i}:${segment}`} className={styles.crumb}>
                {i > 0 && <span className={styles.separator}>›</span>}
                {/*
                 * The label carries the affordance, not the crumb: the `›` before it belongs to
                 * neither segment, and an underline that ran through the separator would read as
                 * one long link. It cannot be pulled out into a sibling of the crumbs instead —
                 * `.path` is `direction: rtl` (that is what makes the trail clip from the left)
                 * and only `.crumb` puts it back, so a bare separator out here would be
                 * reordered by the bidi algorithm.
                 */}
                {target === null ? (
                  segment
                ) : (
                  <span
                    className={styles.crumbLive}
                    role="link"
                    onClick={() => {
                      if (isSelecting()) return
                      onRevealSegment?.(target)
                    }}
                  >
                    {segment}
                  </span>
                )}
              </span>
            )
          })}
        </div>
      </div>

      <div className={styles.right}>
        {/*
         * `Rust · UTF-8 · LF · Ln 7, Col 48`. Written from `editor/statusReadout.ts` and
         * therefore childless in JSX: giving React a child here would let the next render —
         * a branch change, a token tick — overwrite the live text.
         *
         * Inboard of the Claude readout, so the four facts that move as the user types do not
         * push the session slot around: that one is the widest thing on the bar and the one
         * whose position should stay put.
         */}
        <span className={styles.readout} ref={readoutRef} data-audit="editorReadout" />
        <span
          className={`${styles.claude} ${styles.stat}`}
          title={claude === CLAUDE_PLACEHOLDER ? CLAUDE_PENDING : 'Claude session'}
        >
          <Icon name="message-square" size={1} />
          {claude}
        </span>
        {/* The mock's `rust-analyzer` slot sits here and stays out until one is running. */}
      </div>
    </div>
  )
}
