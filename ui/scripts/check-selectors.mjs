/**
 * Refuses a zustand selector that builds a fresh value on every read.
 *
 * # The bug this is made of
 *
 * `useStore(selector)` runs the selector on **every** store read and hands the result to
 * `useSyncExternalStore`, which compares snapshots with `Object.is`. A selector that constructs
 * something — `Object.keys(...)`, `.map(...)`, an array or object literal — therefore returns a
 * value that is never `Object.is`-equal to the last one. React concludes the store changed,
 * re-renders, reads again, gets another new value, and the loop ends only when React gives up
 * with *Maximum update depth exceeded* — which unmounts the entire root, not just the component.
 *
 * `ToolWindowSplitter` had `useWorkspace((s) => Object.keys(s.boot?.workspace.windows ?? {}))`.
 * It renders exactly when the git tool window is open, and `open` is persisted on `Project`, so
 * the window came up empty on every launch with no way back. Every check in the suite passed
 * while that was true, and this is the reason: the render checks SSR the **pure view** of each
 * surface, and server rendering does one pass with no updates in it, so a re-render loop is
 * invisible to the entire suite by construction. A source rule is what is left.
 *
 * # Why a source scan rather than a runtime test
 *
 * Reproducing it needs a real reconciler, a real store, and the willingness to hang the process
 * for a few thousand renders before concluding anything. The shape, by contrast, is decidable by
 * reading — and the fix is always one of two things, both cheap: select the stored value and
 * derive it in a `useMemo`, or wrap the selector in `useShallow`.
 */
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, relative, resolve } from 'node:path'

const UI = resolve(import.meta.dirname, '..')
const SRC = join(UI, 'src')

/** Every hook that takes a zustand selector. Named, because `useMemo(() => ...)` must not match. */
const STORE_HOOKS = /\buse(?:[A-Z]\w*)?(?:Store|Workspace|GitStatus|GitCount|Sidebar|Tabs|Panel)?\w*\(/

/**
 * What "builds something new" looks like. Each is a construction whose result is a fresh
 * reference every time the expression is evaluated.
 */
const FRESH = [
  { re: /\bObject\.(keys|values|entries|assign|fromEntries)\s*\(/, why: 'Object.$1() is a new array or object every call' },
  { re: /\.(map|filter|flatMap|slice|concat|sort|reverse|flat)\s*\(/, why: 'an array method returns a new array every call' },
  { re: /\[\s*\.\.\./, why: 'a spread into an array literal is a new array every call' },
  { re: /\{\s*\.\.\./, why: 'a spread into an object literal is a new object every call' },
  { re: /\bnew (Map|Set|Array)\s*\(/, why: 'a new $1 every call' },
]

/**
 * Blank out comments, keeping every byte's position so reported line numbers stay true.
 *
 * Not cosmetic: this file's own header quotes the offending line, and half the modules that
 * *fixed* this bug now carry a paragraph explaining it. Scanning raw text makes every one of
 * those explanations a fresh failure, which would teach the next reader to delete the
 * explanation rather than to keep the rule.
 */
function stripComments(text) {
  let out = ''
  let i = 0
  while (i < text.length) {
    const two = text.slice(i, i + 2)
    if (two === '/*') {
      const end = text.indexOf('*/', i + 2)
      const stop = end === -1 ? text.length : end + 2
      // Newlines survive so line counting is unaffected; everything else becomes a space.
      out += text.slice(i, stop).replace(/[^\n]/g, ' ')
      i = stop
    } else if (two === '//') {
      const end = text.indexOf('\n', i)
      const stop = end === -1 ? text.length : end
      out += ' '.repeat(stop - i)
      i = stop
    } else {
      out += text[i]
      i += 1
    }
  }
  return out
}

/** Walk `.ts`/`.tsx` under `src`, skipping nothing — a dead file's landmine still gets copied. */
function* files(dir) {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name)
    if (statSync(path).isDirectory()) yield* files(path)
    else if (/\.tsx?$/.test(name)) yield path
  }
}

/**
 * The selector body of a `useSomething((s) => …)` call, or `null`.
 *
 * Balances parentheses from the arrow, so a selector spanning twenty lines with calls inside it
 * is read whole. Anything less would pass a multi-line selector that a one-line one fails.
 */
function selectorBody(text, arrowAt) {
  let depth = 0
  for (let i = arrowAt; i < text.length; i++) {
    const c = text[i]
    if (c === '(') depth++
    else if (c === ')') {
      depth--
      if (depth < 0) return text.slice(arrowAt, i)
    }
  }
  return null
}

/**
 * What the selector actually hands back — which is the only part that matters.
 *
 * The first version of this rule tested the whole body and was wrong in a way worth keeping a
 * note about: `editor/mentionTarget.ts` calls `Object.values(...).find(...)` inside its selector
 * and returns **a string**. A primitive is `Object.is`-equal to itself for ever, so that
 * selector is not merely safe, it is the recommended fix — its own comment says so, having
 * packed two ids into one string precisely to avoid this bug. Flagging it would have taught the
 * next reader to undo a correct workaround.
 *
 * So: a concise arrow's body *is* its return expression, and a block arrow's are its `return`
 * statements. The known imprecision is a nested block arrow inside the selector, whose `return`
 * is read as the selector's own — that direction is a false positive on a shape nothing here
 * uses, and a false positive costs a comment while a false negative costs the window.
 */
function returnExpressions(body) {
  const trimmed = body.trim()
  if (!trimmed.startsWith('{')) return [trimmed]
  const out = []
  const re = /\breturn\b/g
  let m
  while ((m = re.exec(trimmed)) !== null) {
    // To the end of the statement: a `;`, or a newline that closes a balanced expression.
    let depth = 0
    let i = m.index + 'return'.length
    let expr = ''
    for (; i < trimmed.length; i++) {
      const c = trimmed[i]
      if (c === '(' || c === '[' || c === '{') depth++
      else if (c === ')' || c === ']' || c === '}') {
        if (depth === 0) break
        depth--
      } else if (c === ';' && depth === 0) break
      else if (c === '\n' && depth === 0 && expr.trim().length > 0) break
      expr += c
    }
    out.push(expr.trim())
  }
  return out
}

let failed = 0
let scanned = 0
let selectors = 0
const offenders = []

for (const path of files(SRC)) {
  const text = stripComments(readFileSync(path, 'utf8'))
  scanned += 1
  // `use…(` immediately followed by an arrow whose parameter is a single identifier: that is the
  // selector shape. `useEffect(() => …)` has an empty parameter list and does not match.
  const call = /\buse[A-Z]\w*\(\s*(?:useShallow\(\s*)?\(\s*([a-zA-Z_$][\w$]*)\s*\)\s*=>/g
  let m
  while ((m = call.exec(text)) !== null) {
    const head = text.slice(m.index, m.index + m[0].length)
    if (!STORE_HOOKS.test(head)) continue
    // `useShallow` is the sanctioned escape: it compares one level deep, so a fresh array of
    // stable members settles instead of looping.
    if (/useShallow\(/.test(head)) continue
    // `useMemo`/`useCallback` take a dependency array, never a selector — and their arrow has no
    // parameter, so they cannot reach here. Guarded anyway, since the name test is a heuristic.
    if (/\buse(Memo|Callback|Effect|LayoutEffect|State|Reducer|Ref)\(/.test(head)) continue
    selectors += 1
    const body = selectorBody(text, m.index + m[0].length)
    if (body === null) continue
    let flagged = false
    for (const expr of returnExpressions(body)) {
      if (flagged) break
      for (const { re, why } of FRESH) {
        const hit = re.exec(expr)
        if (hit === null) continue
        const line = text.slice(0, m.index).split('\n').length
        offenders.push({
          file: relative(UI, path),
          line,
          why: why.replace('$1', hit[1] ?? ''),
          snippet: expr.replace(/\s+/g, ' ').trim().slice(0, 100),
        })
        flagged = true
        break
      }
    }
  }
}

if (offenders.length > 0) {
  for (const o of offenders) {
    console.error(
      `FAIL ${o.file}:${o.line} — a store selector that builds a new value on every read.\n`
        + `  ${o.why}.\n`
        + `  ${o.snippet}\n`
        + '  React compares snapshots with Object.is, so this re-renders for ever and ends at\n'
        + '  "Maximum update depth exceeded", which unmounts the whole window — not just this\n'
        + '  component. Select the stored value and derive it in a useMemo, or wrap the\n'
        + '  selector in useShallow from zustand/react/shallow.',
    )
    failed += 1
  }
  console.error(`\n${failed} failure(s)`)
  process.exit(1)
}

console.log(`store selectors: ok (${selectors} selectors in ${scanned} files, none build a fresh value)`)
