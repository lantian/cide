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
import { Fragment } from 'react'
import { badgeLabel, badgeText } from '@/sidebar/GitPanel/model'
import { groupDigits } from '@/overlays/format'
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
   * The item's icon, as an inline path in a 24×24 viewBox, stroked not filled and drawn at
   * [`ICON_PX`].
   *
   * Required, for every item, and the note above the drawings below is why: this was once
   * `glyph` plus a per-item `size`, and a rail whose marks are characters is a rail whose sizes
   * belong to whatever face fontconfig picked.
   */
  path: string
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
 * by font size is arithmetically impossible in a 28px button: to reach 15px of ink ⌕ would need
 * a 28px font and ≡ a 26px one, in a box that is 28px including its hover background.
 *
 * So the rail draws its own marks. `GIT_PATH` had already been converted for a different reason
 * — U+2442 is in no UI font — and the comment on it argued *against* converting the rest, on
 * the grounds that half glyphs and half paths is two icon systems to keep optically matched
 * rather than one. That argument now points the other way: eight paths is one system, every
 * mark is the same box, and the size on screen is a number in this file instead of a property
 * of the fonts on the user's machine. It is still not a bundled icon set — `Explorer.tsx` cites
 * that policy and it is intact — it is eight hand-written paths on one 24×24 grid.
 *
 * Measured the same way afterwards, all eight ink boxes are **16×16 px, each centred on its
 * button's centre to the pixel**. Be clear about what that is worth: it is one host's
 * rendering, taken with a throwaway script, and **nothing in `ui/scripts/` can measure a mark**
 * — every check there is a standalone `tsc` compile or an SSR render into a string, and
 * `./run.sh --audit-chrome` measures the 28px box and the 2px gaps, never what is inside them.
 * What has *gone* is the exposure: a path draws identically on a host whose fontconfig answers
 * with other faces, which is a thing no glyph in this rail could ever promise.
 */

/*
 * The drawn box, in CSS pixels, for every icon in the rail.
 *
 * 20 in a 28px button leaves 4px of clear space on every side — clear of the button's 5px
 * corner radius, and enough that the 14px badge pill overlaps a corner of a mark rather than
 * its middle. The marks below fill about 19 of the 24 grid units, so 20px of box is 16px of
 * ink: larger than the *largest* glyph this rail ever drew (⚙, at 14px) and more than twice
 * the smallest (≡, at 7px).
 *
 * 24 was the alternative and it is better arithmetic — the viewBox would map 1:1 onto device
 * pixels, so the 2px stroke would be exactly 2px instead of the 1.667px it is at 5/6 scale —
 * but it leaves 2px of margin, and at that size the badge sits on top of a mark instead of
 * beside it. Half-integer coordinates are used for straight runs to keep what alignment 5/6
 * scaling still allows.
 */
const ICON_PX = 20

/*
 * The Git branch mark: two nodes on a trunk, a third on a limb that leaves and rejoins — the
 * shape every git UI uses, so it needs no learning.
 *
 * **Unchanged**, and it is the reference the other seven were drawn to match: 24×24 viewBox,
 * `fill="none"`, `strokeWidth` 2, round caps and joins, stroked with `currentColor` so the
 * active and inactive rail colours apply to a drawing exactly as they applied to text. Its ink
 * measures 16×16 before and after this change, because it was already a path at 20px — which
 * is the other reason `ICON_PX` is 20: the one icon nobody complained about does not move.
 */
const GIT_PATH =
  'M6 3v12M6 21a2 2 0 1 0 0-4 2 2 0 0 0 0 4M6 7a2 2 0 1 0 0-4 2 2 0 0 0 0 4'
  + 'M18 9a2 2 0 1 0 0-4 2 2 0 0 0 0 4M18 7c0 4-4 5-6 6'

/*
 * A document with a turned corner and two lines of text: the file tree.
 *
 * The corner is what makes it a document rather than a card, and it is the only notched
 * silhouette in the rail — which matters, because three of these eight marks are rectangles and
 * the rail has no labels. The two rules are 4.5 units apart so that at 20px they are two
 * strokes with a visible gap rather than one thick one, and the lower one is shorter, which is
 * how a paragraph ends.
 */
const FILES_PATH = 'M15 3.5H4V20.5H20V8.5ZM15 3.5v5h5M8 12h8M8 16.5h5'

/*
 * A magnifier: a circle of radius 6.5 with a handle off its lower right.
 *
 * The circle alone would leave this mark smaller than its straight-edged neighbours, which is
 * the usual optical problem with a round shape in a square grid; the handle runs out to the
 * corner so the whole mark fills the same 19 units as the rest. That correction belongs inside
 * the viewBox — never in a per-item size — because one `ICON_PX` for everything is the entire
 * point of this file's icons.
 */
const SEARCH_PATH = 'M10 3.5a6.5 6.5 0 1 0 0 13 6.5 6.5 0 0 0 0-13M15 15L20.5 20.5'

/*
 * A warning triangle with a bang: Problems. The only triangle in the rail.
 *
 * The dot is a zero-length subpath — `h.01` under a round cap — which is how a 2-unit dot is
 * drawn in a set where every mark is one stroked path. It sits 2.5 units below the stem: any
 * closer and the two round caps merge into a single bar at 20px, which is the failure mode of
 * every exclamation mark drawn at this size.
 */
const PROBLEMS_PATH = 'M12 3.5L20.5 20.5H3.5ZM12 9v4M12 17.5h.01'

/*
 * A hexagon: Agents. It keeps the silhouette the ⌬ (benzene ring) it replaces had, which was
 * the one thing about that glyph worth keeping, and nothing else in the rail is six-sided.
 */
const AGENTS_PATH = 'M8 3.5h8l4.5 8.5-4.5 8.5H8l-4.5-8.5Z'

/*
 * A ticked box: Tasks, and again the silhouette of the ☑ it replaces.
 *
 * The tick is inset from the box on every side by more than the stroke is wide, so the two do
 * not touch anywhere — a tick that meets the box reads as a scribble at this size rather than
 * as a tick.
 */
const TASKS_PATH =
  'M6 3.5h12a2.5 2.5 0 0 1 2.5 2.5v12a2.5 2.5 0 0 1-2.5 2.5H6a2.5 2.5 0 0 1-2.5-2.5V6'
  + 'a2.5 2.5 0 0 1 2.5-2.5ZM8 12l3 3 5-6'

/*
 * A cog: Settings. Eight teeth standing about two units proud of the root circle, and a bore.
 *
 * Drawn as one closed toothed outline rather than as a rim with eight radial strokes, which was
 * tried first and is a third of the path data. The reason is what it draws: a ring with rays is
 * Feather's *sun*, and a sun in a rail means brightness. The outline reads as a cog at 20px —
 * verified in the snapshot, because this is exactly the mark the "simplify rather than
 * reproduce" rule warns can arrive as a blob.
 */
const SETTINGS_PATH =
  'M13.6 3.5h-3.2l-.5 2.4-2.1 1.2-2.3-.8-1.6 2.8 1.8 1.6v2.6l-1.8 1.6 1.6 2.8 2.3-.8'
  + 'l2.1 1.2.5 2.4h3.2l.5-2.4 2.1-1.2 2.3.8 1.6-2.8-1.8-1.6v-2.6l1.8-1.6-1.6-2.8-2.3.8-2.1-1.2Z'
  + 'M12 9.5a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5'

/*
 * A panel with a divided region at its foot: the git tool window, which is exactly that — a
 * strip across the bottom of the workspace.
 *
 * The divider sits low, five and a half units off the bottom, so the region it marks off is the
 * one the tool window occupies. That low line is also what tells this apart from the ticked box
 * two buttons above it, since both are rounded rectangles and the rail has no labels.
 */
const TOOLWINDOW_PATH =
  'M6 4h12a2.5 2.5 0 0 1 2.5 2.5v11a2.5 2.5 0 0 1-2.5 2.5H6a2.5 2.5 0 0 1-2.5-2.5v-11'
  + 'A2.5 2.5 0 0 1 6 4ZM3.5 14.5h17'

/*
 * Four blocks with one lifted clear of the others: Extensions. (M22)
 *
 * A jigsaw piece is the conventional mark and it is the wrong one at 20px — the tab and the
 * socket are the whole silhouette and both are smaller than the stroke, so it arrives as a blob.
 * Three blocks in an L with a fourth floating above the gap reads as "a thing that plugs in" at
 * this size, and is distinguishable at a glance from the ticked box and the panel below it,
 * which are the two other rounded rectangles in this rail.
 */
const EXTENSIONS_PATH =
  'M4.5 4.5h6v6h-6ZM4.5 13.5h6v6h-6ZM13.5 13.5h6v6h-6ZM16.5 3v7M13 6.5h7'

const ITEMS: readonly RailItem[] = [
  { id: 'files', path: FILES_PATH, label: 'Files' },
  { id: 'git', path: GIT_PATH, label: 'Git' },
  { id: 'search', path: SEARCH_PATH, label: 'Search' },
  { id: 'problems', path: PROBLEMS_PATH, label: 'Problems' },
  /*
   * M18's two, and they go *before* the bottom-anchored Settings rather than after it:
   * `SPACER_AT` is a `findIndex` for the first `bottom` item, so inserting ahead of it moves the
   * spacer's index along with them and keeps it immediately before Settings, which stays pinned
   * to the foot. Appending after Settings instead would have put two buttons below the flexible
   * gap — the kind of change that looks right in the diff and wrong on screen.
   */
  { id: 'agents', path: AGENTS_PATH, label: 'Agents' },
  { id: 'tasks', path: TASKS_PATH, label: 'Tasks' },
  /*
   * M22's manager panel, and the *contributed* panels arrive separately through the `extra` prop
   * below rather than by being pushed onto this array. Two reasons, and the second is the real
   * one: this array is a module constant that `chrome/layoutAudit.ts` drives with fixed props,
   * and a set that changed when an extension was enabled would make the audit's measurements
   * depend on what the developer running it happens to have installed. The first is simply that
   * this file may not import the extension store — every value in this component is a prop, which
   * is the rule its header states.
   */
  { id: 'extensions', path: EXTENSIONS_PATH, label: 'Extensions' },
  { id: 'settings', path: SETTINGS_PATH, label: 'Settings', bottom: true },
]

/**
 * The one drawing surface in this rail, so its two call sites cannot drift apart in box size,
 * stroke weight or cap shape — the same reason the spans it replaces shared a class, and the
 * same argument: two rail buttons that draw their icon differently is a worse rail than one
 * that draws every icon wrongly.
 *
 * `currentColor` is what makes the rail's inherited `--dim`, `.item:hover`'s `--text` and
 * `.itemActive`'s `--accent` apply to a drawing exactly as they applied to a character. `aria-hidden`, because
 * each button's `aria-label` is already its whole accessible name.
 *
 * The `<svg>` is the button's only child and `.item` is a `place-items: center` grid, so it is
 * blockified and centred as a box — no line box, no baseline, and therefore none of the
 * per-face vertical drift that `ActivityRail.module.css` used to have a rule about.
 */
function RailIcon({ path }: { path: string }) {
  return (
    <svg
      aria-hidden="true"
      viewBox="0 0 24 24"
      width={ICON_PX}
      height={ICON_PX}
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d={path} />
    </svg>
  )
}

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
}

export function ActivityRail({
  active,
  changed,
  errors,
  agents,
  agentsAwaiting,
  tasks,
  onSelect,
  toolWindowOpen,
  onToggleToolWindow,
  extra,
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
    <div className={styles.rail} data-audit="rail" role="group" aria-label="Activity">
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
      {items.map((item, i) => {
        const selected = item.id === active
        const badge = badges[item.id]
        const name = badge === undefined ? item.label : `${item.label} — ${badge.name}`
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
              <RailIcon path={item.path} />
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
        <RailIcon path={TOOLWINDOW_PATH} />
      </button>
    </div>
  )
}
