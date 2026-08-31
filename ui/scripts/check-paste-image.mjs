/**
 * `ui/src/editor/pasteImage.ts` — pasting a screenshot into a buffer. (M35)
 *
 * The module is import-free so it can be compiled and driven standalone, which is the only way
 * these rules get checked at all: every one of them fails *invisibly* in the app.
 *
 *   * `wantsImagePaste` too eager and a copied file path stops pasting as text — the buffer
 *     grows a screenshot instead, and the text the user copied is gone from the gesture.
 *   * `relativeTo` off by one `../` and the markdown preview shows a broken-image glyph. No
 *     error, nothing logged, and the file really is on disk — so it reads as a preview bug.
 *   * `referenceFor` not wrapping a name with a space in it and CommonMark parses the
 *     destination as `Pasted` with prose after it, which renders as a broken image too.
 *
 * The suite is the same shape as `check-new-entry.mjs`: compile the one module with the
 * TypeScript in `node_modules`, require the output, assert on returned values.
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-paste-image-'))

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/editor/pasteImage.ts',
      '--outDir', out,
      '--rootDir', 'src',
      // `esnext` + `bundler`, exactly as `check-new-entry.mjs` compiles its module, and the
      // `--lib` this first carried is deliberately absent: narrowing the lib drops `dom`, and
      // the ambient `@types/react-dom` in `node_modules` then fails to resolve `ReferrerPolicy`
      // before this module is even looked at. The module itself is DOM-free.
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      '--noUncheckedIndexedAccess',
      '--exactOptionalPropertyTypes',
    ],
    { stdio: 'inherit' },
  )

  const {
    wantsImagePaste,
    imageDirFor,
    onImagePaste,
    referenceFor,
    relativeTo,
  } = await import(`file://${join(out, 'editor', 'pasteImage.js')}`)

  let failed = 0
  const eq = (got, want, why) => {
    const ok = JSON.stringify(got) === JSON.stringify(want)
    if (!ok) {
      failed += 1
      console.error(`FAIL ${why}\n  got  ${JSON.stringify(got)}\n  want ${JSON.stringify(want)}`)
    }
  }

  // ---- what counts as an image paste ------------------------------------------------------

  eq(wantsImagePaste(['image/png']), true,
    'a screenshot tool offers the image alone — Spectacle, Flameshot, GNOME Screenshot, ⌘⇧4')
  eq(wantsImagePaste(['image/jpeg']), true,
    'the prefix, not an exact type: cide writes a PNG out of the decoded bitmap either way')
  eq(wantsImagePaste([]), false, 'an empty clipboard is not an image')
  eq(wantsImagePaste(['text/plain']), false, 'and neither is text')
  eq(wantsImagePaste(['image/png', 'text/plain']), false,
    'a clipboard holding both is holding text — the same trade terminal/clipboard.ts makes, '
      + 'and it is what keeps a copied file path pasting as a path')
  eq(wantsImagePaste(['text/uri-list', 'image/png']), false,
    'a file copied in a file manager offers a uri-list and a thumbnail; the paste means the file')
  eq(wantsImagePaste(['Files']), false,
    'copying a .png file is the file tree\'s paste — copy the file, do not re-encode its pixels')

  // ---- where the file goes ----------------------------------------------------------------

  eq(imageDirFor('/home/u/p/docs/notes.md'), '/home/u/p/docs',
    'beside the document, which is what "in place where we pasting it" means')
  eq(imageDirFor('/notes.md'), '/', 'a file at the root has a directory, and it is not ""')
  eq(imageDirFor('scratch.md'), null,
    'a buffer that is not a file on disk has nowhere to write — the caller must refuse rather '
      + 'than drop a screenshot into the process cwd')

  // ---- the relative reference -------------------------------------------------------------

  eq(relativeTo('/home/u/p/docs/notes.md', '/home/u/p/docs/shot.png'), 'shot.png',
    'the ordinary case: the image is written into the document\'s own directory')
  eq(relativeTo('/home/u/p/docs/notes.md', '/home/u/p/img/shot.png'), '../img/shot.png',
    'one level up and back down')
  eq(relativeTo('/home/u/p/a/b/c/notes.md', '/home/u/p/shot.png'), '../../../shot.png',
    'one ".." per level, and the count is the whole thing that can be wrong here')
  eq(relativeTo('/home/u/p/notes.md', '/home/u/p/sub/deep/shot.png'), 'sub/deep/shot.png',
    'no "./" prefix — markdown, HTML and CSS all resolve a bare name against the document')
  eq(relativeTo('/a/notes.md', '/b/shot.png'), '../b/shot.png',
    'siblings under the root still relate through it')
  eq(relativeTo('notes.md', '/home/u/shot.png'), '/home/u/shot.png',
    'a non-absolute side cannot be related; the absolute path is the honest answer')

  // ---- what is typed ----------------------------------------------------------------------

  eq(referenceFor('markdown', 'shot.png'), '![](shot.png)',
    'a markdown buffer means a picture, and the preview renders it immediately')
  eq(referenceFor('mdx', 'shot.png'), '![](shot.png)',
    'mdx is markdown with components in it; ![]() means the same thing')
  eq(referenceFor('rust', 'shot.png'), 'shot.png',
    'everywhere else the bare path is what a comment, a string literal or an src wants')
  eq(referenceFor(null, 'shot.png'), 'shot.png', 'and a buffer with no language is everywhere else')
  eq(
    referenceFor('markdown', 'Pasted image 2026-08-31 at 22.41.07.png'),
    '![](<Pasted image 2026-08-31 at 22.41.07.png>)',
    'a space ends the destination in CommonMark, so the generated name — which always has '
      + 'spaces — must be wrapped, or every pasted image renders broken with prose after it',
  )
  eq(referenceFor('markdown', 'a <b>.png'), '![](a%20<b>.png)',
    'a destination already holding an angle bracket cannot be wrapped — the delimiter would '
      + 'close early — so its spaces are percent-encoded instead')
  eq(referenceFor('rust', 'Pasted image 2026-08-31 at 22.41.07.png'),
    'Pasted image 2026-08-31 at 22.41.07.png',
    'no escaping outside markdown — there is no syntax to escape for')

  // ---- the decision, end to end -----------------------------------------------------------

  const host = (over = {}) => {
    const calls = { dirs: [], inserted: [] }
    const h = {
      docPath: '/home/u/p/docs/notes.md',
      languageId: 'markdown',
      readOnly: false,
      write: async (dir) => {
        calls.dirs.push(dir)
        return '/home/u/p/docs/Pasted image 2026-08-31 at 22.41.07.png'
      },
      insert: (text) => calls.inserted.push(text),
      ...over,
    }
    return [h, calls]
  }

  {
    const [h, calls] = host()
    eq(onImagePaste(h, ['image/png']), 'claimed', 'an image alone is this gesture')
    await Promise.resolve()
    await Promise.resolve()
    eq(calls.dirs, ['/home/u/p/docs'], 'the file goes beside the document')
    eq(calls.inserted, ['![](<Pasted image 2026-08-31 at 22.41.07.png>)'],
      'and the reference is typed at the cursor, wrapped because the name has spaces')
  }
  {
    const [h, calls] = host()
    eq(onImagePaste(h, ['text/plain']), 'no-image', 'text falls through to the webview')
    eq(calls.dirs, [], 'and nothing is written')
  }
  {
    // A revision or a diff. It has a path, and pasting into it would put a file beside the
    // *working copy* on behalf of a buffer that cannot be edited.
    const [h, calls] = host({ readOnly: true })
    eq(onImagePaste(h, ['image/png']), 'read-only', 'a read-only buffer refuses')
    eq(calls.dirs, [], 'before writing anything')
  }
  {
    const [h, calls] = host({ docPath: null })
    eq(onImagePaste(h, ['image/png']), 'no-path',
      'a buffer that is not a file on disk has nowhere to put it — never the process cwd')
    eq(calls.dirs, [], 'and nothing is written')
  }
  {
    // The clipboard can change between the event and the write, so `write` is allowed to
    // answer "there was no image after all" and the buffer must be left alone.
    const [h, calls] = host({ write: async () => null })
    eq(onImagePaste(h, ['image/png']), 'claimed', 'the event is still claimed synchronously')
    await Promise.resolve()
    await Promise.resolve()
    eq(calls.inserted, [], 'but nothing is typed when nothing was written')
  }
  {
    const [h, calls] = host({ languageId: 'rust', docPath: '/home/u/p/src/main.rs' })
    eq(onImagePaste(h, ['image/png']), 'claimed', 'the gesture is not markdown-only')
    await Promise.resolve()
    await Promise.resolve()
    eq(calls.inserted, ['../docs/Pasted image 2026-08-31 at 22.41.07.png'],
      'a non-markdown buffer gets the bare relative path, and it is relative to *its* directory')
  }

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('pasting an image into a buffer: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
