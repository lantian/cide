/**
 * Which panel the left sidebar is showing, whether it is showing one at all, and which one
 * comes back when it is asked to.
 *
 * Three gestures write this state and they mean three different things, which is the whole
 * reason it is a module rather than a `useState` with a ternary in it:
 *
 * ```
 * a rail button click   selectView(s, next)   the lit one toggles shut; any other switches
 * a sidebar.* command   showPanel(s, next)    always reveals — a command that named a panel
 *                                             and then closed it would be a command that
 *                                             sometimes did the opposite of its title
 * F4 / sidebar.toggle   toggleSidebar(s)      shut, or back to the last panel that was open
 * ```
 *
 * # Why `last` exists at all
 *
 * The hidden state has shipped since M3: `App.tsx` sets the view to `null` when you click the
 * lit rail button, `ActivityRail::active` is typed nullable, all four panel branches are
 * `view === '…' &&` guards and the splitter is withheld with them. It had no command, no
 * binding and no palette row — the mouse was the only way in *and* the only way back out. What
 * it also had was no memory: `setView(null)` threw the identity away, so a toggle could only
 * ever restore a hard-coded panel. `last` is the one genuinely new fact here.
 *
 * # `settings` is a view without a panel, and it counts as closed
 *
 * The rail's ⚙ opens a workspace *tab*; there is no settings panel, so with it lit the sidebar
 * is already showing nothing. F4 there therefore opens `last` rather than doing nothing at all,
 * which is the difference between a key that works and a key that looks broken exactly once per
 * session — right after somebody clicks the gear. For the same reason `last` is only ever
 * written by the two functions that show a real panel: restoring "the last thing that was open"
 * must never restore a state with nothing in it.
 *
 * # Not persisted, deliberately
 *
 * Nothing here reaches disk, and the two widths beside it (`chrome/sidebarWidth.ts`) still do.
 * That asymmetry is a decision rather than an omission. `Settings` is global and rides every
 * `cide://workspace-changed` to **every** window — which is exactly how a width drag in one
 * window reaches another — so in `PerProject` window mode a persisted "hidden" would mean F4 in
 * one window closing another window's sidebar. And a Rust-owned flag would arrive an IPC round
 * trip after first paint, so the panel would render and then be yanked away: the exact jump
 * `sidebarWidth.ts` writes thirty lines about avoiding, needing its own `localStorage` boot
 * cache to avoid it again. So a restart still opens on Files with the panel showing, bit for bit
 * as before, and the persistence question gets decided on its own merits rather than as a side
 * effect of a hotkey. `README.md` says so out loud.
 *
 * Import-free on purpose, exactly as `sidebarWidth.ts` is and for the same reason:
 * `ui/scripts/check-sidebar.mjs` compiles it standalone with the TypeScript already in
 * `node_modules` and executes it. A rule that lives in a React state updater is a rule no check
 * script can compile, and this project has paid for that five times.
 */

/**
 * Every button on the activity rail. `settings` is the one with no panel behind it.
 *
 * `agents` and `tasks` joined in M18 and cost this module exactly two union members; `extensions`
 * and M22's whole open-ended family of contributed panels cost one more and a template literal.
 * Nothing below them changed: `PanelView` is `Exclude<…, 'settings'>` and picked both up, and
 * `selectView`, `showPanel` and `toggleSidebar` are written as rules *over* the type rather
 * than as a branch per view, so neither of them mentions a view by name. That is the whole
 * payoff of this module existing, and it is worth stating so the next view added here is
 * added the same way — a function that grows a `if (next === 'tasks')` arm has stopped being
 * a rule and has become the ternary in `App.tsx` this file was extracted from.
 */
type ExtensionView = `ext:${string}`

export type ActivityView = BuiltinView | ExtensionView

/** The views cide ships. */
export type BuiltinView =
  | 'files'
  | 'git'
  | 'search'
  | 'problems'
  | 'agents'
  | 'tasks'
  | 'openspec'
  | 'extensions'
  | 'settings'

/**
 * A panel an extension contributes: `ext:<marketplace>.<extension>.<panel>`. (M22)
 *
 * A template-literal member rather than widening the whole union to `string`, which is what keeps
 * `'setings'` a type error while `'ext:m.sql.statements'` is not — and which is the whole reason
 * the union survived at all rather than becoming a bare `string`.
 *
 * `ext:` and not `ext.`: the command namespace an extension contributes to is dotted
 * (`ext.m.sql.showStatements`) and this one is not, so a string in a log or a bug report says
 * which of the two it is without any context.
 *
 * Every function below is written as a rule *over* the type rather than as a branch per view, so
 * none of them mentions this by name and none of them changed to accept it. That is the whole
 * payoff of this module existing, and M18 spent exactly two union members proving it; this spent
 * one and a half.

/** The views that actually draw a sidebar panel — everything a hidden sidebar can restore. */
export type PanelView = Exclude<ActivityView, 'settings'>

export interface SidebarState {
  /**
   * What the rail has lit, or `null` for nothing lit and the sidebar hidden.
   *
   * [`PanelView`] and not [`ActivityView`]: **⚙ is not a state this can be in.** It used to be,
   * and that was the bug —
   *
   * > *"when i click it - it stucks in active state like git panel, but it should - settings
   * > icon just opens settings tab, no need to stay in active state."*
   *
   * Settings is a workspace *tab*. Lighting a rail button for it claimed the sidebar was showing
   * something it has no panel for, and the button then stayed lit until another one was pressed,
   * because nothing else could clear a view that draws nothing. Narrowing the type is what makes
   * that unreachable rather than merely fixed: `isPanelOpen` no longer needs to special-case it
   * and no future updater can put it back.
   */
  readonly view: PanelView | null
  /**
   * The last panel that was actually showing. Never `settings`, never `null`.
   *
   * What [`toggleSidebar`] restores. It survives a hide and it survives a trip through the
   * settings tab, which is the whole of what makes F4 a toggle rather than a "show Files".
   */
  readonly last: PanelView
}

/** Files, open — what every shell window has started with since M3. */
export const SIDEBAR_INITIAL: SidebarState = { view: 'files', last: 'files' }

/**
 * Is a panel on screen?
 *
 * One comparison, because `view` is a [`PanelView`] — every value it can hold draws a panel. It
 * carried a second test against `'settings'` until that stopped being representable.
 */
export function isPanelOpen(state: SidebarState): boolean {
  return state.view !== null
}

/**
 * A click on a rail button.
 *
 * The lit one toggles its panel shut — the gesture that has existed since M3, kept exactly — and
 * any other switches to it, whether or not the sidebar is currently hidden.
 *
 * **⚙ changes nothing here.** It opens the Settings tab, which the call site does, and the
 * sidebar is left exactly as it was: whatever panel was showing keeps showing, and no rail
 * button lights up for it.
 *
 * It used to return `{ view: 'settings' }`, on the reasoning that a rail click should always
 * move the rail. That was wrong twice over — the gear stayed lit with no panel under it until
 * some other button was pressed, and clicking it hid whatever panel the user had open, which is
 * a side effect nobody asked a tab-opening button for. Returning the state untouched is also why
 * this is still a function rather than an early return at the call site: the *rule* is that ⚙ is
 * orthogonal to the sidebar, and a rule is worth a line here where a check can run it.
 */
export function selectView(state: SidebarState, next: ActivityView): SidebarState {
  if (next === 'settings') return state
  if (state.view === next) return { view: null, last: next }
  return { view: next, last: next }
}

/**
 * A `sidebar.files` / `.git` / `.search` / `.problems` command, or any code path that has to
 * reveal a panel before acting on it (`file.reveal`, `git.commit`, the scratch flow).
 *
 * Always reveals, never toggles. `keys/dispatch.ts` argues it under `sidebar.search`: a panel is
 * not an overlay, the rail button is right there, and a second Ctrl+Shift+F that closed the
 * panel would take away the one thing the binding is for.
 */
export function showPanel(state: SidebarState, next: PanelView): SidebarState {
  // Spread rather than built fresh, and the two are identical *today*: both fields are being
  // written. It is written this way so that a third field added to `SidebarState` later is
  // carried by default instead of being silently reset by the one updater that forgot it.
  return { ...state, view: next, last: next }
}

/**
 * F4.
 *
 * Written as a plain `(prev) => next` so `App.tsx` can hand the function itself to
 * `setSidebar` — there is nothing to close over, which is what keeps the rule out of the
 * component.
 */
export function toggleSidebar(state: SidebarState): SidebarState {
  return isPanelOpen(state)
    ? { view: null, last: state.last }
    : { view: state.last, last: state.last }
}
