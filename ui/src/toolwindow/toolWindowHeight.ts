/**
 * The parts of "resize the bottom tool window and remember it" that are decisions rather than
 * side effects: the clamp, the token, the viewport ceiling, and the boot cache's encoding.
 *
 * The vertical twin of `chrome/sidebarWidth.ts`, and split out for the same reason: there is no
 * JS test runner in this project and the app must never be launched to look at a layout, so
 * everything that can be wrong on its own is pulled into a module `scripts/check-toolwindow.mjs`
 * can compile with `tsc` and assert against. Values in, values out — reading `window.innerHeight`,
 * writing a custom property and touching `localStorage` stay with the caller.
 *
 * Deliberately import-free, including of `@/ipc/client`: the check compiles this one file on its
 * own, and a single `import type` of the generated DTOs would drag the path alias and the whole
 * IPC surface in behind it. [`StoredToolWindow`] is therefore a structural restatement of
 * `ToolWindowState` from `generated.ts` rather than that type — `geometryFromState` is called
 * with the real one, so a rename on the Rust side still fails the build.
 *
 * # Why it is a "tool window" and not a "dock"
 *
 * **"Dock" is taken.** In this codebase it means re-docking a *detached pane* — `DockAnchor`,
 * `DockSibling`, `Project::dock_anchors`, `windows/detachedPane.ts`, `DetachedPaneWindow`'s
 * `redock`. A module here called `dockHeight.ts` would make every reader of `dock_anchors`
 * misparse it, and the two concepts never touch. IDEA's own word is *tool window*, this repo's
 * README and ADRs already use it in prose, so the code uses it too.
 */

/** The custom property the height is driven through. */
export const TOOL_TOKEN = '--h-toolwindow'

/**
 * The default height, in CSS pixels.
 *
 * Also the literal `tokens.css` ships and `ToolWindowState`'s `Default` impl in
 * `crates/cide-ipc/src/workspace.rs`. Three copies of one number, pinned against each other by
 * `check-toolwindow.mjs`, because the failure of letting them drift is that the panel opens at
 * the wrong size on first launch and then never again — invisible, and therefore never reported.
 */
export const TOOL_DEFAULT = 260

/**
 * The floor, in CSS pixels. Mirrors `TOOL_WINDOW_MIN_HEIGHT` in `cide-ipc`.
 *
 * 120 because that is what the panel's own furniture costs before it shows anything: a 28px tab
 * row, a 30px filter bar, and enough left for a header row and two commits at 22px each. Below
 * it the panel is not tight, it is empty — and an empty panel reads as a broken one rather than
 * as a small one.
 */
export const TOOL_MIN = 120

/** The stored ceiling. What actually *fits* is [`toolWindowCeiling`]'s job, not this one. */
export const TOOL_MAX = 900

/**
 * The chrome rows this panel shares its column with, at the *design* chrome font size.
 *
 * Design sizes, because all three tokens are `calc(<this> * var(--ui-scale))` since the chrome
 * gained a font-size setting — see `tokens.css`. What a window actually reserves is these times
 * the live scale, which is why [`toolWindowCeiling`] takes one rather than summing them flat.
 */
/** `--h-header`. */
export const HEADER_HEIGHT = 38
/** `--h-tabstrip`. */
export const TABSTRIP_HEIGHT = 34
/** `--h-status`. */
export const STATUS_HEIGHT = 26

/**
 * The chrome above and below the pane area, which the tool window shares the column with.
 *
 * Summed here rather than at the call site so `check-toolwindow.mjs` can pin the total against
 * the three tokens in `tokens.css`. The assertion exists because the failure mode of adding a
 * chrome row and forgetting this constant is that the ceiling is one row too generous — so the
 * panel is allowed to squeeze the panes below their floor, on tall windows only, which is
 * exactly where nobody looks for it.
 */
export const CHROME_HEIGHT = HEADER_HEIGHT + TABSTRIP_HEIGHT + STATUS_HEIGHT

/**
 * `--w-splitter`, used here as a height.
 *
 * Subtracted by [`toolWindowCeiling`] because the handle is a **real flex item in the same
 * column**, so its pixels come out of the pane area just as the panel's do. Leaving it out
 * under-delivers [`MIN_PANES`] by exactly that much — the same drift `SPLITTER_WIDTH`'s comment
 * records for the horizontal case.
 */
export const SPLITTER_HEIGHT = 3

/**
 * What the pane grid keeps, whatever the tool window asks for.
 *
 * The vertical `MIN_WORKSPACE`. 200 rather than the horizontal 360 because a pane is useful much
 * shorter than it is narrow: a terminal at 200px is about ten rows, which is a usable shell, while
 * a 200px-wide one wraps every path.
 */
export const MIN_PANES = 200

/**
 * The tallest the tool window may be in a window this tall, in CSS pixels.
 *
 * `available` is `window.innerHeight`. Omitted — which is what a check with no DOM passes — the
 * static [`TOOL_MAX`] applies and the viewport does not constrain anything.
 *
 * **The floor wins on a short window**, and that is the decision rather than an accident of
 * `Math.max`: at this app's narrowest legal window (`MIN_HEIGHT` is 430 in
 * `crates/cide-app/src/windows.rs`) the room left after the chrome, the splitter and
 * [`MIN_PANES`] is 126px, which is above [`TOOL_MIN`] — so the band is never empty. Were it ever
 * to go negative, a 40px tool window is not a compromise, it is a broken panel, and the user can
 * make the window taller. `check-toolwindow.mjs` pins that 126 against `MIN_HEIGHT`, so a change
 * to either number has to be made on purpose.
 */
export function toolWindowCeiling(available?: number | undefined, scale = 1): number {
  if (available === undefined || !Number.isFinite(available)) return TOOL_MAX
  /*
   * `scale` is `--ui-scale`, and defaulting it to 1 is what keeps every existing caller and
   * every check that passes only a viewport correct — at the default chrome size the three
   * chrome rows are exactly [`CHROME_HEIGHT`].
   *
   * It has to be here rather than folded into the constants because the constants are design
   * numbers pinned against `tokens.css`, and because this is the half of the arithmetic that
   * is *wrong* when it disagrees with CSS. The header, tab strip and status bar are `calc(… *
   * var(--ui-scale))`; if this sum stayed flat, then at 17px the real chrome would be 22px
   * taller than the ceiling believes, and the panel would be allowed to squeeze the panes
   * below [`MIN_PANES`] by exactly that much. On tall windows only, which is where nobody
   * looks for it — the same failure the comment on [`CHROME_HEIGHT`] records for the case of
   * forgetting a row.
   *
   * The splitter does not scale: it is a drag handle, not a text box, and `--w-splitter` is a
   * flat 6px for the same reason the activity rail's width is flat.
   */
  const chrome = Number.isFinite(scale) && scale > 0 ? CHROME_HEIGHT * scale : CHROME_HEIGHT
  const room = available - chrome - SPLITTER_HEIGHT - MIN_PANES
  return Math.max(TOOL_MIN, Math.min(TOOL_MAX, room))
}

/**
 * A height brought inside the band, rounded to whole pixels.
 *
 * Rounded for `clampSidebarWidth`'s reason: the value goes straight into a custom property and
 * is read back by `getBoundingClientRect` on the next gesture, and sub-pixel drift over a dozen
 * drags is how a stored integer stops round-tripping.
 *
 * A non-finite input returns the floor rather than throwing or propagating `NaN`. `NaN` in a
 * custom property is not an error anywhere: the declaration is simply invalid, the panel falls
 * back to whatever the cascade had, and nothing says so.
 */
export function clampToolWindowHeight(
  px: number,
  available?: number | undefined,
  scale = 1,
): number {
  if (!Number.isFinite(px)) return TOOL_MIN
  // `scale` only ever reaches [`toolWindowCeiling`]; it is threaded rather than read here so
  // this module stays DOM-free and `check-toolwindow.mjs` can keep compiling it standalone.
  return Math.round(Math.max(TOOL_MIN, Math.min(px, toolWindowCeiling(available, scale))))
}

/**
 * What a drag has made the height, given where it started and how far the pointer moved.
 *
 * **`dy` is subtracted, and that inversion is the whole difference from the sidebar's version.**
 * The handle is on the tool window's *top* edge, so dragging **down** shrinks it and dragging up
 * grows it. Getting this backwards produces a splitter that works — it resizes, it clamps, it
 * commits — and moves the wrong way, which no type can catch. `check-toolwindow.mjs` asserts the
 * sign explicitly for that reason.
 */
export function heightFromDrag(
  originHeight: number,
  dy: number,
  available?: number | undefined,
  scale = 1,
): number {
  return clampToolWindowHeight(originHeight - dy, available, scale)
}

/** The custom property and its value, ready for `setProperty`. */
export function heightDeclaration(px: number): [property: string, value: string] {
  return [TOOL_TOKEN, `${px}px`]
}

/**
 * The shape of the persisted state this module reads.
 *
 * A structural restatement of `ToolWindowState`, not an import of it — see the module header.
 * Only the two fields that move a pane are named: the active tab and the open history list do
 * not change any geometry, so a frame without them costs nothing and this module has no opinion
 * about them.
 */
export interface StoredToolWindow {
  open: boolean
  height: number
}

/** What the panel is showing right now, and how tall. */
export interface ToolWindowGeometry {
  readonly open: boolean
  readonly height: number
}

/** The geometry a workspace snapshot carries, clamped. */
export function geometryFromState(
  state: StoredToolWindow | null | undefined,
): ToolWindowGeometry {
  if (!state) return { open: false, height: TOOL_DEFAULT }
  return {
    open: state.open === true,
    height:
      typeof state.height === 'number' ? clampToolWindowHeight(state.height) : TOOL_DEFAULT,
  }
}

/**
 * The boot cache's key.
 *
 * # Why a cache at all
 *
 * `open` and `height` have to be right on the tool window's **first painted frame**, or the pane
 * area changes size after the panes are already in it — and every `PaneSlot` resize runs
 * `fit()`, which reflows xterm and sends a SIGWINCH to a live `claude`. `App.tsx` fires
 * `app.restorePlan()` and `app.getBootstrap()` as two independent round trips in the same tick,
 * so the panes can exist before this state does. `sidebarWidth.ts` writes thirty lines about
 * this problem for a *width*; the tool window's version costs an agent's transcript a redraw.
 *
 * # Why the key is the window label
 *
 * A project id is not known at module-eval time — it arrives with the bootstrap, which is the
 * round trip being outrun. A window label *is*: `client.ts`'s `windowLabel()` reads it out of
 * `location.search`, and `cide-app`'s `restore_windows` recreates every window under its
 * *recorded* label, so the key is the one this window wrote last time.
 *
 * # Why the value is a map and not a bare object
 *
 * `localStorage` is per **origin**, so every window of this app shares this one key. Two windows
 * each writing a bare object would be precisely the cross-window bleed that keeping this state
 * per-project exists to prevent, moved out of Rust and into the cache. Hence a map, and hence
 * [`encodeToolWindow`] taking the live label set: labels are uuids, so a user opening and closing
 * projects in `PerProject` mode would otherwise grow this line for ever.
 *
 * A hint, never the truth. Every snapshot from Rust overwrites it, and anything unparseable, out
 * of band, or for an unknown label falls back to closed at the default height — which is also
 * what a first launch, a cleared profile and a webview with storage disabled get.
 */
export const TOOL_CACHE_KEY = 'cide.toolWindow'

/** This window's cached geometry, or the closed default. */
export function decodeToolWindow(
  raw: string | null | undefined,
  label: string,
): ToolWindowGeometry {
  const fallback: ToolWindowGeometry = { open: false, height: TOOL_DEFAULT }
  if (typeof raw !== 'string' || raw.length === 0) return fallback
  try {
    const parsed: unknown = JSON.parse(raw)
    if (typeof parsed !== 'object' || parsed === null) return fallback
    const entry = (parsed as Record<string, unknown>)[label]
    if (typeof entry !== 'object' || entry === null) return fallback
    const { open, height } = entry as Partial<StoredToolWindow>
    return {
      open: open === true,
      height: typeof height === 'number' ? clampToolWindowHeight(height) : TOOL_DEFAULT,
    }
  } catch {
    // A half-written or hand-edited line must not stop the app from booting.
    return fallback
  }
}

/**
 * This window's entry written into the cache line, every other live window's left byte-identical,
 * and every dead window's dropped.
 *
 * `liveLabels` comes from the workspace snapshot's `windows` map. An entry for a label that is no
 * longer there is not merely stale, it is unreachable — nothing will ever read it again — so
 * pruning here is what stops the line growing without bound.
 */
export function encodeToolWindow(
  raw: string | null | undefined,
  label: string,
  geometry: ToolWindowGeometry,
  liveLabels: readonly string[],
): string {
  const out: Record<string, StoredToolWindow> = {}
  if (typeof raw === 'string' && raw.length > 0) {
    try {
      const parsed: unknown = JSON.parse(raw)
      if (typeof parsed === 'object' && parsed !== null) {
        for (const [key, value] of Object.entries(parsed as Record<string, unknown>)) {
          if (key === label || !liveLabels.includes(key)) continue
          if (typeof value !== 'object' || value === null) continue
          const { open, height } = value as Partial<StoredToolWindow>
          if (typeof height !== 'number') continue
          out[key] = { open: open === true, height: clampToolWindowHeight(height) }
        }
      }
    } catch {
      // Unreadable: this window's entry is still worth writing, the rest is gone either way.
    }
  }
  out[label] = { open: geometry.open, height: clampToolWindowHeight(geometry.height) }
  return JSON.stringify(out)
}
