/**
 * Code completion: the CodeMirror source, the keymap, and what accepting a row actually does.
 *
 * Every *decision* is in `completionGate.ts` and every snippet translation in `lspSnippet.ts`,
 * both import-free and both driven by `ui/scripts/check-completion.mjs`. What is left here is the
 * IPC, the document sync and the transactions — the parts a headless check could not run anyway.
 * The same split `codeIntel.ts`/`codeIntelGate.ts` makes, and for the same reason.
 *
 * # The three things this file gets right that are easy to get wrong
 *
 * 1. **The document is synced before the question is asked.** `docSync.ts` is a 300 ms trailing
 *    throttle, so the server's copy is routinely behind the buffer — and a completion computed
 *    against text that does not contain the caret is not late, it is about a different file.
 *    [`source`] awaits `syncNow` first, and the ordering that makes that sufficient is written
 *    out at `syncNow`.
 * 2. **An implicit failure is silent.** This popup opens by itself many times a minute; a notice
 *    per failed query would train the user to ignore the notice channel. Only a Ctrl+Space — a
 *    key the user actually pressed — reports why nothing happened.
 * 3. **An auto-import row is resolved before it is accepted, and a failed resolve refuses the
 *    accept.** rust-analyzer offers no auto-import candidates *at all* to a client that cannot
 *    resolve lazily, so for those rows the `use` line genuinely does not exist until
 *    [`applyRow`] asks for it. Inserting the identifier without it leaves the file not compiling,
 *    which is worse than not completing — so the accept is refused with a sentence instead.
 */
import {
  acceptCompletion,
  autocompletion,
  closeCompletion,
  moveCompletionSelection,
  snippet,
  startCompletion,
  type Completion,
  type CompletionContext,
  type CompletionResult,
} from '@codemirror/autocomplete'
import { ChangeSet, type Extension } from '@codemirror/state'
import { EditorView, keymap } from '@codemirror/view'
import { diagnostics as diagnosticsApi, type ProjectId } from '@/ipc/client'
import type { CompletionEdit, CompletionItem } from '@/ipc/generated'
import { completionBadge } from '@/overlays/format'
import { syncNow } from './docSync'
import {
  TYPING_DELAY_MS,
  canFilterLocally,
  mapCompletion,
  needsResolve,
  resolveFailureSentence,
  shouldAsk,
  wordStart,
} from './completionGate'
import { toCodeMirrorSnippet } from './lspSnippet'

/**
 * Everything one editor's completion needs that is not in its state.
 *
 * `path` is the buffer's real path and not its `identity`: it names the document the language
 * server was told about through `docSync`, and a revision buffer or a diff side must not be able
 * to claim that name. Those surfaces do not mount this extension at all.
 */
interface CompletionHost {
  readonly project: ProjectId
  readonly path: string
}

/** What `EditorSurface` knows about completion when it builds its extension array. */
export interface CompletionConfig {
  /**
   * `undefined` in a surface that has no project to resolve against — a scratch buffer, a
   * fixture. There is nothing to ask, so nothing is mounted.
   */
  readonly project: string | undefined
  /** The buffer's real path — the name the language server was told through `docSync`. */
  readonly path: string
  /** `settings.editor.completion`. */
  readonly enabled: boolean
  /** `settings.editor.completionOnTyping`. */
  readonly onTyping: boolean
}

/**
 * What a row needs carried from the reply to the moment it is accepted.
 *
 * Hung off the `Completion` object rather than kept in a module-level map, because CodeMirror
 * owns the lifetime of these — it discards a list when a newer one arrives, and anything keyed
 * beside it would have to be told. This rides along and is collected with it.
 */
interface RowPayload {
  readonly item: CompletionItem
  readonly token: number
  readonly host: CompletionHost
}

/** The extra fields this module hangs on a `Completion`. */
type CideCompletion = Completion & { readonly cide: RowPayload }

/**
 * Put a sentence on screen through the failure channel.
 *
 * `chrome/Failures.tsx` listens for `unhandledrejection`, which is how a keystroke's outcome
 * becomes visible with no `.catch` at the call site — the same trick `codeIntel.ts` and
 * `goToDefinition.ts` use, and for the same reason: a gesture that silently does nothing is
 * indistinguishable from one wired to nothing.
 */
function report(message: string): void {
  void Promise.reject(new Error(message))
}

/**
 * Ask the server what can go here.
 *
 * Returns `null` for "no popup" in every failing case, which is most of what this function does.
 * The one exception is an explicit request that came back `unavailable`: the user pressed
 * Ctrl+Space and is owed an answer.
 */
async function source(
  host: CompletionHost,
  context: CompletionContext,
): Promise<CompletionResult | null> {
  const { state, pos, explicit } = context
  const line = state.doc.lineAt(pos)
  const before = state.sliceDoc(line.from, pos)

  if (!shouldAsk(before, explicit)) return null

  /*
   * The text and the position both come from `context.state`, which is immutable — so what the
   * server is told and what it is asked about cannot disagree, however much the user types while
   * this is in flight. If they do type, CodeMirror aborts this query and starts another.
   */
  await syncNow(host.path, state.doc.toString())
  if (context.aborted) return null

  const answer = await diagnosticsApi.completion(
    host.project,
    host.path,
    line.number,
    // 1-based, UTF-16 — the units `position_params` converts from. `pos - line.from` is already
    // a UTF-16 code-unit offset, because that is what a JavaScript string index is.
    pos - line.from + 1,
    // The character the caret is sitting behind, for Rust to weigh against the server's declared
    // trigger list. `null` at the start of a line.
    before.slice(-1) === '' ? null : before.slice(-1),
  )

  if (answer.kind === 'unavailable') {
    // Silent unless the user asked — see the module header. A superseded request is the common
    // outcome here and narrating it would put a notice on screen for every third keystroke.
    if (explicit) report(answer.reason)
    return null
  }
  if (context.aborted) return null

  const from = line.from + wordStart(before)
  const options: CideCompletion[] = answer.items.map((item) => ({
    ...mapCompletion(item),
    apply: (view: EditorView, completion: Completion, at: number, to: number) => {
      applyRow(view, completion as CideCompletion, at, to)
    },
    cide: { item, token: answer.token, host },
  }))

  return {
    from,
    options,
    /*
     * Whether further typing may filter this list without asking again — `completionGate`'s rule,
     * and both halves of it matter. The pattern is the identifier alphabet: CodeMirror keeps the
     * list while the text between `from` and the caret still matches it, which is exactly as long
     * as the user is extending the same word.
     */
    ...(canFilterLocally(answer.incomplete, answer.truncated)
      ? { validFor: /^[\p{L}\p{N}_$]*$/u }
      : {}),
  }
}

/**
 * Accept one row: fetch anything deferred, land the extra edits, then insert.
 *
 * # Why the edits go first and the insert second
 *
 * `additionalTextEdits` are always outside the completion range — an import at the top of the
 * file — and they are stated in offsets against the document *as the server saw it*. Applying
 * them first and then mapping `from`/`to` through that change set is what keeps the insert
 * landing where the user's caret is: an import inserted above shifts every later offset, and an
 * insert computed before it would go in a line too high.
 *
 * Both dispatches carry `userEvent: 'input.complete'`, which is what lets `@codemirror/commands`'
 * history group them. One Ctrl+Z undoes the completion *and* its import, which is the only
 * behaviour that is not surprising.
 */
function applyRow(view: EditorView, completion: CideCompletion, from: number, to: number): void {
  const { item, token, host } = completion.cide

  if (!needsResolve(item)) {
    landRow(view, item, item.extraEdits, from, to)
    return
  }

  void diagnosticsApi
    .completionResolve(host.project, token, item.resolve ?? 0)
    .then((answer) => {
      if (answer.kind === 'unavailable') {
        /*
         * **The accept is refused, not completed without the import.**
         *
         * This is the one place in the feature where doing half the job is available and wrong.
         * Inserting `HashMap` and silently not adding `use std::collections::HashMap` leaves a
         * file that does not compile, and leaves it looking exactly like a successful
         * completion — the user has no reason to check. Not completing at all is recoverable in
         * one keystroke and says so out loud.
         */
        report(resolveFailureSentence(item.label, answer.reason))
        return
      }
      landRow(view, item, answer.extraEdits, from, to)
    })
    .catch((error: unknown) => {
      report(resolveFailureSentence(item.label, String(error)))
    })
}

/**
 * The two transactions an accepted row turns into.
 *
 * Split out of [`applyRow`] so the resolved and unresolved paths cannot drift: they differ only
 * in where the edits came from, and every rule about *applying* them — the ordering, the mapping,
 * the snippet fallback, the user event — is written once.
 *
 * A late arrival is dropped rather than applied. Between the resolve going out and coming back
 * the user may have typed, undone, or switched files, and `from`/`to` describe a document that no
 * longer exists; CodeMirror would either throw out of `dispatch` — which unmounts the React root
 * — or splice text into the middle of a word.
 */
function landRow(
  view: EditorView,
  item: CompletionItem,
  extras: readonly CompletionEdit[],
  from: number,
  to: number,
): void {
  /*
   * A late arrival is dropped rather than applied.
   *
   * Between the resolve going out and coming back the user may have typed, undone, or switched
   * files, and `from`/`to` describe a document that no longer exists. CodeMirror would either
   * throw out of `dispatch` — and an exception on this path unmounts the React root, taking the
   * window with it — or splice text into the middle of an unrelated word.
   */
  if (to > view.state.doc.length) return

  /*
   * The server's own replace range wins when it sent one, and only then.
   *
   * An `insertText`-only item carries no range and means "replace whatever the client thinks the
   * current word is", which is what `from`/`to` already are. A `textEdit`'s range is a statement,
   * and it is regularly *wider* than the word — gopls replacing `fmt.Pri` including the dot,
   * rust-analyzer replacing a whole `use` path — so ignoring it leaves the prefix behind and
   * produces `fmt.fmt.Println`.
   */
  let start = from
  let end = to
  if (item.replace !== null) {
    const ranged = toChange(view, item.replace)
    if (ranged !== null) {
      start = ranged.from
      end = ranged.to
    }
  }

  /*
   * The extra edits land first, and then the insert's range is mapped through them.
   *
   * `additionalTextEdits` are always outside the completion range — an import at the top of the
   * file — and every offset in play here, `start`/`end` included, is stated against the document
   * *before* any of them. Inserting a `use` line shifts everything below it, so an insert
   * computed in the old coordinates would land a line too high.
   *
   * `ChangeSet.of` rather than dispatching and comparing document lengths: `mapPos` is exact for
   * edits anywhere, including one *below* the caret or one that deletes, where arithmetic on the
   * length difference quietly gives the wrong sign.
   */
  if (extras.length > 0) {
    const changes = extras
      .map((edit) => toChange(view, edit))
      .filter((change): change is DocChange => change !== null)
    if (changes.length > 0) {
      const set = ChangeSet.of(changes, view.state.doc.length)
      view.dispatch({ changes: set, userEvent: 'input.complete', scrollIntoView: false })
      // Associate forward, so an import inserted at exactly the position the completion starts
      // pushes the completion right rather than swallowing it.
      start = set.mapPos(start, 1)
      end = set.mapPos(end, 1)
      if (end > view.state.doc.length) return
    }
  }

  const plan = toCodeMirrorSnippet(item.insert, item.snippet)
  if (plan.kind === 'snippet') {
    // `snippet()` dispatches its own transaction, including the selection that puts the caret in
    // the first field. It takes `{ state, dispatch }`, which an `EditorView` satisfies.
    snippet(plan.template)(view, completionOf(item), start, end)
    return
  }
  view.dispatch({
    changes: { from: start, to: end, insert: plan.template },
    selection: { anchor: start + plan.template.length },
    userEvent: 'input.complete',
  })
}

/** A CodeMirror change spec, named so the type predicate above can say it. */
interface DocChange {
  from: number
  to: number
  insert: string
}

/** One cide edit as a CodeMirror change, or `null` when it names a position outside the doc. */
function toChange(
  view: EditorView,
  edit: CompletionEdit,
): DocChange | null {
  const from = offsetOf(view, edit.line, edit.column)
  const to = offsetOf(view, edit.endLine, edit.endColumn)
  if (from === null || to === null || to < from) return null
  return { from, to, insert: edit.text }
}

/**
 * A 1-based line and UTF-16 column as a document offset, or `null` when the file is shorter than
 * that.
 *
 * Clamping rather than throwing, for `lintMap.ts`'s reason: these coordinates came from another
 * process and describe the file as it was when it answered. A line past the end throws out of
 * `dispatch`, and an exception on that path takes the whole React root down with it — the
 * failure `EditorSurface`'s `try` around the view constructor exists for, arriving by a different
 * road.
 */
function offsetOf(view: EditorView, line: number, column: number): number | null {
  const doc = view.state.doc
  if (line < 1 || line > doc.lines) return null
  const at = doc.line(line)
  return Math.min(at.from + Math.max(0, column - 1), at.to)
}

/** A bare `Completion` for `snippet()`, which reads only the label. */
function completionOf(item: CompletionItem): Completion {
  return { label: item.label }
}

/**
 * The completion extensions for one editor.
 *
 * # Two non-defaults, and both are load-bearing
 *
 * **`defaultKeymap: false`.** CodeMirror's own `completionKeymap` is installed at `Prec.highest`,
 * which would put its `Escape` above the find bar's — see the note on the keymap below for why
 * this array wants the opposite order. The list below is that keymap restated at ordinary
 * precedence, with `Tab` added: **Tab and Enter both accept.**
 *
 * **`icons: false`.** CodeMirror's base theme draws its completion icons as Unicode glyphs in
 * `::after` content, and `check:ui-icons` bans a Unicode symbol drawn as an icon outside a
 * per-file allowlist — the rule that exists because this app drew a hundred and thirty of them
 * that way until M23. The kind reaches the row as a `cm-completionIcon-<kind>` class instead, and
 * `EditorSurface.module.css` draws it as a text badge in the vocabulary `overlays/format.ts`
 * already uses for the File Structure popup and Go-to-Symbol.
 */
export function completionExtensions(config: CompletionConfig): Extension {
  /*
   * Nothing at all when it is off, rather than a mounted extension with the source stubbed out.
   *
   * The difference shows up on the keyboard: a mounted `autocompletion()` claims Escape and the
   * arrow keys through the keymap below, and `closeCompletion` returning false is not quite the
   * same as the binding not being there. Mounting nothing is also the only version where turning
   * the setting off is *certain* to cost nothing.
   */
  if (!config.enabled || config.project === undefined) return []
  const host: CompletionHost = { project: config.project, path: config.path }
  return [
    autocompletion({
      override: [(context) => source(host, context)],
      activateOnTyping: config.onTyping,
      activateOnTypingDelay: TYPING_DELAY_MS,
      defaultKeymap: false,
      icons: false,
      /*
       * The kind badge, rendered here rather than by CodeMirror.
       *
       * `icons: false` above means CodeMirror never creates its `.cm-completionIcon` element at
       * all — it is built inside `if (config.icons)` — so this is not decoration over the top of
       * its icons, it is the replacement for them. Discovered the direct way: the first version
       * set `icons: false` and styled `.cm-completionIcon-*`, and every badge was dead CSS.
       *
       * The label and the tone come from `overlays/format.ts::completionBadge`, which is the same
       * function the File Structure popup and Go-to-Symbol use, so a `function` cannot read blue
       * in one surface and unlabelled in another.
       *
       * **Data attributes, not classes.** `optionClass` and a class here would both need a class
       * *name*, and this stylesheet is a CSS module whose names are hashed at build time — a
       * literal handed to CodeMirror would match nothing. An attribute survives the hashing, and
       * it is the same trick `foldMarker` uses with `data-open`.
       *
       * `position: 20` is where CodeMirror's own icons sit, which puts the badge before the
       * label and after nothing.
       */
      addToOptions: [
        {
          position: 20,
          render: (option) => {
            const row = (option as Partial<CideCompletion>).cide
            if (row === undefined) return null
            const badge = completionBadge(row.item.kind)
            const mark = document.createElement('span')
            mark.textContent = badge.label
            mark.dataset['cideBadge'] = badge.tone
            // Read by the stylesheet to strike the row through. On the badge rather than on a
            // marker of its own, so one element carries both facts and the row needs one `:has()`.
            if (row.item.deprecated) mark.dataset['cideDeprecated'] = 'true'
            /*
             * `aria-hidden`, because the badge is a *duplicate* of information the row already
             * carries: CodeMirror puts the kind on the option's own `aria-label` through its
             * `type`, and a screen reader announcing "FN push" reads the abbreviation as a word.
             */
            mark.setAttribute('aria-hidden', 'true')
            return mark
          },
        },
      ],
      // The rows are already ranked by the server and re-ranked by `boost`; a heading per source
      // would be a heading over one source. Left off deliberately rather than by omission.
      closeOnBlur: true,
    }),
    /*
     * `Prec.highest` is **not** needed and would be wrong.
     *
     * CodeMirror puts its own `completionKeymap` there, and the instinct is to match it. But this
     * array is added to `EditorSurface`'s `shared` *before* the main keymap and after
     * `findExtensions()`, and that order is exactly what is wanted: Escape closes the find bar
     * first when both are open (`closeSearchPanel` returns false when it is not), then this
     * closes the popup, then `defaultKeymap`'s `simplifySelection` gets a look. Raising this
     * above the find bar would make Escape close a popup the user may not have been looking at
     * and leave the bar they were.
     */
    keymap.of([
      /*
       * Tab accepts. `acceptCompletion` returns `false` when no popup is open, so the stroke
       * falls through to `indentWithTab` — which is why this array must be reachable *before*
       * that binding and why `check:editor`'s composed keymap has to include it.
       *
       * Nothing binds `tab` or `ctrl+space` in `cide-core::keymap`, and a Rust test says so:
       * the key gate is a window **capture** listener, so a binding there would swallow both
       * before CodeMirror was ever offered them.
       */
      { key: 'Tab', run: acceptCompletion },
      { key: 'Ctrl-Space', run: startCompletion },
      // macOS sends no usable `Ctrl-Space`; these are the two chords CodeMirror's own keymap
      // offers there, kept so the feature is reachable on a platform cide has not run on yet.
      { mac: 'Alt-`', run: startCompletion },
      { mac: 'Alt-i', run: startCompletion },
      { key: 'Escape', run: closeCompletion },
      { key: 'ArrowDown', run: moveCompletionSelection(true) },
      { key: 'ArrowUp', run: moveCompletionSelection(false) },
      { key: 'PageDown', run: moveCompletionSelection(true, 'page') },
      { key: 'PageUp', run: moveCompletionSelection(false, 'page') },
      /*
       * Enter accepts as well as Tab.
       *
       * This started the other way round — Tab only, with Enter left to insert a newline — on the
       * argument that Enter-accepts eats a line break whenever the popup is open and the user had
       * not noticed it. That argument is real and it is the reason the setting exists in other
       * editors. It is also not what anybody expects when they first meet a completion popup, and
       * the first thing this shipped with was a report that Enter "does nothing".
       *
       * The fallthrough is what makes it safe enough to be the default: `acceptCompletion`
       * returns `false` when no popup is open, so Enter reaches `defaultKeymap`'s
       * `insertNewlineAndIndent` untouched in every buffer where nothing is being suggested,
       * which is nearly all of them nearly all of the time.
       *
       * It is listed **after** the arrow keys and Escape rather than beside Tab so the reading
       * order of this array matches what a user tries: move the selection, dismiss it, then
       * commit to it.
       */
      { key: 'Enter', run: acceptCompletion },
    ]),
  ]
}
