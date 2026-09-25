# The UI kit

How every control in cide should look, as live components and as rules. The kit is the
reference for the planned redesign and for **every new piece of UI from now on**.

- **The page:** `ui/kit.html` → `ui/src/kit/Kit.tsx`. With the dev server up (`./run.sh`
  starts it, or `pnpm --dir ui dev` on its own), open **<http://localhost:1420/kit.html>** in any
  browser. `?theme=dark` and `?ui=15` mean what they mean in a cide window (`public/theme-boot.js`
  reads them), and the page's own theme and size switchers write them back into the URL.
- **The components:** `ui/src/kit/components/*.tsx`, one `*.module.css` beside each. They are
  real, typed, accessible React components, built only on `styles/tokens.css`, the shared
  `styles/buttons.module.css` and the `Icon` component.
- **The palette:** a bright red accent in place of the old orange-brown, in `styles/tokens.css`
  since the redesign started (2026-09-24); the kit page and the app paint with the same values.
  The kit's other tokens (`--grad-green`, `--grad-yellow` with
  `--on-yellow`, `--grad-red`, `--grad-purple`, `--grad-blue`, `--grad-cyan`, `--grad-neutral`,
  `--on-fill`, `--fill-sheen`, `--grad-danger`, `--on-danger`, `--danger-edge`)
  live there too.
- **The gate:** `pnpm --dir ui run check:kit`. See [Keeping it true](#keeping-it-true).

## Where the look comes from

The user picked two surfaces as the reference: the **New Project wizard**
(`chrome/newProject/`) and the **GitLab panel**. For the GitLab panel that means the inbox and the
review sidebar (`gitlab/ReviewChrome.module.css`), not the older `GitLab.module.css`. The kit takes
their look and rebuilds it on tokens throughout. Where the two disagreed, or where one bypassed a
token, the kit settled it; those decisions are listed under
[Decisions the kit made](#decisions-the-kit-made).

**Status (2026-09-24): wired in.** The redesign moved the whole app onto the kit, in phases the
user reviewed: the palette, every dialog and picker, settings, the sidebar panels, the panes and
windows, and the app chrome. Where a surface could not render the kit component itself it
composes the kit's classes (rule 3 says when). What is left of the app's own drawing, and why, is
the page's last chapter, *What the app has today*.

## Rules for new UI

These are for anyone adding UI, Claude included.

1. **Look here before drawing anything.** Find the element in the chapter index below, read its
   entry, and use the kit component: `import { Button } from '@/kit/components/Button'`. Don't
   write a new `.primary`, `.action`, `.row` or `.notice` class in a feature's module; that is how
   the app ended up with about 45 secondary buttons.
2. **Something missing gets added to the kit first, then used.** Adding it to the kit means:
   - the component, in `ui/src/kit/components/`;
   - a specimen, in the chapter it belongs to under `ui/src/kit/page/chapters/`;
   - an entry in this file.

   `check:kit` fails when any of the three is missing. Before inventing, check whether an
   existing component with one more prop covers it.
3. **Moving an existing surface onto the kit** means rendering the kit component. Two
   exceptions, both written down where they happen:
   - an element whose markup a check pins line by line (the Agents settings form) keeps its
     element and takes the kit's drawing with `composes: button sm secondary from
     '../kit/components/Button.module.css'` (likewise `Status.module.css`'s `badge soft`);
   - a field that carries a state the kit field does not draw (a struck-through refused CLI
     argument, a column-sized cell in a grid) stays a bare `<input>` drawn with the kit field's
     numbers: `--bg`, one control height, the accent border and 3px halo on focus.
4. **Changing a kit component changes every user of it.** Update its specimen and its entry here
   in the same change, and look at the page in both themes and at UI size 11 and 17.
5. **Tokens only.** Every colour, size, radius, gap and duration is a `var(--…)` from
   `tokens.css`. A value that isn't a token is added as a token first. Sizes around text are
   `calc(Npx * var(--ui-scale))`, literal first (`check:ui-scale`). Font sizes are `--fs-ui-*`
   rungs only.
6. **The kit stays standalone.** Nothing under `ui/src/kit/` may import from `ipc/`, `store/`, a
   panel or a pane. It imports only React (and `react-dom` for portals), `@/icons/Icon`,
   `@/icons/iconPaths`, `styles/`, and other kit files. The page runs in a plain browser tab with no Tauri behind it, and the same
   rule keeps a component usable from any surface. `check:kit` enforces this.
7. **App chrome is on the kit too** (the user's call, 2026-09-24, reversing the first draft of
   this rule). The rail, the editor tab strip, the status bar, a pane's title bar and the
   window's own buttons are drawn by `Chrome.tsx` / `Chrome.module.css`. The app's chrome
   components (`chrome/TabStrip.tsx`, `chrome/ActivityRail.tsx`, `chrome/StatusBar.tsx`,
   `layout/PaneTitleBar.tsx`, `chrome/AppHeader.tsx`) keep their markup — drag, overflow,
   detach and the layout audit (`chrome/layoutAudit.ts`) read it — and compose those classes.
   Still out: what those surfaces *render* — diff, merge and commit-graph drawing, the terminal
   and the editor surface — and the splitters' drag geometry; they use the tokens only.

## Foundations

### Colour: one meaning per token

| token | use |
| --- | --- |
| `--bg` | the window; the inside of a text field |
| `--chrome` | header, tab strip, dialogs, pickers |
| `--panel` | sidebars, cards, menus |
| `--panel-2` | a recessed strip inside a card; row hover |
| `--chrome-hi` | pressed controls, tracks, counters |
| `--sel` | the selected row, the focused menu item |
| `--border` / `--border-soft` | edges of boxes / dividers between rows inside one |
| `--text-hi` / `--text` / `--dim` / `--faint` | titles and typed values / body / secondary / hints, placeholders, disabled |
| `--accent` | links, focus, the current thing. **Sparingly**: it marks the current thing and the one primary act. Proposed: `#dc1f2b` light (4.91:1 on white), `#ff5a5f` dark |
| `--grad-accent-ink` | anything carrying text on the accent: primary button, current step dot, switch when on, accent counter |
| `--grad-accent` | decorative only. White on its orange end is 2.4:1, so never put text on it |
| `--grad-accent-h` / `--grad-accent-soft` | horizontal bars (progress, tab underline) / art grounds (wizard rail, empty-state mark) |
| `--green` `--yellow` `--red` `--purple` `--blue` | status: done / waiting on someone / failed / merged / informational. Used as text, an icon or a wash, never as a big fill |
| `--grad-danger` + `--on-danger` + `--danger-edge` | the danger button and a danger menu item under the pointer: a black fill, white label; the edge is a light border only the dark theme needs |
| `--grad-<tone>` + `--on-fill` + `--fill-sheen` | small filled marks (badges, counters, dots, note marks, progress): the tone's gradient under a white label, with a 1px lit top edge like the primary button. White is ≥ 4.5:1 on both stops of every one but amber, which takes `--on-yellow`, in both themes |

Surfaces step down one token at a time: `--bg` → `--panel` → `--panel-2` → `--chrome-hi`. A card
on a panel is told apart from it by its border, not by a second fill.

### Type, space, radius, elevation, motion

- **Faces:**
  - Inter for the interface.
  - JetBrains Mono for anything a user might paste into a terminal: paths, branches, ids,
    commands, shortcuts.
- **Sizes and weights:**
  - `--fs-ui-20` is for page titles only.
  - `-16` is dialog and step titles; `-14` is card titles.
  - `-13` is body text and list titles; `-12` is labels, buttons, rows and menus.
  - `-11` is hints, captions, badges, meta lines and small buttons.
  - Weight is 400, 500 or 600. A primary or danger button's label, a filled badge, a counter
    and a title are 600; every other button label is 400.
  - **A selected state is shown by ground, colour or an underline, not by going bold.** A
    chosen segment or the current tab is 500, never 600.
  - **Small coloured text on a light ground is at most 500.** At 11–12px a red or tinted label
    at 600 smears into colour fringes (the user's note on the segmented control). White text
    on a fill is the exception and stays 600.
- **Uppercase:** only the section caption (11px, 600, 0.05em tracking, `--faint`). It always
  means "a group starts here".
- **Spacing:**
  - The scale is 2 · 4 · 6 · 8 · 12 · 16 · 24 (`--sp-1`…`--sp-7`).
  - Use 12 inside cards and rows, 16 around sections, and 24 between groups.
- **Radius**, which grows with the object:
  - `--r-1` (4): tag, key, inline code, icon button
  - `--r-2` (6): buttons, fields, notes
  - `--r-3` (8): cards, menus, summaries
  - `--r-4` (12): dialogs, pickers
  - `--r-full`: badges, dots, the switch, avatars
- **Elevation:**
  - `--shadow-1`: a resting primary button, a raised segment
  - `--shadow-2`: a hover lift, menus, tooltips
  - `--shadow-3`: dialogs, pickers, toasts
- **Motion:**
  - `--dur-1` (90ms) for hover and press, `--dur-2` (140ms) for something arriving, both on
    `--ease-out`.
  - Only paint properties are animated (`check:motion`).
  - A looping animation sets `animation-play-state: var(--motion-loop)` so reduced motion pauses
    it.
- **Icons:** `<Icon name size>` only, never a Unicode glyph (`check:ui-icons`). Sizes are 12,
  14 (rows and small buttons), 16 (toolbars and md buttons) and 20 (empty states). A missing
  mark is vendored with `scripts/vendor-ui-icons.mjs`.

### State, the same way everywhere

| state | how it is drawn |
| --- | --- |
| hover (row, icon button, quiet button) | `--panel-2` wash |
| selected (row, picker row, menu item) | `--sel` |
| open / current (the MR you're viewing, the current task) | 2px `--accent` bar on the left + `color-mix(--accent 5%, --panel)` |
| on (segmented) | raised onto `--panel`, 1px `--border`, `--shadow-1`, label `--text-hi` 500 |
| on (icon toggle) | `--accent` on an accent 12% wash |
| focus (controls) | `--focus-ring`, 2px offset |
| focus (fields) | `--accent` border + `0 0 0 3px` accent at 20% |
| focus (rows) | `--focus-inset` |
| disabled | primary and danger buttons fade to 0.45; everything else turns `--faint` |
| invalid | red border **and** the reason in words underneath |

## Components

Chapters are anchors on the page: `kit.html#<id>`.

| chapter | id |
| --- | --- |
| Colour · Type · Space, radius, elevation · Icons | `colour` · `type` · `space` · `icons` |
| Buttons | `buttons` |
| Fields | `fields` |
| Choices | `choices` |
| Status marks | `status` |
| Feedback | `feedback` |
| Panels, cards, lists | `structure` |
| Dialogs, menus, pickers | `overlays` |
| App chrome | `chrome` |
| Patterns | `patterns` |
| What the app has today | `inventory` |

### Buttons: `Button.tsx`

- **`Button`**, in two sizes: `md` is 30px (dialogs and forms), `sm` is 24px (panel rows and
  toolbars). The variants:
  - `primary`: the one act the surface exists for, at most one per footer or row. It composes
    `styles/buttons.module.css`.
  - `secondary`: every other act beside it, such as Back, Cancel or Browse.
  - `quiet`: a borderless act inside dense content, such as "Load more" or a row's Retry.
  - `danger`: the confirming half of something destructive. **Filled black** (`--grad-danger`,
    white label), drawn like the primary in ink, so destroying never shares the primary's red.
  - `link`: an act inside a sentence.

  `busy` swaps the icon for a spinner and disables the button. `trailingIcon="chevron-down"`
  marks a button that opens a menu.
- **`IconButton`**: square, the same two heights (24 and 30), no border at rest. `label` is
  required, because it is both the name and the tooltip. `pressed` makes it a toggle. A panel
  header has at most three before the rest go into a `…` menu.

### Fields: `Field.tsx`

- **`Field`** wraps every control. It wires the label, the `optional` marker, the hint and the
  error to the control by id.
- **`TextInput`**: `--bg` box, same heights as buttons, `mono` for anything pasteable, an optional
  leading `icon` and a `trailing` slot. **`SearchField`** is a `TextInput` with the search mark; the browser's own clear button is
  hidden, so the kit's is the only one.
- **`Select`** (`Select.tsx`) is the kit's own dropdown, not the OS popup: a field-shaped
  trigger and a menu-shaped list with a check on the chosen option, optional `icon` and
  `detail` per option, `placeholder`, `invalid` and `disabled`. It follows the WAI-ARIA
  select-only combobox pattern: focus stays on the trigger, and the keys are arrows, Home/End,
  Enter/Space, Escape, Tab and letter type-ahead. The list is portalled to `<body>`, so a
  dialog's `overflow` can't clip it, and it opens upward when there is no room below.
  `onChange` receives the value, not an event.
- **`Textarea`** follows the same focus rule as the other fields.
- **`FormRow`**: a settings row, with label and hint on the left and the control on the right.
  The text keeps a floor of about 260px; a control too wide to leave it that (a four-way
  `Segmented`) wraps onto its own line under the text rather than squeezing the text column.

### Choices: `Choice.tsx`

- **`Checkbox`**: an independent yes/no applied on submit. A native input with `accent-color`,
  and it supports `indeterminate`.
- **`RadioGroup`**: one of 2–5 options that each need a sentence. `inline` lays short options
  in a row and keeps the legend for screen readers only, for a choice whose consequences the
  text around it explains (a reset's Soft / Mixed / Hard).
- **`Switch`**: a yes/no that takes effect at once. It is the primary gradient when on.
- **`Segmented`**: one of 2–4 short views or scopes of the same thing, drawn like the GitLab
  inbox's scope tabs. `block` stretches it across the panel, and the arrow keys move between
  segments. For switching to different content, use `Tabs` instead.
  Its segments share the width and clip their labels, so it never pushes past a narrow panel.
- **`ChoiceCards`**: one of 2–4 options that each need a picture or a paragraph (the wizard's
  project type).
- **`ToggleCard`**: a checkbox that needs a sentence of why.

### Status marks: `Status.tsx`

Tones are meanings: `blue` in progress or informational, `green` done, `yellow` (an amber fill
with dark ink) waiting on someone, `red` failed, `purple` merged, `cyan` a kind that is not a
state (suggestion, docs), `neutral` no state worth a colour. **`accent` is never a status**: it
is the brand red, and "running" drawn in it read as failed. It marks only "yours" or "asking for
you" (an awaiting-input counter, an unsaved dot).

- **`Badge`**: a state someone else named, filled with the tone's gradient. Use `dot` for a state
  (pipeline, job) and `squared` for a kind (MR state, severity). `soft` draws the quiet form (the
  hue on a 12% wash) for a badge repeated down a long list.
- **`Tag`**: a label the user attached. `onRemove` adds its ✕.
- **`Counter`**: filled like a badge, tabular figures, and 99+ above 99. Neutral by default; it
  is accent only when it asks for attention.
- **`Dot`**: a wordless state, filled with the tone's gradient. It always has a `label`, which
  becomes its tooltip. `pulse` adds a halo one gap away for a live state.
- **`Kbd`**: every shown shortcut.
- **`Avatar`** / **`Person`**: an initial tinted by the person's role colour.
- **`Code`**: a branch, path or sha inside a sentence.

### Feedback: `Feedback.tsx`

- **`Note`**: inline, about the thing next to it. The tones are `info` (blue), `ok`, `warn` and
  `bad`.
  The tone shows as a 3px gradient bar down the left, a round gradient mark holding the icon, a
  hairline border and a wash that fades left to right. The words stay `--text`, so every tone
  reads the same. It takes at most one action at the right edge (`action`). A note with more than
  one thing to do, or one in a sidebar-wide column, puts a row of `sm` buttons under the text
  instead (`actions`); the MR panel's agent-review note is the first user. Never both.
- **`Banner`**: flush across the top of a panel or pane, about the whole of it.
- **`Toast`**: something that happened elsewhere. A failure toast stays until dismissed.
- **`EmptyState`**: a mark, a claim, a sentence and one button, centred both ways.
- **`Spinner`** when the fraction is unknown, **`Progress`** when it is known, and
  **`Skeleton`** lines while a first page loads.
- **`Checklist`**: steps that go waiting → running → done or failed. A failed step says why
  under itself.

### Panels, cards, lists: `Surface.tsx`

- **`PanelHeader`**: tops every sidebar panel, at `--h-panelheader`.
- **`Section`**: the uppercase caption over a group.
- **`Heading`**: a title plus one lead sentence.
- **`Tabs`**: switch what is shown, with a gradient underline on the current tab and arrow-key
  movement. A tab's label reserves its bold width, so choosing a tab never shifts its
  neighbours (`Segmented` does the same).
- **`Toolbar`**, with **`ToolbarSeparator`** and **`ToolbarSpacer`**. `label` names what the
  tools act on for a screen reader. It wraps to a second line rather than clipping in a narrow panel.
- **`List`** + **`ListItem`**: the rich row from the GitLab inbox (top facts, a title clamped to
  two lines, meta, a foot of signals). `current` draws the accent bar.
- **`List`** + **`Row`**: the compact row, 26px, one line. Pass `depth` and `expanded` to make it
  a tree row, which indents 12px per level.
- **`Card`**: a box. A toned card is a state; a `draft` (dashed) card is proposed but not yet
  real.
- **`Summary`**: key → value with zebra rows.
- **`Table`**: numbers are right-aligned with tabular figures.
- **`Breadcrumbs`**: mono path segments; the last one is the current place.
- **`Disclosure`**: a native `<details>` with the kit's chevron.
- **`CodeBlock`**: output a user may copy. It is selectable and scrolls sideways rather than
  wrapping.
- **`PathList`** + **`PathRow`** (+ **`PathGroup`** for a caption between groups): what an act
  is about to touch, one named row each — a confirm's files, a close's unsaved tabs, a pull's
  commits. It sits flush in a `Dialog` body. Never a count; the name stays whole and the place
  beside it ellipsises first. The `mark` carries the tone, the words stay neutral. Capped at
  260px and scrolling unless `max={false}`. `trailing` holds a row's own answers (`sm` buttons) or its
  state in words (a conflict's "resolved").

### Dialogs, menus, pickers: `Overlay.tsx`

- **`Dialog`**: a title and lead, a body, and a footer with an optional note on the left and the
  buttons on the right, **primary last**. When a dialog confirms something destructive, the safe
  button is the primary and holds the focus. **`Scrim`** is the dimmed layer under it.
  - `width`: `narrow` 420 (a one-question confirm), default 520, `picker` 620, `wide` 880.
  - `titleAside` sits right of the title: which question this is ("2 of 7"), a badge.
  - `head` is more of the head under the lead, for a control the body is *about* (a mode
    picker); `flush` runs the body edge to edge for a list; no `actions` means no footer.
  - `below` is a strip pinned under the scrolling body: provenance that is the same for every
    page of it (who ran a log line, on what model).
  - It never grows past the window: the body scrolls, head and footer stay.
  - Extra attributes (`data-audit`, `onKeyDown`) land on the dialog. In the app it is mounted
    inside `Modal` (`overlays/ModalShell.tsx`), which portals the `Scrim` to `<body>`.
- **`Wizard`** with **`Stepper`**: the New Project frame. A rail holds the brand, the steps and a
  note; the footer keeps one height on every step.
- **`Picker`**: the command-palette frame. The matched characters are drawn in the accent, and a
  footer of key hints sits below.
  - It comes in parts, because the app's lists are virtualised and its field draws its own
    caret: **`PickerFrame`** (the box; `narrow` is 340 for a one-field popup),
    **`PickerInput`** (the 44px row: `lead` mark, the caller's field, a `trailing` counter),
    **`PickerList`** (the scroll container a virtualiser measures), **`PickerRow`**
    (`selected`, `disabled`, `virtual` when a virtualiser places it), **`PickerMatch`** (the
    matched characters), **`PickerStatus`** (a line in the list's place: "No matches"),
    **`PickerFoot`** and **`PickerHint`** (`⏎ open`). `Picker` is them assembled for a short
    static list.
  - In the app, `overlays/ModalShell.tsx` builds every picker from these inside `Modal`.
- **`Menu`**: icons are optional but aligned, shortcuts sit on the right in mono, the destructive
  item comes last, bold, with the black danger fill under the pointer, and a group caption appears once there are more than seven items.
- **`Tooltip`**: inverted, short, and with the shortcut if there is one. The app has none today
  and uses the native `title` about 200 times.

These draw the surface only. Positioning, anchoring and Escape are the caller's job, as with
`overlays/OverlayCard.tsx` today.

### App chrome: `Chrome.tsx`

- **`Rail`** + **`RailButton`**: the activity rail. 32px buttons in the 46px gutter; the current
  panel is the accent on a 12% wash; a count is a `Counter` (neutral, `accent` when it asks for
  you, `red` for failures).
- **`ChromeTabs`** + **`ChromeTab`**: the editor-style tab strip at `--h-tabstrip`. The current
  tab is the panel ground with the accent underline and `--text-hi` (not bolder: the strip is
  measured by the layout audit and must not reflow); an unsaved tab's close wears the
  accent dot until pointed at.
- **`StatusBar`** + **`StatusItem`**: the bottom bar at `--h-status`, mono at 12 (readouts a user
  compares and pastes), clipping rather than wrapping. A status hue goes on the
  icon only; an item that acts is a button with the kit hover.
- **`PaneBar`**: a pane's title bar, lit when the pane has focus, tools at the right.
- **`ChromeButton`** and **`WindowControls`**: the 22px buttons of a pane bar or a self-drawn
  window frame. On is the accent on a 12% wash; the window's close is red under the pointer only.

## Decisions the kit made

These are the places where the two reference surfaces disagreed with each other or with the
tokens:

- **One control height per size.** Buttons and fields are both 30 (`md`) or 24 (`sm`). The
  wizard had a 32px path field beside a 30px button.
- **`--grad-accent-ink` for every accent fill that carries text.** The wizard's Continue,
  step dot and brand mark used `--grad-accent`, which is decorative only.
- **Radius tokens instead of literals.** The GitLab panel used literal 4, 5 and 6px radii and
  `#8884` border fallbacks.
- **The switch takes its knob colour from `--on-accent`, not `--text-hi`.** It also takes the
  gradient track. The settings knob was near-black on the accent in the light theme.
- **The select's chevron is an `Icon`.** The settings select used a data-URI hard-coded to
  `#888`.
- **A bright red accent** (user's call, 2026-09-24), in `tokens.css` since the redesign
  started. Dark keeps the deep ink gradient with a white label rather than the app's dark ink
  on a light fill.
- **Filled status marks.** Badges, counters, dots and note marks are the tone's gradient, like
  the primary button. The GitLab panel's 12% wash read as washed out beside it; the wash is kept
  as `Badge soft`.
- **Statuses have their own palette.** In progress and info are blue, waiting is amber with
  dark ink, suggestion is cyan. None of them uses the brand red.
- **Danger is black.** A filled ink button, and in menus a bold item with the black fill under
  the pointer. With a red accent, a red danger looked like the primary.
- **Non-primary button labels are 400.** At 500 the red danger label read as bold and competed
  with the primary.
- **A single definition for each state.** "Selected" and "on" each have one drawing (see
  [State](#state-the-same-way-everywhere)); the app had six and five.

## Keeping it true

`check:kit` (`ui/scripts/check-kit.mjs`) fails when:

- an exported component in `ui/src/kit/components/` has no specimen, meaning it is not rendered
  by any chapter or kit component;
- an exported component is not named in this file;
- a chapter in `Kit.tsx`'s `CHAPTERS` is not listed in the table above;
- a file under `ui/src/kit/` imports anything outside the allowed set (rule 6);
- the whole page fails to render under node, or renders a `class="…undefined"`.

The kit's stylesheets are also held to every app-wide check. `check:ui-scale`, `check:motion`,
`check:theme`, `check:ui-icons` and `check:menus` all scan `ui/src/kit/` like any other
stylesheet.

**When you add or change a component**, work through these:

- the component and its module;
- a specimen showing each variant and state, with a `use` line (when to reach for it) and a
  `spec` table for anything with numbers;
- its entry in this file;
- the page, viewed in both themes and at UI size 11 and 17;
- `pnpm --dir ui run check:kit`, followed by the other `check:*` scripts.

**When the redesign moves a surface onto the kit**, update that row in
`page/chapters/Inventory.tsx`. Once a row reaches zero, delete it.
