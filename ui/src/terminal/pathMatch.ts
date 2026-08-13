/**
 * Finding file paths in terminal output, and deciding which file on disk one names.
 *
 * Two pure functions and no imports, for the reason the rest of this project has learned the
 * hard way: a rule that lives inside a React hook or a DOM callback is in the one place no
 * `check:*` script can compile. `ui/scripts/check-paths.mjs` builds this file on its own with
 * a bare `tsc` and drives every sample below under node. The impure half — the xterm link
 * provider, the existence probe, the ctrl+click gate — is `pathLinks.ts`, which owns no rules
 * at all.
 *
 * # The thing this is optimised against
 *
 * **A matcher that lights up every token containing a dot is worse than no feature.** A build
 * log is mostly prose, version numbers and punctuation; a terminal where half of every
 * sentence is underlined teaches the user to stop looking at underlines, and then the real
 * links are gone too. So the grammar below is deliberately narrow, three separate proofs are
 * required before anything is offered, and the *whole* list of things it refuses to match is
 * written down under "Deliberately not matched" rather than left to be discovered.
 *
 * # The three stages, and why they are separate
 *
 * 1. [`matchPaths`] finds *candidates* in one logical line. It knows nothing about disks.
 * 2. [`candidatePaths`] turns one candidate into the absolute paths it could possibly name,
 *    given the cwds and roots in play, and drops anything outside the project.
 * 3. [`resolveCandidate`] asks an existence oracle which of those are real, and answers with
 *    **one**, **many** or **none** — never with a guess. `many` is not an inconvenience to be
 *    smoothed over; it is the answer, and the caller must refuse rather than pick.
 *
 * Splitting 2 from 3 is what lets the caller batch: every candidate on a hovered line
 * contributes its possible paths, one round trip answers all of them at once, and stage 3
 * then runs against a map.
 *
 * # What the output of this repo's own toolchain actually looks like
 *
 * Captured from real runs rather than recalled, because the shapes differ in ways that decide
 * the grammar (each of these is a case in `check-paths.mjs`):
 *
 * ```text
 * rustc/cargo    -->  crates/cide-app/src/lib.rs:270:13
 * rust panic     thread 'main' panicked at src/main.rs:1:13:      <- trailing colon
 * cargo status   Compiling cide-app v0.1.0 (/home/…/cide)         <- a DIRECTORY in parens
 * tsc (pretty)   src/bad.ts:1:7 - error TS2322: …                 <- relative to ui/, not the root
 * tsc (plain)    src/bad.ts(1,7): error TS2322: …                 <- parenthesised line,col
 * esbuild/vite   src/foo.tsx:12:3:                                <- trailing colon again
 * node stack     at f (/home/…/ui/scripts/check-theme.mjs:42:11)
 * git status     ` M ui/package.json`
 * git diff       `--- a/ui/package.json`, `+++ b/ui/package.json` <- a/ and b/ are not real
 * ripgrep        ui/src/App.tsx:830:                void fileApi… <- "col" is content
 * Claude Code    ⏺ Read(ui/src/App.tsx),  @ui/src/App.tsx,  path#L12-20
 * bash           cat: 'ui/src/foo.ts': No such file or directory
 * ```
 *
 * The toolchain runs at **two different cwds** — cargo at the workspace root, `pnpm run
 * typecheck` at `ui/` — which is why [`candidatePaths`] takes a list of bases and not a cwd.
 *
 * # Deliberately not matched, and why each one lost
 *
 * * **Unquoted paths containing a space.** In `error at foo bar.rs:3` nothing in the text says
 *   where the path begins. Guessing lights up half a sentence. A path inside `'…'` or `"…"` is
 *   the one place a space is allowed, because there the producer said where it ended.
 * * **Bare words with a dot and no slash** — `0.12.0`, `v0.1.0`, `e.g.`, `README`,
 *   `target(s)`. Resolving a lone basename against the project index is possible and is
 *   deliberately not done in v1: it is the rule that turns every English sentence into
 *   candidate links, and it is the one whose false positives are unbounded.
 * * **Paths a TUI has truncated** — `ui/src/sidebar/GitPa…`. The ellipsis is the producer
 *   saying the text is not the path. Claude Code's own TUI does this constantly in a narrow
 *   pane.
 * * **Paths a TUI has wrapped by itself.** The terminal's *own* wrapping is handled (see
 *   `pathLinks.ts`, which rebuilds the logical line through `isWrapped`), but Ink draws its
 *   own line breaks without setting that flag, and a path split that way is unrecoverable
 *   from the buffer. Better unmatched than matched as two wrong halves.
 * * **`~/…`.** Trivial to expand and excluded on purpose: it is the one form that reaches out
 *   of every project root, and `~/.claude/.credentials.json` is a small UTF-8 text file. One
 *   fewer way to leave the project.
 * * **URLs.** Anything inside a `scheme://…` run is skipped whole, so `http://localhost:5173/
 *   src/main.tsx` contributes nothing. Opening web links from terminal output is a separate
 *   decision with a separate threat model; see `xterm.ts`'s `linkHandler`.
 * * **Windows paths, UNC, backslash separators.** This is a Linux/WebKitGTK app and a
 *   backslash in this output is an escape, not a separator.
 * * **Bare `/etc`, `/tmp`, `/`.** One non-empty segment is not a path, it is a word with a
 *   slash on it.
 *
 * Two things it *does* match that are not files — `and/or` in prose, `crates/cide-app/` — and
 * that is intended. This stage is a candidate generator; the index is the oracle. A candidate
 * that names nothing is dropped in stage 3 without ever being drawn.
 */

/** How much of one logical line is scanned. Past this the line is not searched at all. */
export const MAX_LINE = 4096

/**
 * One thing in a line that might be a path.
 *
 * `start`/`end` are 0-based indices into the *logical* line the caller passed in, and they
 * span the whole clickable run — the path **and** its `:12:3` — because the position is part
 * of what the user is pointing at. `text` is the path alone, with `@`, `a/`, quotes and
 * trailing punctuation already removed, which is what stages 2 and 3 work on.
 */
export interface Candidate {
  readonly text: string
  readonly start: number
  readonly end: number
  /** 1-based, or `null` when the producer named no line. */
  readonly line: number | null
  /** 1-based, or `null`. Never set without `line`. */
  readonly column: number | null
}

/**
 * The characters a path may be spelled with here.
 *
 * `/` and no backslash. `@` is in the set so `node_modules/@xterm/xterm/package.json` survives
 * as one run; a *leading* `@` is stripped afterwards because that is Claude Code's mention
 * marker. `:` is deliberately absent — it is what ends the body and begins the position, and
 * having it in both roles is how `foo.ts:12` becomes a filename nobody has.
 */
const BODY = /[A-Za-z0-9._+%~@/-]/

/** A run of body characters, as long as possible. Maximal, so runs never overlap. */
const RUN = /[A-Za-z0-9._+%~@/-]+/g

/**
 * A URL, so its insides can be skipped whole.
 *
 * Matched by shape rather than by a scheme allowlist: the point is not to recognise `https`,
 * it is to make sure `…//host/path/file.ts` never contributes a candidate whose text came out
 * of an authority component.
 */
const URL_RUN = /[A-Za-z][A-Za-z0-9+.-]*:\/\/[^\s'"`<>]*/g

/**
 * A quoted run — the only place a space may appear inside a path.
 *
 * The opening quote must not follow a word character, which is what keeps the apostrophe in
 * `don't open src/foo.rs' contents` from opening a span that swallows the real path in the
 * middle of it. That is not hypothetical prose: it is the shape of every possessive in every
 * English error message a build prints.
 */
const QUOTED = /(^|[^A-Za-z0-9_])(['"])([^'"\n]{1,512})\2/g

/** `:12`, `:12:5`, and the trailing colon rustc, esbuild and ripgrep all add. */
const COLON_POS = /^:(\d{1,9})(?::(\d{1,9}))?:?/

/** `(12,5)` — tsc with `--pretty false`, which is what a CI log holds. */
const PAREN_POS = /^\((\d{1,9}),(\d{1,9})\)/

/** `#L12` / `#L12-20` — the form Claude Code writes its own mentions in. */
const HASH_POS = /^#L(\d{1,9})(?:-\d{1,9})?/

/**
 * `git diff`'s pre-image/post-image prefixes, which name no directory anybody has.
 *
 * The trailing `\s` is not decoration and `\b` cannot replace it: `-` is not a word character,
 * so `/^---\b/` never matches `--- a/x` at all — the boundary it is looking for is between two
 * non-word characters and does not exist. That spelling shipped for exactly as long as it took
 * this file's own check script to run.
 */
const DIFF_LINE = /^(?:---\s|\+\+\+\s|diff --git\s)/

/**
 * Every candidate in one logical line, in the order they appear.
 *
 * The line is the *logical* one: `pathLinks.ts` rebuilds it across the terminal's own wrapping
 * before calling here, so a path broken by the right edge of the pane is still one string by
 * the time it arrives.
 */
export function matchPaths(logicalLine: string): Candidate[] {
  const line = logicalLine.length > MAX_LINE ? logicalLine.slice(0, MAX_LINE) : logicalLine
  const blocked = urlSpans(line)
  const out: Candidate[] = []

  // Quoted runs first, and they *claim* their span: a quoted path with a space in it is one
  // candidate, and letting the ordinary scan also produce `my` and `file.txt` from inside it
  // would draw two links over one filename.
  for (const span of quotedSpans(line)) {
    if (overlaps(blocked, span.start, span.end)) continue
    const candidate = build(line, span.text, span.start, span.end, span.end)
    if (candidate !== null) {
      out.push(candidate)
      blocked.push({ start: span.start, end: span.end })
    }
  }

  RUN.lastIndex = 0
  let match: RegExpExecArray | null
  while ((match = RUN.exec(line)) !== null) {
    const raw = match[0]
    const start = match.index
    const end = start + raw.length
    if (overlaps(blocked, start, end)) continue

    // Truncation, stated by the producer. `…` is not a body character, so it cannot be inside
    // the run — it is always the character that ended it, and `...` is the ASCII spelling that
    // the trailing-dot strip below would otherwise quietly turn into a plausible filename.
    if (line[end] === '…' || raw.endsWith('...')) continue

    const candidate = build(line, raw, start, end, end)
    if (candidate !== null) out.push(candidate)
  }

  out.sort((a, b) => a.start - b.start)
  return out
}

/**
 * Turn one raw run into a candidate, or refuse it.
 *
 * `posAt` is where the position suffix would begin — the end of the *raw* run, before any
 * trailing punctuation was taken off, because `src/main.rs:2:18` and `src/main.rs.` end in
 * different places and only one of them has a position.
 */
function build(
  line: string,
  raw: string,
  rawStart: number,
  rawEnd: number,
  posAt: number,
): Candidate | null {
  let text = raw
  let start = rawStart
  let proven = false

  // Sentence punctuation the producer added, not part of the name: "see src/main.rs." The
  // comma, semicolon and the rest are not body characters and so were never in the run.
  while (text.endsWith('.')) text = text.slice(0, -1)

  // Claude Code's mention marker. One, not a run: `@@foo` is not a mention of `@foo`.
  if (text.startsWith('@')) {
    text = text.slice(1)
    start += 1
  }

  // `a/` and `b/` are git's names for "the pre-image" and "the post-image", not directories.
  // Only on a line that is actually a diff header — `a/b.txt` in ordinary output is a path.
  //
  // Stripping it can leave one segment (`--- a/run.sh`), and that is admitted: the line shape
  // is git saying "what follows this prefix is a path in this repository", which is a stronger
  // proof than the slash rule below is looking for. Without this exception every top-level
  // file in every diff would be unlinkable, which is most of what a `git diff` at a repo root
  // prints.
  if (DIFF_LINE.test(line) && (text.startsWith('a/') || text.startsWith('b/'))) {
    text = text.slice(2)
    start += 2
    proven = true
  }

  if (text === '' || !plausible(text, proven)) return null

  const position = readPosition(line, posAt)
  return {
    text,
    start,
    end: rawEnd + position.length,
    line: position.line,
    column: position.column,
  }
}

/**
 * The proof of pathhood: at least two non-empty `/`-separated segments.
 *
 * This one rule is what keeps prose out. `0.12.0`, `v0.1.0`, `README`, `e.g.` and every other
 * dotted word fail it; `src/main.rs`, `./run.sh`, `../vendor/z.c` and `/home/u/p/f.rs` pass
 * it. `/etc` fails deliberately — one segment behind a slash is a word, not a path — and so
 * does a bare `src/`, whose trailing empty segment leaves it with one.
 *
 * `proven` is the one way past it, and only the diff-header prefix sets it: see [`build`].
 *
 * A backslash anywhere is refused rather than treated as a separator: in this output it is an
 * escape, and a run containing one is a quoted string being reported, not a Windows path.
 */
function plausible(text: string, proven: boolean): boolean {
  if (text.includes('\\')) return false
  if (text.includes('://')) return false
  /*
   * `~/…` is refused outright rather than left to resolve as a literal directory named `~`.
   *
   * Both readings end in "no link", so this line changes no behaviour — it exists so that the
   * refusal is a *rule* somebody can find and a check script can pin, rather than an accident
   * of the resolver. It is the one form that reaches straight out of every project root, and
   * `~/.claude/.credentials.json` is a small, valid-UTF-8 text file that would open perfectly.
   */
  if (text === '~' || text.startsWith('~/')) return false
  let segments = 0
  for (const segment of text.split('/')) {
    if (segment !== '') segments += 1
  }
  return segments >= (proven ? 1 : 2)
}

/** The `:12:5` / `(12,5)` / `#L12` after a run, or nothing. */
function readPosition(
  line: string,
  at: number,
): { line: number | null; column: number | null; length: number } {
  const rest = line.slice(at, at + 24)

  const colon = COLON_POS.exec(rest)
  if (colon !== null) {
    return {
      line: number(colon[1]),
      // ripgrep prints `path:830:` and then the *content* of the line, so a second number is
      // often not a column at all. Taking it anyway is the right trade: the file and the line
      // are what the user pointed at, and a column that lands one word early costs nothing,
      // while refusing every two-number form would drop rustc, tsc and esbuild.
      column: number(colon[2]),
      length: colon[0].length,
    }
  }

  const paren = PAREN_POS.exec(rest)
  if (paren !== null) {
    return { line: number(paren[1]), column: number(paren[2]), length: paren[0].length }
  }

  const hash = HASH_POS.exec(rest)
  if (hash !== null) {
    // The `-20` of `#L12-20` is inside the matched length, so the whole mention underlines,
    // but only the start is a caret. A range selection would need an end column this form
    // does not carry.
    return { line: number(hash[1]), column: null, length: hash[0].length }
  }

  return { line: null, column: null, length: 0 }
}

/** A captured group as a positive integer, or `null`. Zero is not a line number. */
function number(text: string | undefined): number | null {
  if (text === undefined) return null
  const value = Number.parseInt(text, 10)
  return Number.isFinite(value) && value > 0 ? value : null
}

interface Span {
  readonly start: number
  readonly end: number
}

function urlSpans(line: string): Span[] {
  const spans: Span[] = []
  URL_RUN.lastIndex = 0
  let match: RegExpExecArray | null
  while ((match = URL_RUN.exec(line)) !== null) {
    spans.push({ start: match.index, end: match.index + match[0].length })
  }
  return spans
}

/**
 * Quoted runs that are worth treating as one path — which means the ones with a space in
 * them.
 *
 * A quoted run *without* a space is left to the ordinary scan, which handles it correctly
 * already and also reads the position suffix. So quoting buys exactly one thing here: spaces.
 */
function quotedSpans(line: string): Array<{ text: string; start: number; end: number }> {
  const out: Array<{ text: string; start: number; end: number }> = []
  QUOTED.lastIndex = 0
  let match: RegExpExecArray | null
  while ((match = QUOTED.exec(line)) !== null) {
    const lead = match[1] ?? ''
    const body = match[3] ?? ''
    if (!body.includes(' ')) continue
    if (body.startsWith(' ') || body.endsWith(' ')) continue
    // Every character must be one a path may be spelled with, or a space. This is what stops
    // a quoted *sentence* — which build tools print constantly — from being taken as a
    // filename with several spaces in it.
    if (![...body].every((c) => c === ' ' || BODY.test(c))) continue
    const start = match.index + lead.length + 1
    out.push({ text: body, start, end: start + body.length })
  }
  return out
}

function overlaps(spans: readonly Span[], start: number, end: number): boolean {
  return spans.some((s) => start < s.end && s.start < end)
}

// --- stage 2: which files could this name? ----------------------------------------------

export interface Bases {
  /**
   * Working directories to try first, most trustworthy first.
   *
   * In practice: the pane's child's cwd read from `/proc`, then the cwd the session was
   * spawned with. Both, and in that order, because a shell pane after `cd ui` prints paths
   * relative to `ui` while the spawn cwd is still the project root — and because the `/proc`
   * answer is the cwd *now*, which is not necessarily the cwd the line was printed from.
   */
  readonly cwds: readonly string[]
  /** The project's roots, in `Project.roots` order. */
  readonly roots: readonly string[]
}

/**
 * Every absolute path this candidate could name, most likely first, deduplicated.
 *
 * **Nothing outside the project's roots survives this function.** That is the frontend's half
 * of containment: an absolute `/etc/shadow` printed by a build script produces an empty list
 * and is never even asked about, and a relative path that climbs out with `../..` is dropped
 * after normalisation rather than before, so the climb cannot be hidden by a middle segment.
 * It is *not* the security boundary — Rust re-checks on the click, because a frontend answer
 * is not evidence — it is the first of the two locks, and the one that keeps the hover probe
 * from ever naming a file outside the project.
 *
 * A bare basename yields nothing: [`matchPaths`] cannot produce one, and if a caller invents
 * one anyway, resolving it against every root is the ambiguity this whole design refuses.
 */
export function candidatePaths(text: string, bases: Bases): string[] {
  const roots = bases.roots.map(normalize).filter((r) => r !== '')
  const out: string[] = []

  const add = (path: string): void => {
    const normalized = normalize(path)
    if (normalized === '') return
    if (!roots.some((root) => within(root, normalized))) return
    if (!out.includes(normalized)) out.push(normalized)
  }

  if (text.startsWith('/')) {
    add(text)
    return out
  }

  for (const base of [...bases.cwds, ...bases.roots]) {
    if (base === '' || !base.startsWith('/')) continue
    add(`${base}/${text}`)
  }
  return out
}

/**
 * Collapse `.`, `..` and repeated separators, textually.
 *
 * Textual on purpose, and the same choice `cide_fs::ops::check_within` makes for the same
 * reason: resolving symlinks here would need a disk, and this module has none. The
 * consequence — a symlink inside the project pointing out of it — is answered where it can be
 * answered, in Rust, on the click.
 *
 * Returns `''` for anything that is not absolute, so a caller cannot accidentally produce a
 * relative "absolute" path.
 */
function normalize(path: string): string {
  if (!path.startsWith('/')) return ''
  const parts: string[] = []
  for (const segment of path.split('/')) {
    if (segment === '' || segment === '.') continue
    if (segment === '..') {
      // Climbing past the filesystem root is not an error to report, it is a path that names
      // nothing. `/..` is `/`.
      parts.pop()
      continue
    }
    parts.push(segment)
  }
  return `/${parts.join('/')}`
}

/** Containment between two already-normalised absolute paths. */
function within(root: string, path: string): boolean {
  return path === root || path.startsWith(root === '/' ? '/' : `${root}/`)
}

// --- stage 3: which one is it, if any? --------------------------------------------------

export type Resolution =
  | { readonly kind: 'one'; readonly path: string }
  | { readonly kind: 'many'; readonly paths: readonly string[] }
  | { readonly kind: 'none' }

const NONE: Resolution = { kind: 'none' }

export interface ResolveContext extends Bases {
  /**
   * Whether an absolute path is a regular file the project holds.
   *
   * Supplied by the caller from the project index — see `fs_paths_exist` — so this module
   * needs no disk and the check script can drive it from a `Set`. Directories must answer
   * `false`: `Compiling cide-app v0.1.0 (/home/…/cide)` is a frequent, real line, and a link
   * that opens a directory in a text editor is a link that does nothing.
   */
  readonly isFile: (path: string) => boolean
}

/**
 * Which file this candidate names: exactly one, several, or none.
 *
 * **`many` is an answer, not a failure to answer.** The caller must not take the first: a link
 * that opens the wrong file is worse than no link, and the two ways to get here are both real.
 * A multi-root project can hold `src/index.ts` under two roots; a pane whose child has `cd`-ed
 * can make a path resolve under both the new cwd and a root. Neither has a winner that is
 * right often enough to guess at.
 *
 * The tempting alternative — "the cwd wins, it is where the program was running" — lost
 * because the cwd is read at *hover* time and the line was printed at some earlier time. A
 * user who builds in `ui/`, `cd ..`s, and then ctrl+clicks a line from the old build would be
 * sent confidently to the wrong file, which is precisely the failure mode ambiguity exists to
 * prevent.
 */
export function resolveCandidate(text: string, ctx: ResolveContext): Resolution {
  const hits = candidatePaths(text, ctx).filter(ctx.isFile)
  if (hits.length === 0) return NONE
  const only = hits[0]
  if (hits.length === 1 && only !== undefined) return { kind: 'one', path: only }
  return { kind: 'many', paths: hits }
}

/*
 * Runtime values only, no imports: `check-paths.mjs` compiles this file alone with a bare
 * `tsc` and loads the emitted `.js` in node. Adding any import — a type from `@/ipc/client`,
 * a constant from a sibling — breaks that, and the check would then fail with a
 * module-resolution error rather than telling anyone what actually broke. The same note is at
 * the foot of `chrome/notices.ts` and `chrome/branchModel.ts`.
 */
