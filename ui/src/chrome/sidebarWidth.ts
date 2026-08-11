/**
 * The parts of "resize the left panel and remember it" that are decisions rather than side
 * effects: the clamp, the two tokens, and the boot cache's encoding.
 *
 * Split out of `SidebarSplitter.tsx` for the same reason `settings/theme.ts` is split out of
 * `useSettings.ts` — there is no JS test runner in this project and the app must never be
 * launched to look at a layout, so everything that can be wrong on its own is pulled into a
 * module `scripts/check-sidebar.mjs` can compile with `tsc` and assert against. The rule for
 * what belongs here: values in, values out. Reading `window.innerWidth`, writing a custom
 * property and touching `localStorage` stay with their caller.
 *
 * Deliberately import-free, including of `@/ipc/client`: the check compiles this one file on
 * its own, and a single `import type` of the generated DTOs would drag the path alias and the
 * whole IPC surface in behind it. [`StoredSidebar`] is therefore a structural restatement of
 * `SidebarSettings` from `generated.ts` rather than that type — `widthsFromSettings` is
 * called with the real one, so a rename on the Rust side still fails the build.
 */

/** The two panels that own a width. See [`SIDEBAR_TOKEN`] for why there are only two. */
export type SidebarPanel = 'files' | 'git'

/**
 * Which custom property each panel's width is stored in.
 *
 * These are the tokens `tokens.css` already had, and driving the resize through them is what
 * makes this feature cost no changes in `FileTree.module.css`, `GitPanel.module.css`,
 * `SearchPanel.module.css` or `ProblemsPanel.module.css` — each one already reads
 * `width: var(--w-sidebar-…)`. The alternative was an inline `style={{ width }}` on each
 * panel, which would have meant editing four stylesheets this change does not own and giving
 * the width two sources that could disagree.
 *
 * `--w-sidebar-files` sizes search and problems too. That is `tokens.css`'s decision, made
 * before this change and followed rather than reopened: those panels are the explorer's
 * column with different rows in it, and a user who widens the tree to read long paths means
 * the same thing when they widen the search results.
 */
export const SIDEBAR_TOKEN: Readonly<Record<SidebarPanel, string>> = {
  files: '--w-sidebar-files',
  git: '--w-sidebar-git',
}

/**
 * The mock's widths, which are also the literals `tokens.css` ships and `SidebarSettings`'
 * `Default` impl in `crates/cide-ipc/src/settings.rs`. Three copies of two numbers, pinned
 * against each other by `check-sidebar.mjs`, because the failure of letting them drift is
 * that the panel moves on first launch and then never again.
 */
export const SIDEBAR_DEFAULT: Readonly<Record<SidebarPanel, number>> = {
  files: 252,
  git: 420,
}

/**
 * The floor, in CSS pixels. Mirrors `SIDEBAR_MIN_WIDTH` in `cide-ipc`.
 *
 * 180 because below it both panels stop being usable rather than merely getting tight: the
 * explorer indents 12px per depth on 21px mono rows, so a file three levels down has about a
 * dozen characters left at 180 and none at 140, and the git panel's 30px header carries a
 * segmented Commit/Shelf control and its counts on one line. There is no "reset width"
 * gesture in the chrome, so a panel that can be dragged to a sliver is one a user can lose.
 */
export const SIDEBAR_MIN = 180

/**
 * The stored ceiling, in CSS pixels. Mirrors `SIDEBAR_MAX_WIDTH` in `cide-ipc`.
 *
 * Roughly 1.5x the mock's 420px git panel. It is not the number that decides what fits —
 * [`sidebarCeiling`] is, because what fits depends on the window — this is the bound that
 * keeps a stored value sane on a 4K monitor whose window is later opened on a laptop.
 */
export const SIDEBAR_MAX = 640

/** The activity rail's width: `--h-rail`, used as a width because the rail is square. */
export const RAIL_WIDTH = 42

/**
 * What the workspace keeps for itself, whatever the sidebar asks for.
 *
 * 360px is about 45 columns of the 12px mono terminal — enough for a Claude session to be
 * read rather than merely present. It is the number that makes the ceiling dynamic: with
 * `MIN_WIDTH = 720` in `crates/cide-app/src/windows.rs`, the narrowest legal window leaves
 * `720 - 42 - 360 = 318px` for the sidebar, so the drag stops there rather than at 640 and
 * the workspace cannot be squeezed to nothing on any window the app allows.
 */
export const MIN_WORKSPACE = 360

/** Both widths, in CSS pixels. */
export interface SidebarWidths {
  files: number
  git: number
}

/**
 * The shape `SidebarSettings` has on the wire. Structural on purpose — see the module note.
 */
export interface StoredSidebar {
  filesWidth: number
  gitWidth: number
}

/**
 * The widest this panel may be drawn given a viewport of `available` CSS pixels.
 *
 * `undefined` means "no viewport to ask" — the boot path, before there is a window to
 * measure — and falls back to the static ceiling. Never returns less than [`SIDEBAR_MIN`]:
 * on a window too narrow to honour both bounds the floor wins, because a panel clamped to
 * 90px is not a compromise, it is a broken panel, and the user can still make the window
 * bigger.
 */
export function sidebarCeiling(available?: number | undefined): number {
  if (available === undefined || !Number.isFinite(available)) return SIDEBAR_MAX
  return Math.max(SIDEBAR_MIN, Math.min(SIDEBAR_MAX, available - RAIL_WIDTH - MIN_WORKSPACE))
}

/**
 * A width brought inside the band, rounded to whole pixels.
 *
 * Rounded because the value is written straight into a custom property and read back by
 * `getBoundingClientRect` on the next gesture; letting sub-pixel drift accumulate over a
 * dozen drags is how a panel ends up at 251.9997px and a stored integer stops round-tripping.
 *
 * A non-finite input returns the floor rather than throwing or propagating `NaN`. `NaN` in a
 * custom property is not an error anywhere: the declaration is simply invalid, the panel
 * falls back to whatever the cascade had, and nothing says so.
 */
export function clampSidebarWidth(px: number, available?: number | undefined): number {
  if (!Number.isFinite(px)) return SIDEBAR_MIN
  return Math.round(Math.max(SIDEBAR_MIN, Math.min(px, sidebarCeiling(available))))
}

/** What a drag has made the width, given where it started and how far the pointer moved. */
export function widthFromDrag(
  originWidth: number,
  dx: number,
  available?: number | undefined,
): number {
  return clampSidebarWidth(originWidth + dx, available)
}

/** One panel replaced, the other copied. */
export function withPanel(
  widths: SidebarWidths,
  panel: SidebarPanel,
  px: number,
  available?: number | undefined,
): SidebarWidths {
  return { ...widths, [panel]: clampSidebarWidth(px, available) }
}

/**
 * The widths a settings snapshot carries, clamped.
 *
 * `null`/`undefined` is the pre-bootstrap window, which has no opinion yet and gets the
 * defaults. Clamped on the way in as well as on the way out: `workspace.json` is a file a
 * user can edit, and Rust clamps a *patch* rather than re-clamping every load.
 *
 * Deliberately not clamped against the viewport here — that is [`applyWidths`]' caller's
 * job, and doing it here would quietly narrow the value that then gets written back.
 */
export function widthsFromSettings(sidebar: StoredSidebar | null | undefined): SidebarWidths {
  if (!sidebar) return { ...SIDEBAR_DEFAULT }
  return {
    files: clampSidebarWidth(sidebar.filesWidth),
    git: clampSidebarWidth(sidebar.gitWidth),
  }
}

/** The patch shape, for `settings.set({ sidebar })`. Both widths, per the patch-per-field rule. */
export function toStored(widths: SidebarWidths): StoredSidebar {
  return { filesWidth: widths.files, gitWidth: widths.git }
}

/**
 * The declarations to write on `<html>` for these widths.
 *
 * Returned as pairs rather than applied, so the decision is testable without a DOM. The
 * caller is three lines of `setProperty` in `SidebarSplitter.tsx`.
 */
export function widthDeclarations(widths: SidebarWidths): [property: string, value: string][] {
  return [
    [SIDEBAR_TOKEN.files, `${widths.files}px`],
    [SIDEBAR_TOKEN.git, `${widths.git}px`],
  ]
}

// --- the boot cache ----------------------------------------------------------------------

/**
 * Where the first-paint copy of the widths lives.
 *
 * See [`decodeWidths`] for why there is a second copy of a setting `workspace.json` owns.
 */
export const SIDEBAR_CACHE_KEY = 'cide.sidebarWidths'

/** The cache line. Compact and versionless: it is a hint, and a stale one is discarded. */
export function encodeWidths(widths: SidebarWidths): string {
  return JSON.stringify(toStored(widths))
}

/**
 * The widths a cache line holds, or the defaults if it holds anything else.
 *
 * **Why a cache at all.** The stored width has to be on the panel's *first* painted frame or
 * the panel visibly jumps on every launch, and `app.get_bootstrap` — where settings actually
 * come from — is an IPC round trip that resolves several frames after React first paints.
 * That is the same problem `theme-boot.js` solves, and its mechanism was read first: Rust
 * bakes `?theme=` onto the webview URL in `windows.rs`, and a classic script in `<head>`
 * writes the attribute before the parser reaches `<body>`.
 *
 * That mechanism is the better one and it is not available to this change: `windows.rs`,
 * `index.html` and `public/theme-boot.js` are all outside what this task owns, and inventing
 * a *third* boot channel was the thing to avoid. So this takes the option `theme-boot.js`
 * explicitly rejected — `localStorage` — under the two conditions that made it wrong there
 * and make it acceptable here.
 *
 * First, the theme is written by Rust as well as by the UI (a `workspace.json` restored from
 * a backup, a default that moved), so a UI-owned copy could be authoritative and wrong. A
 * sidebar width has exactly one writer, the gesture in `SidebarSplitter.tsx`, and it updates
 * this cache in the same breath as it sends the patch.
 *
 * Second, and the real difference: the theme's first frame is painted by the *browser*, from
 * `:root`, before any script runs — so only a `<head>` script can be early enough. The
 * sidebar's first frame is painted by React, and this module is imported by
 * `SidebarSplitter.tsx`, which is imported by `App.tsx`, which is imported by `main.tsx`.
 * Module evaluation finishes before `createRoot().render()`, so a value read here is already
 * on `<html>` when the panel is laid out for the first time. There is no window in which the
 * default is visible.
 *
 * The cache is a hint and never the truth. Every snapshot from Rust overwrites it, and
 * anything unparseable, out of band or not a number falls back to the mock's widths — which
 * is also what a first launch, a cleared profile and a webview with storage disabled get.
 */
export function decodeWidths(raw: string | null | undefined): SidebarWidths {
  if (typeof raw !== 'string' || raw.length === 0) return { ...SIDEBAR_DEFAULT }
  try {
    const parsed: unknown = JSON.parse(raw)
    if (typeof parsed !== 'object' || parsed === null) return { ...SIDEBAR_DEFAULT }
    const { filesWidth, gitWidth } = parsed as Partial<StoredSidebar>
    return {
      files: typeof filesWidth === 'number' ? clampSidebarWidth(filesWidth) : SIDEBAR_DEFAULT.files,
      git: typeof gitWidth === 'number' ? clampSidebarWidth(gitWidth) : SIDEBAR_DEFAULT.git,
    }
  } catch {
    // A half-written or hand-edited line must not stop the app from booting.
    return { ...SIDEBAR_DEFAULT }
  }
}
