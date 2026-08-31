/**
 * The 42px activity rail down the left edge, below the header.
 *
 * The rail behaves like a vertical tablist — one selection at a time, each button swapping
 * the sidebar's contents — so it is marked up as one rather than as a toolbar. The items are
 * a data array rather than five hand-written blocks because later milestones add entries and
 * per-item adornments (the git count here) without disturbing the layout rules.
 *
 * Every value it draws is a prop and it reads no store, which is the rule for every component
 * in `chrome/`: it is what lets `chrome/layoutAudit.ts` drive each surface with fixed props
 * and measure the result. The git count is subscribed in `App.tsx` and passed down for that
 * reason and no other.
 */
import { Fragment, useCallback, useEffect, useRef, useState } from 'react'
import { badgeLabel, badgeText } from '@/sidebar/GitPanel/model'
import { groupDigits } from '@/overlays/format'
import { Icon, type IconName, type IconSize } from '@/icons/Icon'
import { useContextMenu } from '@/menus'
import { railFit, railOverflowHint } from './railOverflow'
import type { ActivityView } from './sidebarView'
import styles from './ActivityRail.module.css'

/*
 * The type moved to `chrome/sidebarView.ts` in M14 and is re-exported here so the half-dozen
 * existing import sites did not have to move with it. It lives there because the *rules* about
 * it do — which view has a panel, which one can be restored, what a click means — and that
 * module is import-free so `check-sidebar.mjs` can compile and execute them. This component
 * still only draws.
 */
export type { ActivityView }

export interface RailItem {
  id: ActivityView
  /**
   * The item's mark: a name from the vendored set, or a raw 24×24 path.
   *
   * Two fields because there are genuinely two provenances. A built-in item names a mark and is
   * checked at compile time; an **extension** supplies `path`, a raw `d` that
   * `cide_ext::manifest::is_svg_path` has already validated on the Rust side and which no
   * TypeScript union could ever narrow. `<Icon>` collapses both to one `<path>`, so the code
   * that draws an extension's icon is the same code that draws every built-in one.
   *
   * One of the two is required, and the note above the drawings below is why: this was once
   * `glyph` plus a per-item `size`, and a rail whose marks are characters is a rail whose sizes
   * belong to whatever face fontconfig picked.
   */
  icon?: IconName
  path?: string
  label: string
  /* Renders after the flexible spacer, pinned to the foot of the rail. */
  bottom?: boolean
}

/*
 * Why every icon in this rail is a drawn path, and why they are all one size.
 *
 * The rail was seven Unicode characters at seven font sizes, and it was reported twice: first
 * as "icons are quite small", answered by adding 4px to every `size`, and then — because that
 * answer could not hold — as "why all icons of different size? They should be one size and
 * bigger". Font size was never the variable. What a character puts on screen is its **ink
 * inside its em box**, and that ratio belongs to the face fontconfig picked, not to us.
 * Rendered offscreen in the engine the app actually uses (WebKitGTK 4.1, this host's fonts)
 * and read out of the pixels, at the sizes then in force:
 *
 *     ▤ files 18px → 14×13 ink    ⌕ search 17px → 10×9     ⚑ problems 17px → 12×12
 *     ⌬ agents 17px → 12×13       ☑ tasks 16px → 12×12     ⚙ settings 17px → 14×14
 *     ≡ tool window 14px → 7×8
 *
 * — 0.53 to 0.82 em of ink, so ⚙ drew nearly twice the mark ≡ did while their font sizes
 * differed by 3px, and the seven `size` values were doing almost nothing. Equalising *upwards*
 * by font size is arithmetically impossible in a 28px button (the size then): to reach 15px of ink ⌕ would need
 * a 28px font and ≡ a 26px one, in a box that is 28px including its hover background.
 *
 * So the rail drew its own marks: eight hand-written paths on one 24×24 grid, `fill="none"`,
 * `stroke="currentColor"`, `stroke-width="2"`, round cap and join. `GIT_PATH` had already been
 * converted for a different reason — U+2442 is in no UI font — and the comment on it argued
 * *against* converting the rest, on the grounds that half glyphs and half paths is two icon
 * systems to keep optically matched rather than one. That argument pointed the other way as
 * soon as the drawings existed: eight paths is one system, and the size on screen became a
 * number in this file instead of a property of the fonts on the user's machine.
 *
 * **And then it pointed further.** The eight paths fixed the rail and left the git toolbar, the
 * pane title bar, the status bar, six panel headers and every dialog still drawing characters —
 * so the app had exactly the two-systems problem this comment was written to avoid, one rail
 * larger. This file used to end by saying "it is still not a bundled icon set, and `Explorer.tsx`
 * cites that policy". That policy is reversed: `ui/src/icons/iconPaths.ts` is a vendored Lucide
 * set, ISC, pinned by revision, and `Explorer.tsx` and `GitPanel/Toolbar.tsx` say so too now.
 *
 * The rail lost nothing in the move. Lucide's envelope is the one above, attribute for
 * attribute — the eight paths were drawn to that convention in the first place — and `--icon-3`
 * is pitched at the 1.667px stroke this rail already had. What it gained is that the same marks
 * are now available to the other thirty-odd surfaces, and that a name is checked at compile
 * time where a `d` string never was.
 *
 * Measured after the hand-drawn conversion, all eight ink boxes were **16×16 px, each centred on
 * its button's centre to the pixel**. Be clear about what that is worth: it was one host's
 * rendering, taken with a throwaway script, and **nothing in `ui/scripts/` can measure a mark**
 * — every check there is a standalone `tsc` compile or an SSR render into a string, and
 * `./run.sh --audit-chrome` measures the button box and the gaps, never what is inside them.
 * The vendored set was checked differently and more strongly: every flattened path was
 * rasterised at 192×192 and compared pixel-for-pixel against upstream's own multi-element SVG.
 * What has *gone*, either way, is the exposure: a path draws identically on a host whose
 * fontconfig answers with other faces, which is a thing no glyph in this rail could ever promise.
 */

/*
 * The rung every mark in this rail is drawn at.
 *
 * `--icon-3` is 20px, and 20 in a 32px button leaves 6px of clear space on every side — clear of
 * the button's 8px corner radius, and enough that the 16px badge pill overlaps a corner of a
 * mark rather than its middle. Lucide's marks fill about 19 of the 24 grid units, so 20px of box
 * is roughly 16px of ink: larger than the *largest* glyph this rail ever drew (⚙, at 14px) and
 * more than twice the smallest (≡, at 7px).
 *
 * The button grew from 28 to 32 and this **deliberately did not follow**. 24 has always been the
 * better arithmetic — the viewBox would map 1:1 onto device pixels, so the stroke would be
 * exactly 2px instead of the 1.667px it is at 5/6 scale — and at a 32px button the old objection
 * to it (2px of margin, with the badge landing on a mark rather than beside it) no longer holds.
 * It is still 20, because this is the one icon set in the app nobody has ever reported: the
 * complaints were "too small" and "all different sizes", both about a rail of *characters*, and
 * both already answered. Growing the artwork now would be a change with no report behind it, on
 * the only surface that was already right. That 1.667px stroke is also what `Icon.module.css`
 * pitches every other size to match, so leaving it makes the rail the reference the rest of the
 * app is measured against rather than an exception to it.
 */
const RAIL_ICON: IconSize = 3

/**
 * What a contributed panel draws when its manifest supplies no icon.
 *
 * `cide_ext::manifest` already warns the *author* that "its button will be blank", which is the
 * right thing to tell them and no help at all to the user looking at an empty 32px button on
 * their rail. A jigsaw piece reads as "something plugged in" and is distinct from the manager
 * panel's own mark — which is why it is `puzzle` and not `blocks`: an extension wearing the
 * Extensions button's icon looks like a second Extensions panel.
 */
const EXTENSION_FALLBACK: IconName = 'puzzle'

const ITEMS: readonly RailItem[] = [
  { id: 'files', icon: 'file-text', label: 'Files' },
  { id: 'git', icon: 'git-branch', label: 'Git' },
  { id: 'search', icon: 'search', label: 'Search' },
  { id: 'problems', icon: 'triangle-alert', label: 'Problems' },
  /*
   * M18's two, and they go *before* the bottom-anchored Settings rather than after it:
   * `SPACER_AT` is a `findIndex` for the first `bottom` item, so inserting ahead of it moves the
   * spacer's index along with them and keeps it immediately before Settings, which stays pinned
   * to the foot. Appending after Settings instead would have put two buttons below the flexible
   * gap — the kind of change that looks right in the diff and wrong on screen.
   */
  { id: 'agents', icon: 'hexagon', label: 'Agents' },
  { id: 'tasks', icon: 'square-check-big', label: 'Tasks' },
  /*
   * M28's, and it goes here for the reason the comment above gives — before the bottom-anchored
   * Settings, so `SPACER_AT` moves with it.
   *
   * **Always drawn, even in a project with no `openspec/`.** This array is a module constant
   * precisely so `chrome/layoutAudit.ts` can drive it with fixed props; a rail whose buttons
   * depended on what a project happened to contain would make the audit's measurements depend on
   * the repository the developer had open. So the button is permanent and unbadged, and it *is*
   * the one quiet entry point — everything else the feature adds stays hidden until the board is
   * ready.
   */
  { id: 'openspec', icon: 'book-open-text', label: 'OpenSpec' },
  /*
   * M22's manager panel, and the *contributed* panels arrive separately through the `extra` prop
   * below rather than by being pushed onto this array. Two reasons, and the second is the real
   * one: this array is a module constant that `chrome/layoutAudit.ts` drives with fixed props,
   * and a set that changed when an extension was enabled would make the audit's measurements
   * depend on what the developer running it happens to have installed. The first is simply that
   * this file may not import the extension store — every value in this component is a prop, which
   * is the rule its header states.
   */
  { id: 'extensions', icon: 'blocks', label: 'Extensions' },
  { id: 'settings', icon: 'settings', label: 'Settings', bottom: true },
]

/* The spacer belongs immediately before the first pinned item, wherever the array puts it. */
const SPACER_AT = ITEMS.findIndex((item) => item.bottom === true)

export interface ActivityRailProps {
  active: ActivityView | null
  /**
   * Changed files across every repository in the project — `null` while none has been counted
   * yet, which is not the same as `0` and is drawn the same way for a different reason.
   *
   * A number rather than the `gitDirty` boolean this replaced. That boolean had exactly one
   * call site and it was `gitDirty={auditMode()}`: the badge was wired to the layout-audit
   * query flag and to nothing else, so in a normal launch it never rendered in any repository
   * state. See `chrome/gitCountStore.ts` for where the number now comes from.
   */
  changed?: number | null | undefined
  /**
   * Errors in the project, for the badge on the warning triangle. (M12)
   *
   * `null` means **nobody has looked** — no analyser is running, or none has answered yet — and
   * it draws no badge at all. That is the same distinction the panel and the status bar make, and
   * making it here too is what stops the rail from being the one surface that implies a clean
   * workspace: a `0` and a `null` must not look alike, so only a positive count draws anything.
   */
  errors?: number | null | undefined
  /**
   * Whether any analyser is working right now, for a busy dot on the warning triangle. (M25)
   *
   * A boolean and not a count, because "how many analysers are scanning" is not a number a
   * user acts on — the fact is "cide is still looking", and it matters exactly when the
   * user is waiting on the answer. Drawn as a small pulsing dot **only when the error pill
   * is absent**: the pill is the call to action and wins the one badge slot a 42px button
   * has; while both are true the accessible name still says both, because a screen reader
   * has no such space constraint.
   */
  busy?: boolean | undefined
  /**
   * Subagent runs that are live — running, or stopped waiting for the user — for the badge on
   * the hexagon.
   * (M18)
   *
   * The `errors` rule above, exactly, and it is the rule and not a preference: `null` means
   * **nobody has looked** — subagents are off for this project, or the roster has not answered
   * yet — and `0` means looked, and nothing is running. Both draw no badge, because in neither
   * case is there anything for the user to do, and a badge is a call to action. What must never
   * happen is the two being drawn *alike as a number*: a `0` pill would tell a user with the
   * feature switched off that cide had checked and found their agents idle.
   */
  agents?: number | null | undefined
  /**
   * Whether any of those runs is stopped at a permission prompt, which only changes the badge's
   * **tone**, never whether it is drawn or what number it shows. (M18)
   *
   * A separate value rather than a second count because it answers a different question. The
   * count is "how much is in flight", which the pill shows; this is "is any of it stuck on me",
   * which is the one agent state that is a call to action rather than a status — the run has
   * stopped, and it stays stopped until the user answers. So it draws the same red the Problems
   * uses for errors. Folding it into the count instead would mean either drawing the wrong
   * number or drawing two pills on one 42px button.
   *
   * It cannot make a badge appear on its own: with `agents` `null` or `0` there is no pill to
   * tone, and a caller that says "one run is waiting" while saying "no runs are live" has
   * contradicted itself — the rail believes the count, because that is the number on screen.
   */
  agentsAwaiting?: boolean | undefined
  /**
   * Open tasks in `.cide/tasks.json`, for the badge on the ticked box. (M18)
   *
   * Same rule again, and here the `null` is the common case rather than the exotic one: a
   * project with no tracker file has nothing to count, which is *not* the same fact as a
   * tracker whose every task is done. The second of those is worth a quiet, unbadged button;
   * the first must not imply cide looked at a board that does not exist.
   */
  tasks?: number | null | undefined
  onSelect?: ((view: ActivityView) => void) | undefined
  /**
   * Whether the git tool window is showing, for the bottom-most button's pressed state.
   *
   * Optional like every other value here, so `chrome/layoutAudit.ts` and any fixture can keep
   * driving this component with fixed props — the rule this file's header states.
   */
  toolWindowOpen?: boolean | undefined
  onToggleToolWindow?: (() => void) | undefined
  /**
   * Buttons contributed by extensions, in registry order. (M22)
   *
   * Inserted immediately before the spacer, so they sit under Extensions and above the pinned
   * Settings — the position M18's two took for the same reason, which `ITEMS` records: appending
   * after Settings would put buttons *below* the flexible gap, a change that looks right in the
   * diff and wrong on screen.
   *
   * A prop and not a module constant, because unlike every other button here the set is not known
   * until `extensions.json` has been read. Optional, so `chrome/layoutAudit.ts` and every fixture
   * keep driving this component with fixed props and measure a rail that does not depend on what
   * the developer running the audit has installed.
   */
  extra?: readonly RailItem[] | undefined
  /**
   * Contributed panels the user has taken *off* the rail in Settings. (M31)
   *
   * Never drawn as buttons, and always listed in the `···` menu — which is the whole reason
   * they are a prop here rather than simply being filtered out of [`extra`] and forgotten.
   * Dropping them would make the setting a one-way door: a rail button is the only route into
   * a contributed panel, so "hide the icon" would silently mean "you can never open this
   * again", and the way back would be a Settings page the user has no reason to connect with
   * a panel that has vanished.
   *
   * So `···` means *panels not in the strip*, whichever reason put them there — the window is
   * too short, or the user asked. One control, one sentence, and the setting stays a
   * preference about the icon strip instead of a destructive act.
   */
  extraHidden?: readonly RailItem[] | undefined
}

export function ActivityRail({
  active,
  changed,
  errors,
  busy,
  agents,
  agentsAwaiting,
  tasks,
  onSelect,
  toolWindowOpen,
  onToggleToolWindow,
  extra,
  extraHidden,
}: ActivityRailProps) {
  /*
   * The builtins, with the contributed buttons spliced in ahead of the spacer.
   *
   * `SPACER_AT` is a `findIndex` over `ITEMS`, so the splice has to happen before the index is
   * used or the gap lands in the middle of the extension buttons. Recomputed rather than
   * adjusted, which is one line and cannot be off by one.
   */
  const items: readonly RailItem[] =
    extra === undefined || extra.length === 0
      ? ITEMS
      : [...ITEMS.slice(0, SPACER_AT), ...extra, ...ITEMS.slice(SPACER_AT)]
  const spacerAt = items.findIndex((item) => item.bottom === true)

  /*
   * How many of the top buttons fit, and the menu for the ones that do not. (M31)
   *
   * The arithmetic is `railOverflow.railFit`, which is a function of the container and of two
   * CSS lengths and nothing else — see its header for why measured boxes would be a feedback
   * loop here where they are the only honest input on the tab strip.
   *
   * The lengths are read off the DOM rather than written as `32` and `4` in this file. They are
   * declared in `ActivityRail.module.css`, whose comments tune them (32px "from 28", the gap is
   * `--sp-2`), and a second copy here would be a rail that miscounts by a slot the first time
   * anybody retunes the stylesheet — silently, and only on a short window, which is the one
   * place nobody looks.
   */
  const railEl = useRef<HTMLDivElement | null>(null)
  const [shown, setShown] = useState<number>(spacerAt)
  const flexible = spacerAt
  // Settings and whatever else sits after the spacer, plus the tool-window toggle below the
  // tablist. Never collapsed: the menu is reached through the rail, so a rail that could clip
  // its own last controls could clip the way out of its own overflow.
  const pinned = items.length - spacerAt + 1
  const measure = useCallback(() => {
    const box = railEl.current
    if (box === null) return
    const first = box.querySelector<HTMLElement>('[data-audit="railIcon"]')
    if (first === null) return
    const style = window.getComputedStyle(box)
    const padding = parseFloat(style.paddingTop) + parseFloat(style.paddingBottom)
    const gap = parseFloat(style.rowGap)
    const next = railFit({
      available: box.clientHeight - (Number.isFinite(padding) ? padding : 0),
      item: first.offsetHeight,
      gap: Number.isFinite(gap) ? gap : 0,
      flexible,
      pinned,
    })
    // Identity is already a primitive here, but the guard still matters: the effect below runs
    // on every commit with no dependency array, and an unconditional `setShown` would be a
    // state write per render — a loop that shows up only as the app pinning a core.
    setShown((prev) => (prev === next ? prev : next))
  }, [flexible, pinned])

  /*
   * Trigger 1: every commit, after paint. No dependency array, for `TabStrip`'s reason — the
   * things that change the answer are not enumerable (a contributed panel arrives, `--ui-scale`
   * moves the header and status bar, the window is tiled) and a dependency array is how this
   * goes stale. Stale is worse than absent: it lists a panel that is on screen, or hides while
   * three are not.
   *
   * `useEffect` and not `useLayoutEffect`, also for `TabStrip`'s reason: a layout effect reads
   * geometry while the commit that triggered it has just dirtied style across the document, so
   * it forces a full synchronous layout of the window on every render. After paint the same
   * reads cost nothing, at the price of the control updating one painted frame late.
   */
  useEffect(measure)

  // Trigger 2: the rail changing size without re-rendering — the window resized, a detached
  // window tiled, the status bar growing under `--ui-scale`. None of those is a commit here.
  useEffect(() => {
    const box = railEl.current
    if (box === null || typeof ResizeObserver === 'undefined') return
    const observer = new ResizeObserver(measure)
    observer.observe(box)
    return () => observer.disconnect()
  }, [measure])

  const hidden = [
    ...items.slice(Math.min(shown, flexible), flexible),
    ...(extraHidden ?? []),
  ]
  const overflowAnchor = useRef<HTMLButtonElement | null>(null)
  /*
   * `items` is called at open time, so the list is built from the measurement as it stands when
   * the user presses rather than from whatever was true at the last render — `TabStrip`'s
   * overflow menu makes the same point.
   *
   * Reusing `useContextMenu` rather than writing a popup brings the keyboard model, the
   * dismiss-on-scroll/resize/blur handling and `placeMenu`'s edge flipping with it, and it
   * portals to the app root so no `overflow: hidden` ancestor can clip it — which matters here
   * more than anywhere, because the ancestor that clips this rail is the reason the control
   * exists.
   */
  const overflow = useContextMenu({
    label: 'Panels not in view',
    items: () =>
      hidden.map((item) => ({
        // Prefixed for `overflowEntries`' reason: a bare view id would read as though the entry
        // *were* the panel rather than a way to reach it.
        id: `rail-overflow:${item.id}`,
        label: item.label,
        ...(onSelect
          ? { run: () => onSelect(item.id) }
          : // `chrome/layoutAudit.ts` renders a handler-free rail; a live-looking row that does
            // nothing on click is the failure the menu model exists to make unrepresentable.
            { disabled: true }),
      })),
  })
  /*
   * Keyed by view id so the loop stays layout-only; more entries land here, not in JSX.
   *
   * Two values per entry, because the badge and its wording are no longer the same string: at
   * a hundred changed files the pill says `99+` and the button is still named "Git — 1,203
   * changed files". Both come from `GitPanel/model.ts` — the same module the panel's own repo
   * rows count with — so the rail and the panel cannot disagree about a number they both show.
   * The comma is this component's only contribution, because `model.ts` may not import
   * `groupDigits` and stay standalone-compilable for `check-git-tree.mjs`.
   */
  const count = changed ?? null
  const badges: Record<
    string,
    { pill: string; name: string; pillClass?: string | undefined } | undefined
  > = {
    git: (() => {
      const pill = badgeText(count)
      const name = count === null ? undefined : badgeLabel(count, groupDigits(count))
      return pill === null || name === undefined ? undefined : { pill, name }
    })(),
    // Only a positive, *known* count. `null` (nothing looked) and `0` (looked, clean) both draw
    // nothing — a badge is a call to action, and there is nothing to act on in either case.
    problems:
      typeof errors === 'number' && errors > 0
        ? {
            pill: badgeText(errors) ?? String(errors),
            pillClass: styles.badgeError,
            name: `${groupDigits(errors)} ${errors === 1 ? 'error' : 'errors'}`,
          }
        : undefined,
    /*
     * The same shape as `problems` above, deliberately down to the `typeof … && … > 0` — the
     * distinction it encodes (nobody looked / looked and idle / here is a number) is the one
     * every count surface in this app has to make, and the third copy of it is the point at
     * which it is a rule rather than a habit.
     *
     * The one thing that is new is the tone. A run at a permission prompt has *stopped*, and it
     * stays stopped until the user answers, so it gets the red the Problems badge uses; a run
     * merely working gets the ordinary pill and can be ignored. The accessible name says which,
     * because colour is not a name — a screen reader would otherwise be told "Agents — 2 runs"
     * in both states, which is the half of the fact that does not matter.
     */
    agents:
      typeof agents === 'number' && agents > 0
        ? {
            pill: badgeText(agents) ?? String(agents),
            pillClass: agentsAwaiting === true ? styles.badgeError : undefined,
            name:
              agentsAwaiting === true
                ? `${groupDigits(agents)} ${agents === 1 ? 'run' : 'runs'}, waiting for you`
                : `${groupDigits(agents)} ${agents === 1 ? 'run' : 'runs'}`,
          }
        : undefined,
    tasks:
      typeof tasks === 'number' && tasks > 0
        ? {
            pill: badgeText(tasks) ?? String(tasks),
            name: `${groupDigits(tasks)} open ${tasks === 1 ? 'task' : 'tasks'}`,
          }
        : undefined,
  }

  return (
    <div
      ref={railEl}
      className={styles.rail}
      data-audit="rail"
      role="group"
      aria-label="Activity">
      {/*
       * `display: contents`, so the tab buttons and the spacer are still direct flex children of
       * `.rail`: the 42px width, the 8px padding, the 2px gap and the chrome audit's
       * `siblingGap(railIcon)` measurement are all bit-identical to before this wrapper existed.
       *
       * What the wrapper buys is that the tablist owns nothing but tabs. The tool-window button
       * below is **not** one: it toggles a panel that is orthogonal to the sidebar, and a rail
       * button claiming `aria-selected` while doing that would tell a screen reader that Files
       * had been deselected — which is exactly the thing that must not happen, because Files
       * stays lit. Two tablists (a top one and a bottom one) was the alternative and it is worse:
       * one selection modelled as two lists is harder to read than one list plus a button.
       */}
      <div
        className={styles.tabs}
        role="tablist"
        aria-orientation="vertical"
        aria-label="Sidebar panels"
      >
      {/*
       * The buttons that fit, plus everything from the spacer down. `shown` only ever bites
       * into the flexible run above the spacer — see `pinned` in `railFit`.
       */}
      {items.map((item, i) => {
        // Collapsed into the overflow menu below. Only the flexible run above the spacer is
        // ever dropped; `railFit`'s `pinned` is what keeps Settings and the tool-window toggle
        // out of it, so the way *out* of an overfull rail can never itself be clipped.
        if (i >= shown && i < flexible) return null
        const selected = item.id === active
        const badge = badges[item.id]
        const busyHere = item.id === 'problems' && busy === true
        let name = badge === undefined ? item.label : `${item.label} — ${badge.name}`
        if (busyHere) {
          // The name carries the busy fact even when the pill claims the visual slot.
          name = badge === undefined ? `${item.label} — analysing…` : `${name}, analysing…`
        }
        return (
          <Fragment key={item.id}>
            {/* Hidden from the accessibility tree: a tablist should own nothing but tabs,
                and this filler carries no meaning. */}
            {i === spacerAt && <div className={styles.spacer} aria-hidden="true" />}
            <button
              type="button"
              role="tab"
              aria-selected={selected}
              /* The icon carries no text, so the tooltip is the only visible name and the
                 label is the only name assistive technology gets. */
              aria-label={name}
              title={name}
              className={selected ? `${styles.item} ${styles.itemActive}` : styles.item}
              data-audit="railIcon"
              onClick={() => onSelect?.(item.id)}
            >
              <Icon
                {...(item.path !== undefined && item.path !== ''
                  ? { d: item.path }
                  : { name: item.icon ?? EXTENSION_FALLBACK })}
                size={RAIL_ICON}
              />
              {busyHere && badge === undefined && (
                /* The pulsing busy dot, in the pill's anchor corner. `aria-hidden` for the
                   badge's reason: the name above already says "analysing…". */
                <span className={styles.busyDot} data-audit="problemsBusy" aria-hidden="true" />
              )}
              {badge !== undefined && (
                /*
                 * `aria-hidden`, and it is not cosmetic. The badge used to be a 6px dot with
                 * no text, so it needed hiding from nothing; it now has content, and without
                 * this a screen reader reads the button as "Git — 3 changed files, 3" — and
                 * at the cap as "Git — 1,203 changed files, 99+", which is the one rendering
                 * of that number that means nothing at all. The name above is the wording.
                 */
                <span
                  className={`${styles.badge} ${badge.pillClass ?? ''}`}
                  data-audit="gitBadge"
                  aria-hidden="true"
                >
                  {badge.pill}
                </span>
              )}
            </button>
          </Fragment>
        )
      })}
      </div>
      {/*
       * The way into whatever did not fit. (M31)
       *
       * Drawn only when something is actually hidden, so a rail with room is unchanged — which
       * is what keeps `CIDE_AUDIT=1`'s measurements (taken at a forced 1440×900) exactly as
       * they were.
       *
       * # Why it sits here and not next to the buttons it stands for
       *
       * Because the tablist above must own nothing but tabs, which is the rule that put the
       * tool-window toggle out here in the first place and it is not weaker for this control:
       * a `<button>` that opens a menu is not a tab, and one sitting inside a `tablist` tells a
       * screen reader that the panel list contains something that cannot be selected. The
       * spacer that pushes Settings to the foot lives *inside* the tablist (it has to — the
       * wrapper is `display: contents`, so its children are the rail's own flex items, in
       * order), so anything rendered after the wrapper necessarily lands at the bottom.
       *
       * That is also where it reads best: the foot of the rail is where the controls that are
       * always present already live, and it is where a reader looks when the list above them
       * has visibly run out of room.
       */}
      {hidden.length > 0 && (
        <button
          type="button"
          ref={overflowAnchor}
          className={styles.item}
          data-audit="railIcon"
          data-audit-role="railOverflow"
          // The count lives in the name and never on the glyph — a label that grew a digit
          // would change this control's height, which changes what fits, which changes the
          // digit. `railOverflow.railOverflowHint` is the shared wording.
          title={railOverflowHint(hidden.length)}
          aria-label={railOverflowHint(hidden.length)}
          aria-haspopup="menu"
          aria-expanded={overflow.isOpen}
          onClick={() => {
            const el = overflowAnchor.current
            if (el !== null) overflow.openFor(el)
          }}
        >
          {/* Already in the vendored set, so this adds no mark — `check:ui-icons` keeps
              that set closed in both directions. The ellipsis is the "there is more here"
              mark the rest of the app already uses. */}
          <Icon name="ellipsis" size={RAIL_ICON} />
        </button>
      )}
      {/*
       * The git tool window's toggle. Renders after the wrapper, so it is the bottom-most item in
       * the rail — the spacer that pushes ⚙ down lives inside the tablist, and anything after the
       * wrapper falls below it.
       *
       * `aria-pressed`, not `aria-selected`, and that is the whole reason it is out here: this
       * opens a panel *below* the workspace while the sidebar keeps whatever it was showing, so
       * it is a toggle button and not a tab. It also deliberately does not join `ActivityView` —
       * folding a sixth id in there would make `selectView` treat it as a panel, un-light Files
       * when it is clicked, and break the standalone compile `check-sidebar.mjs` depends on.
       */}
      <button
        type="button"
        aria-pressed={toolWindowOpen === true}
        aria-label="Git tool window"
        title="Git tool window"
        className={
          toolWindowOpen === true ? `${styles.item} ${styles.itemActive}` : styles.item
        }
        data-audit="railIcon"
        data-audit-role="toolWindow"
        onClick={() => onToggleToolWindow?.()}
      >
        {/* `RailIcon` for the same reason the tab buttons use it: this button is not a tab, but
            it is the same 28px box with the same mark in it, and one component is what keeps
            that true. It was the worst offender before — ≡ at 14px drew 7×8 of ink against
            ⚙'s 14×14, which is the report this change answers. */}
        <Icon name="panel-bottom" size={RAIL_ICON} />
      </button>
      {overflow.menu}
    </div>
  )
}
