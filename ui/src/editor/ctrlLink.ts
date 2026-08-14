/**
 * Ctrl+hover and Ctrl+click — one extension, because they are one gesture.
 *
 * Holding Ctrl over an identifier underlines it; clicking it goes to the declaration, or, when the
 * pointer is already *on* the declaration, lists the usages. IDEA's arrangement, and VS Code's
 * rule for the discriminator.
 *
 * # Why both handlers are in this file
 *
 * The underline is a promise about what the click will do. Split across two modules — a hover
 * plugin here, a `mousedown` in `EditorSurface.tsx` — the two would each hold their own idea of
 * *which word is under the pointer*, and any drift between them is an affordance that lights up
 * one identifier and acts on another. Here they call one [`wordTargetAt`], key one cache, and read
 * one [`intent`]. The click used to live in `EditorSurface`'s `domEventHandlers` and was moved
 * here for exactly that reason; nothing else about it changed.
 *
 * # Why the extension and not `hoverTooltip`
 *
 * CodeMirror's hover machinery exists to show a *tooltip*, on a 300 ms delay, positioned against
 * the pointer. What is wanted here is a mark on a range and a cursor change, which is
 * `crosshairCursor`'s mechanic — a `ViewPlugin` with `eventObservers` — one level down.
 *
 * # Observers, not handlers, for everything but the click
 *
 * `eventObservers` are registered on `contentDOM` and attached passively when nothing wants to
 * handle the event, so they cannot `preventDefault` and cannot get between the key gate and a
 * keystroke. Only `mousedown` is a real handler, because claiming the click is the point.
 *
 * # The five ways the underline has to go away
 *
 * Each of these is a bug if missed, and the fifth is the one that is easy to forget:
 *
 * | trigger | why |
 * | --- | --- |
 * | Ctrl released | `endsCtrlHold`, which checks the key *identity* first — WebKitGTK reports the pre-release mask |
 * | the pointer leaves the buffer | otherwise it stays lit under a menu |
 * | the window loses focus, or is hidden | Alt+Tab with Ctrl down delivers the keyup to the **other** app |
 * | the document changes | including the agent editing the file under the user |
 * | **the buffer scrolls** | the pointer is stationary and the *text under it* moves |
 */
import { syntaxTree, syntaxTreeAvailable } from '@codemirror/language'
import { StateEffect, StateField, type Extension } from '@codemirror/state'
import {
  Decoration,
  EditorView,
  ViewPlugin,
  type DecorationSet,
  type ViewUpdate,
} from '@codemirror/view'
import {
  HOVER_TIMEOUT_MS,
  SETTLE_MS,
  askableToken,
  endsCtrlHold,
  holdsCtrl,
  hoverPlan,
  onGlyph,
  underlines,
} from './codeIntelGate'
import {
  bumpDocGeneration,
  cachedResolution,
  ctrlActivate,
  forgetCodeIntel,
  resolveWord,
  type WordTarget,
} from './codeIntel'

/**
 * The word at a document position, or `null`.
 *
 * **The shared normaliser.** `column` is the word's start, not the caller's own position, so the
 * pointer anywhere in `parse` and a caret anywhere in `parse` ask the same question and share one
 * cached answer. The Rust discriminator's containment test is half-open precisely so that asking
 * at the first character of a declaration still lands inside the declaration's own range.
 */
export function wordTargetAt(view: EditorView, pos: number): WordTarget | null {
  const range = view.state.wordAt(pos)
  if (range === null) return null
  const line = view.state.doc.lineAt(range.from)
  return {
    from: range.from,
    to: range.to,
    line: line.number,
    // A CodeMirror document offset is already a UTF-16 index, which is what this wire wants.
    column: range.from - line.from + 1,
    text: view.state.sliceDoc(range.from, range.to),
  }
}

/**
 * Gates 1 to 3: is there an identifier under this point?
 *
 * Returns `null` for anything that is not worth a round trip, and the three refusals are in
 * increasing order of cost so the cheapest rejection happens first.
 */
function targetAtPoint(view: EditorView, x: number, y: number): WordTarget | null {
  const pos = view.posAtCoords({ x, y })
  if (pos === null) return null

  /*
   * Gate 1. `posAtCoords` in precise mode still answers with the line-end position for a pointer
   * parked in the empty space to the right of a line, so without this, hovering blank space
   * underlines the last word of the line — and the click would then keep that promise. Upstream's
   * own `HoverPlugin.startHover` rule, one character width of tolerance on each side.
   */
  const rect = view.coordsAtPos(pos)
  if (rect === null) return null
  if (!onGlyph(x, y, rect, view.defaultCharacterWidth)) return null

  // Gate 2. `wordAt` answers `null` when neither neighbour is a word character, which rejects
  // whitespace and runs of punctuation without a tree.
  const word = wordTargetAt(view, pos)
  if (word === null) return null

  /*
   * Gate 3, and the one that removes most of a page of source.
   *
   * `StreamLanguage` names each of its token types after the string the tokenizer returned, so the
   * node's `name` *is* `comment`, `string`, `keyword`, `variableName` — answerable in the webview
   * for nothing. `cide-lang` cannot help here even in principle: its own header says it has "no
   * references, no definitions and no types", it exposes only file outlines, and it lives behind
   * IPC, which is the round trip this ladder exists to avoid.
   *
   * Both failure modes fail **open**, deliberately: a buffer past `HIGHLIGHT_LIMIT_BYTES` loads no
   * language at all and a `StreamLanguage` parses lazily, so an unparsed region is "we do not
   * know", not "this is punctuation". `syntaxTreeAvailable` is the guard; `ensureSyntaxTree` and
   * `forceParsing` are deliberately **not** called — forcing a parse on the pointer path is the
   * cost this whole module is written to avoid.
   */
  if (syntaxTreeAvailable(view.state, word.to)) {
    const node = syntaxTree(view.state).resolveInner(word.from, 1)
    if (!askableToken(node.name)) return null
  }
  return word
}

/** Set or clear the underlined range. */
const setLink = StateEffect.define<{ from: number; to: number } | null>()

/**
 * The mark itself.
 *
 * A `Decoration.mark` and not `contentAttributes`, which is what `crosshairCursor` uses: that one
 * wants the whole content area to change cursor, and this one must be a hand **only over the
 * underlined word**, which is what IDEA does. A mark span gets the cursor for free rather than
 * needing a second mechanism.
 *
 * The class is global and `cide-`-prefixed, styled beside the `.cide-tk-*` block in
 * `EditorSurface.module.css`, so this module needs no CSS-Modules import — the same arrangement
 * `highlight.ts` uses, and `check:editor` asserts that every such class is styled there.
 */
const mark = Decoration.mark({ class: 'cide-ctrl-link' })

const linkField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(deco, tr) {
    // Removal trigger 4. An edit invalidates the answer the mark was drawn from, and re-mapping
    // the range through the change set would keep a stale promise on screen.
    if (tr.docChanged) return Decoration.none
    for (const effect of tr.effects) {
      if (effect.is(setLink)) {
        return effect.value === null
          ? Decoration.none
          : Decoration.set([mark.range(effect.value.from, effect.value.to)])
      }
    }
    return deco
  },
  provide: (field) => EditorView.decorations.from(field),
})

/**
 * The hover half: gates 0, 4, 5 and 6, plus the five removals.
 *
 * `project` may be `undefined` — a buffer outside any project has nothing to resolve against, and
 * the plugin then does nothing at all rather than drawing an underline no click could honour.
 */
class CtrlLinkPlugin {
  private settle: ReturnType<typeof setTimeout> | null = null
  /** The range currently marked, so a redundant dispatch is skipped. */
  private shown: { from: number; to: number } | null = null
  /** The last point the pointer was seen at, so a Ctrl *keydown* can act without a move. */
  private at: { x: number; y: number } | null = null
  /** Bumped on every clear, so an answer for a word the pointer has left is dropped. */
  private generation = 0
  private readonly onBlur = () => this.clear()
  private readonly onVisibility = () => {
    if (document.visibilityState !== 'visible') this.clear()
  }

  constructor(
    private readonly view: EditorView,
    private readonly project: string | undefined,
    private readonly path: string,
  ) {
    /*
     * Removal trigger 3, and it is not hypothetical.
     *
     * Alt+Tab with Ctrl held delivers the keyup to the *other* application, so this window never
     * hears that the hold ended and the underline stays lit until the pointer moves again.
     * `switcherStore.ts` documents the identical loss for the project switcher and installs the
     * identical pair of listeners; `visibilitychange` catches the workspace switch that `blur`
     * does not always produce under a tiling compositor.
     */
    window.addEventListener('blur', this.onBlur)
    document.addEventListener('visibilitychange', this.onVisibility)
    // Removal trigger 5's listener. `scroll` *is* delivered to CodeMirror's observers, but only
    // for the scroller it recognises; a pane that scrolls its own container would not reach one.
    // Direct is one line and cannot be wrong.
    this.view.scrollDOM.addEventListener('scroll', this.onBlur, { passive: true })
    mounted.set(path, (mounted.get(path) ?? 0) + 1)
  }

  /**
   * Removal trigger 4's local half.
   *
   * The generation in the *cache key* is bumped by the update listener below — one writer, so the
   * number cannot advance twice for one edit. What happens here is only this view forgetting what
   * it had drawn; the `StateField` clears the mark itself on the same transaction.
   */
  update(update: ViewUpdate): void {
    if (!update.docChanged) return
    this.shown = null
    this.generation += 1
  }

  destroy(): void {
    window.removeEventListener('blur', this.onBlur)
    document.removeEventListener('visibilitychange', this.onVisibility)
    this.view.scrollDOM.removeEventListener('scroll', this.onBlur)
    if (this.settle !== null) clearTimeout(this.settle)
    /*
     * The cache is module-level and shared between the split panes showing this path, so it is
     * dropped only when the **last** of them goes. Dropping it when the first unmounts would throw
     * away answers the other pane is still using, and would do it silently — as a slower hover
     * rather than a wrong one, which is the kind of regression nothing notices.
     */
    const left = (mounted.get(this.path) ?? 1) - 1
    if (left <= 0) {
      mounted.delete(this.path)
      forgetCodeIntel(this.path)
    } else {
      mounted.set(this.path, left)
    }
  }

  /** Removal trigger 2: the pointer left the text. */
  onLeave(): void {
    this.clear()
  }

  /** The pointer moved. Gate 0 first — it rejects essentially all pointer motion. */
  onMove(event: MouseEvent): void {
    this.at = { x: event.clientX, y: event.clientY }
    if (!holdsCtrl(event)) {
      this.clear()
      return
    }
    /*
     * Suppressed mid-drag. Without this, selecting text with Ctrl held flickers mark spans in and
     * out of the DOM under the pointer while CodeMirror is trying to extend a selection over the
     * same range.
     */
    if (event.buttons !== 0) {
      this.clear()
      return
    }
    this.consider(event.clientX, event.clientY)
  }

  /**
   * Ctrl went down with the pointer already parked.
   *
   * Only reachable while `contentDOM` has focus, which is the honest limit of this observer: a
   * lone `Control` keydown resolves to no binding, so the key gate passes it through and a focused
   * editor sees it. An **unfocused** editor under the pointer does not, and the underline appears
   * on the first pixel of movement instead. That divergence is accepted rather than fixed with a
   * window-level keydown listener — a second modifier latch outside the gate is a route into the
   * dispatcher that this feature does not need.
   */
  onKeyDown(event: KeyboardEvent): void {
    if (!holdsCtrl(event) || this.at === null) return
    this.consider(this.at.x, this.at.y)
  }

  /** Removal trigger 1. See `endsCtrlHold` for why the key identity is checked before the mask. */
  onKeyUp(event: KeyboardEvent): void {
    if (endsCtrlHold(event)) this.clear()
  }

  /**
   * Removal trigger 5. The pointer is stationary and the text under it has moved, so the mark is
   * now over a different word — and the click, which recomputes from its own coordinates, would
   * act on that different word. Easy to miss, and it leaves the underline visibly wrong.
   */
  onScroll(): void {
    this.clear()
  }

  private consider(x: number, y: number): void {
    const project = this.project
    if (project === undefined) return
    const word = targetAtPoint(this.view, x, y)
    // Gate 4 is consulted here rather than inside the plan, because reading the cache is what
    // *makes* the plan `draw` or `hide` — the plan itself has to stay pure so the drag cost can be
    // measured headlessly. See `hoverPlan`.
    const cached = word === null ? undefined : cachedResolution(this.path, word, HOVER_TIMEOUT_MS)
    const step = hoverPlan(word, this.shown, cached)
    if (step === 'keep') return
    if (step === 'clear' || step === 'hide') {
      this.clear()
      return
    }
    if (word === null) return
    if (step === 'draw') {
      this.arm()
      this.draw(word)
      return
    }

    /*
     * Gate 5: a trailing debounce, restarted on every move — the opposite of `docSync`'s throttle,
     * and correctly so. Starvation under continuous motion is what is wanted: there is nothing
     * worth showing while the pointer is moving, and a whole drag across a line collapses into one
     * request at the end of it. `clear` first is what does the restarting.
     */
    this.clear()
    const mine = this.generation
    this.settle = setTimeout(() => {
      this.settle = null
      if (mine !== this.generation) return
      void resolveWord(project, this.path, word, HOVER_TIMEOUT_MS).then((answer) => {
        /*
         * Dropped on arrival if anything has moved on — the pointer left, Ctrl came up, the buffer
         * changed. The same shape `diagnosticsStore` uses for the same problem, and the reason
         * there is no protocol cancel here: the ladder above is the real protection, and a reply
         * that lost its race costs one comparison.
         *
         * **Every outcome is swallowed.** A hover must never report: `goToDefinition`'s `report()`
         * turns each non-`found` answer into a toast, and on this path that would be a toast every
         * time the pointer settled on a comment.
         */
        if (mine !== this.generation) return
        if (underlines(answer.kind)) this.draw(word)
      })
    }, SETTLE_MS)
  }

  private arm(): void {
    if (this.settle !== null) {
      clearTimeout(this.settle)
      this.settle = null
    }
  }

  private draw(word: { from: number; to: number }): void {
    this.shown = { from: word.from, to: word.to }
    this.view.dispatch({ effects: setLink.of({ from: word.from, to: word.to }) })
  }

  /** Take the mark down and make any in-flight answer inert. */
  private clear(): void {
    this.arm()
    this.generation += 1
    if (this.shown === null) return
    this.shown = null
    this.view.dispatch({ effects: setLink.of(null) })
  }
}

/**
 * How many mounted editors are showing each path.
 *
 * A split shows one file in two panes, and the answers are cached module-level so both share them.
 * Dropping the cache when the *first* of the two unmounts would throw away answers the other is
 * still using — and would do it silently, as a slower hover rather than a wrong one, which is the
 * kind of regression nothing notices.
 */
const mounted = new Map<string, number>()

/**
 * Ctrl+hover and Ctrl+click for one buffer.
 *
 * `project` is `undefined` for a buffer outside any project: there is nothing to resolve against,
 * so no underline is drawn and the click falls through to CodeMirror's ordinary selection — which
 * is what happened before this existed.
 */
export function ctrlLink(project: string | undefined, path: string): Extension {
  return [
    linkField,
    ViewPlugin.define((view) => new CtrlLinkPlugin(view, project, path), {
      eventObservers: {
        mousemove(event) {
          this.onMove(event)
        },
        // Removal trigger 2. Registered on `contentDOM`, which is where CodeMirror attaches
        // observers, so this fires when the pointer leaves the *text* rather than the pane.
        mouseleave() {
          this.onLeave()
        },
        // Both only reach an editor that has focus, which is the honest limit of the keydown
        // half — see `onKeyDown`. The keyup half is unaffected: a hold that started with a
        // pointer move over an unfocused editor is ended by `mouseleave`, `blur` or the scroll
        // listener instead.
        keydown(event) {
          this.onKeyDown(event)
        },
        keyup(event) {
          this.onKeyUp(event)
        },
        scroll() {
          this.onScroll()
        },
      },
    }),
    /*
     * Alt adds carets; Ctrl navigates. IDEA's arrangement, and the opposite of CodeMirror's.
     *
     * CodeMirror's default for this facet is `browser.mac ? metaKey : ctrlKey` — so on Linux
     * **Ctrl+click adds a cursor**, which is the chord this whole extension needs. Overriding the
     * facet moves that to Alt. It lives here rather than in `EditorSurface` because it is part of
     * this gesture: deleting the extension without it would leave Ctrl+click doing nothing, and
     * deleting the facet without the extension would leave it adding carets again.
     *
     * Alt+click also reaches `rectangularSelection`'s style (its filter is `altKey && button == 0`,
     * drag or not), and that style consults *this* facet for its `multiple` argument — so one
     * override buys both halves. Two honest divergences from IDEA follow and are accepted:
     * Alt+drag *adds* its rectangle to the selection, and Alt+click cannot *remove* a caret,
     * because `rectangularSelection`'s filter pre-empts the branch that would.
     */
    EditorView.clickAddsSelectionRange.of((event) => event.altKey),
    EditorView.domEventHandlers({
      /*
       * The click.
       *
       * On `mousedown` rather than `click`, because CodeMirror starts its own selection gesture on
       * mousedown: by the time a `click` fired the caret would already have moved and the selection
       * been replaced, so the jump would be computed from the right position but leave the buffer
       * visibly disturbed. Returning `true` marks it handled, and `preventDefault` stops the drag
       * gesture from ever starting.
       *
       * `button === 0` so a Ctrl+right-click still opens the context menu — where the same two
       * actions live for anyone who prefers a menu. `metaKey` is accepted too: ⌘-click is the
       * platform equivalent on macOS, and `platform_layer` rewrites the Ctrl+B binding the same
       * way. `!altKey` so Alt+Ctrl+click stays a multi-caret gesture.
       */
      mousedown: (event, target) => {
        if (event.button !== 0 || !holdsCtrl(event) || event.altKey) return false
        if (project === undefined) return false
        const word = targetAtPoint(target, event.clientX, event.clientY)
        if (word === null) return false
        event.preventDefault()
        /*
         * Focus explicitly, because returning `true` skips the code that would have done it.
         *
         * CodeMirror focuses the content DOM inside its own `mousedown` handler and only once it
         * has produced a selection style, so handling the event here bypasses that entirely: a
         * Ctrl+click into an *unfocused* pane used to act correctly and leave the keyboard pointing
         * at whatever had focus before, and the next keystroke went to the old pane.
         */
        if (!target.hasFocus) target.focus()
        ctrlActivate(project, path, word)
        return true
      },
    }),
    /*
     * The **one** writer of the document generation.
     *
     * Every cached answer for this file is keyed on it, so bumping it is what makes an edit
     * invalidate the lot — an added `use`, a renamed local, the agent rewriting the buffer under
     * the user. Deliberately *not* also done inside the plugin: two writers would advance the
     * number twice for one edit, which is harmless today and is precisely the kind of "harmless"
     * that stops being true when somebody starts comparing generations across views.
     */
    EditorView.updateListener.of((update) => {
      if (update.docChanged) bumpDocGeneration(path)
    }),
  ]
}
