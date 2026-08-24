/**
 * Settings → Editor → Formatters: which program reformats which language. (M26)
 *
 * # What this screen is for
 *
 * Ctrl+Alt+F asks the language server by default, and both shipped servers are bundled — so Rust
 * and Go format out of the box and never need a row here. Everything else does: nine of the
 * eleven builtin languages have no server, and for those this screen *is* the feature.
 *
 * A row also **overrides** a server that would otherwise answer, which is the other reason it
 * exists: it is the only lever over the shipped rustfmt short of switching the whole server to
 * the user's own build. `cide_core::format`'s header argues the precedence.
 *
 * # One input per argument, and why the screen looks like this
 *
 * The stored value is a `Vec<String>` that reaches `execvp` directly — there is no shell, so
 * `$HOME`, `*`, `|` and `&&` are ordinary argument bytes. A single "command" field would need a
 * quoting parser cide would have to invent and would get wrong at the first path with a space in
 * it; `ClaudeCli::args` states that at length and this screen is deliberately shaped like the
 * one that renders it. The readout under each entry joins the tokens with `␣` rather than a
 * space, so nobody reads it as a command line a shell would parse.
 *
 * # Commit on blur
 *
 * Each write is an IPC round trip, a `workspace.json` rewrite and a broadcast to every open
 * window, so committing per keystroke would do that per character. Same choice, and the same
 * reason, as `ClaudeCliSection`'s `TokenList` and `ProxySection`'s fields.
 *
 * The inputs are **uncontrolled** and keyed by content as well as position, for the reason
 * `TokenList` writes out in full: `defaultValue` is read once at mount, so a key of index alone
 * lets React reuse a node for a shifted-up value, and the next blur commits the stale text back
 * — which resurrects a row the user just deleted.
 */
import type { EditorSettings } from '@/ipc/client'
import { BUILTIN_LANGUAGES } from '@/editor/builtinLanguages'
import styles from './FormattersSection.module.css'

/** `${file}` and `${dir}` — the only two, and the same constants `cide_core::format` defines. */
const PLACEHOLDER_HINT = '${file} and ${dir} are substituted; nothing else is.'

/**
 * The languages offered by the id field's datalist.
 *
 * From the generated builtin table rather than a list typed here, so a language added in Rust
 * appears without a second edit — and so a `codegen --check` failure is what catches a rename
 * rather than a formatter row that silently stops matching.
 *
 * It is a *suggestion* list and not a closed set: an extension contributes languages too, and
 * those ids are equally valid here. A `<datalist>` suggests without constraining, which is
 * exactly that shape.
 */
const BUILTIN_IDS = BUILTIN_LANGUAGES.map((language) => language.id)

type Entry = readonly [string, readonly string[]]

export function FormattersSection({
  editor,
  patch,
}: {
  editor: EditorSettings
  patch: (next: Partial<EditorSettings>) => void
}) {
  // `Object.entries` over the map Rust sends. Sorted by Rust already (a `BTreeMap`), and re-sorted
  // here so a locally-added row lands where it will be after the round trip rather than jumping.
  const entries: Entry[] = Object.entries(editor.formatters).sort(([a], [b]) => a.localeCompare(b))

  const commit = (next: Entry[]) => {
    const formatters: Record<string, string[]> = {}
    for (const [language, argv] of next) {
      // A row whose language is blank is one the user is still typing. Dropping it here rather
      // than storing `""` keeps the map free of a key nothing can ever match.
      if (language.trim() === '') continue
      formatters[language.trim()] = [...argv]
    }
    patch({ formatters })
  }

  const replace = (at: number, entry: Entry) => {
    const next = entries.slice()
    next[at] = entry
    commit(next)
  }

  return (
    <div className={styles.list}>
      {entries.map(([language, argv], at) => (
        <div key={`${at}:${language}`} className={styles.entry}>
          <div className={styles.entryHead}>
            <input
              className={`${styles.input} ${styles.language}`}
              type="text"
              spellCheck={false}
              list="cide-formatter-languages"
              aria-label="Language"
              placeholder="language id"
              defaultValue={language}
              onBlur={(event) => {
                if (event.currentTarget.value === language) return
                replace(at, [event.currentTarget.value, argv])
              }}
            />
            <button
              type="button"
              className={styles.remove}
              aria-label={`Remove the formatter for ${language}`}
              title={`Remove the formatter for ${language}`}
              onClick={() => commit(entries.filter((_, index) => index !== at))}
            >
              ×
            </button>
          </div>

          <div className={styles.tokens}>
            {argv.map((token, index) => (
              <input
                key={`${index}:${token}`}
                className={`${styles.input} ${styles.token}`}
                type="text"
                spellCheck={false}
                aria-label={index === 0 ? 'Program' : `Argument ${index}`}
                placeholder={index === 0 ? 'program' : 'argument'}
                defaultValue={token}
                onBlur={(event) => {
                  if (event.currentTarget.value === token) return
                  const next = argv.slice()
                  next[index] = event.currentTarget.value
                  replace(at, [language, next])
                }}
              />
            ))}
            <button
              type="button"
              className={styles.add}
              onClick={() => replace(at, [language, [...argv, '']])}
            >
              Add argument
            </button>
            {argv.length > 1 && (
              <button
                type="button"
                className={styles.remove}
                aria-label="Remove the last argument"
                title="Remove the last argument"
                onClick={() => replace(at, [language, argv.slice(0, -1)])}
              >
                −
              </button>
            )}
          </div>

          {/*
           * The resolved argv, one boxed token per argument rather than a joined string — see
           * the header. It is the one place the "no shell" rule is *visible* rather than merely
           * documented: a reader can count the arguments, and nobody can mistake the readout for
           * a command line a shell would parse and conclude that `|` or `&&` would work.
           *
           * Boxes and not a separator glyph, because `check-ui-icons.mjs` bans drawing a Unicode
           * symbol as a mark outside an allowlist, and it is right to — a `␣` between tokens is
           * exactly the kind of decorative character that should be a border instead.
           */}
          <p className={styles.argv}>
            {argv
              .filter((token) => token !== '')
              .map((token, index) => (
                <span key={`${index}:${token}`} className={styles.argvToken}>
                  {token}
                </span>
              ))}
          </p>
        </div>
      ))}

      <datalist id="cide-formatter-languages">
        {BUILTIN_IDS.map((id) => (
          <option key={id} value={id} />
        ))}
      </datalist>

      <button
        type="button"
        className={styles.add}
        onClick={() => commit([...entries, ['', ['']]])}
        title={PLACEHOLDER_HINT}
      >
        Add a formatter
      </button>
    </div>
  )
}
