/**
 * Checks `src/panes/imageKinds.ts` — the rule that decides whether a file tab shows a picture
 * or a buffer, and the three strings that describe the picture once it does.
 *
 * # Why this rule is worth a gate
 *
 * It is the *only* input to the fork in `panes/PaneBody.tsx`, it runs before any IPC, and it
 * has no other test: there is no JS test runner in this project, and the component around it
 * needs a window, a Tauri host and a real file to render at all. So a wrong answer here is a
 * `.png` that opens as mojibake, or — worse and quieter — a `.ts` that opens as a broken
 * image because someone added `ts` to the extension map while meaning `tif`.
 *
 * The pairs that follow are all cases the implementation gets wrong if it reaches for the
 * obvious `path.split('.').pop()`:
 *
 * * `~/photos.png/notes.txt` — the extension is on the *basename*, not on the path.
 * * `.png` — a dotfile called `.png`, which is a config file and not an image.
 * * `logo.` — an empty extension, not the previous segment's.
 * * `SCREENSHOT.PNG` — the same file as `screenshot.png`.
 *
 * # And the half that is not here
 *
 * The **cap** and the **format detection** are deliberately Rust-side: `cide_core::image`
 * sniffs the file's own bytes and refuses anything over `MAX_IMAGE_BYTES`, with its own tests
 * (`cargo test -p cide-core image`). The tie between that cap and the editor's is not a test at
 * all but a `const` assertion, `_IMAGE_CAP_FITS_UNDER_THE_EDITORS`, because
 * `cmd::file::openable` gates a terminal ctrl+click on the editor's cap and an image cap above
 * it would be unreachable on that route — an invariant worth failing the build rather than a
 * test run. Mirroring either of them into TypeScript would create exactly the drift this module's
 * header exists to prevent — one table per side, answering one question each.
 *
 * Same shape as `check-exit-marker.mjs`: a pure, import-free module the TypeScript in
 * `node_modules` compiles on its own.
 *
 * Run: `pnpm --dir ui run check:image`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-image-'))
let failed = 0

const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}

const ok = (actual, what) => eq(actual, true, what)

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/panes/imageKinds.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
    ],
    { stdio: 'inherit' },
  )

  const { imageKindFor, imageLabel, formatBytes, imageDetail, undecodableMessage } =
    await import(`file://${join(out, 'imageKinds.js')}`)

  // --- every format the report asked for ------------------------------------------------
  //
  // "opening png, jpeg, gif, webp, bmp, svg and ico shows the image". If any of these stops
  // answering, that file silently goes back to opening as binary refusal text.
  eq(imageKindFor('/p/logo.png'), 'png', 'png')
  eq(imageKindFor('/p/photo.jpeg'), 'jpeg', 'jpeg')
  eq(imageKindFor('/p/photo.jpg'), 'jpeg', 'jpg is the same viewer as jpeg')
  eq(imageKindFor('/p/anim.gif'), 'gif', 'gif')
  eq(imageKindFor('/p/shot.webp'), 'webp', 'webp')
  eq(imageKindFor('/p/old.bmp'), 'bmp', 'bmp')
  eq(imageKindFor('/p/mark.svg'), 'svg', 'svg')
  eq(imageKindFor('/p/favicon.ico'), 'ico', 'ico')

  // --- and everything that must keep opening in the editor -------------------------------
  for (const path of [
    '/p/main.rs',
    '/p/README.md',
    '/p/Makefile',
    '/p/image.ts',
    '/p/notes.pngx',
    '/p/archive.tar.gz',
    '/p/scan.tiff',
    '/p/next.avif',
    '/p/mark.svgz',
  ]) {
    eq(imageKindFor(path), null, `${path} is not routed to the image pane`)
  }

  // --- the four parsing traps ------------------------------------------------------------
  eq(
    imageKindFor('/home/u/photos.png/notes.txt'),
    null,
    'the extension comes off the basename, not off the whole path',
  )
  eq(
    imageKindFor('/home/u/v1.2/logo'),
    null,
    'a dot in a directory name is not the file\'s extension',
  )
  eq(imageKindFor('/home/u/.png'), null, 'a dotfile named .png is a config file, not an image')
  eq(imageKindFor('/home/u/logo.'), null, 'a trailing dot is an empty extension')
  eq(imageKindFor('logo.png'), 'png', 'a bare basename still works')
  eq(imageKindFor('C:\\Users\\u\\logo.PNG'), 'png', 'backslashes separate too')

  // --- case ------------------------------------------------------------------------------
  eq(imageKindFor('/p/SCREENSHOT.PNG'), 'png', 'an upper-case extension is the same file')
  eq(imageKindFor('/p/Photo.JpEg'), 'jpeg', 'mixed case too')

  // --- labels ----------------------------------------------------------------------------
  //
  // `WebP` is the trap: a `toUpperCase()` implementation passes every other case in this list.
  eq(imageLabel('webp'), 'WebP', 'WebP is not spelled WEBP')
  eq(imageLabel('png'), 'PNG', 'PNG')
  eq(imageLabel('jpeg'), 'JPEG', 'JPEG')
  eq(imageLabel('svg'), 'SVG', 'SVG')

  // --- sizes -----------------------------------------------------------------------------
  //
  // Binary units, matching the "MiB" the Rust refusals print. A bar that says KB beside a
  // refusal that says MiB invites the reader to work out which one is lying.
  eq(formatBytes(0), '0 B', 'zero')
  eq(formatBytes(812), '812 B', 'bytes below a kibibyte')
  eq(formatBytes(1024), '1 KiB', 'exactly one kibibyte')
  eq(formatBytes(250 * 1024), '250 KiB', 'kibibytes')
  eq(formatBytes(3.4 * 1024 * 1024), '3.4 MiB', 'mebibytes carry one decimal')
  eq(formatBytes(32 * 1024 * 1024), '32.0 MiB', 'the cap itself is printable')
  eq(formatBytes(-1), '—', 'a nonsense size is not printed as a number')

  // --- the status bar line ---------------------------------------------------------------
  //
  // This is what replaces `Rust · UTF-8 · LF · Ln 7, Col 48` while an image tab is in front.
  eq(
    imageDetail('png', 250 * 1024, { width: 1920, height: 1080 }),
    'PNG · 1920 × 1080 · 250 KiB',
    'format, dimensions and byte size',
  )
  ok(
    imageDetail('png', 1024, { width: 8, height: 8 }).includes('×'),
    'the multiplication sign is U+00D7, not the letter x',
  )
  eq(
    imageDetail('svg', 3 * 1024, null),
    'SVG · 3 KiB',
    'an image whose size is not known yet says nothing about it rather than guessing',
  )
  eq(
    imageDetail('svg', 3 * 1024, { width: 0, height: 0 }),
    'SVG · 3 KiB',
    'a zero natural size is "no intrinsic size", not a 0 × 0 image',
  )

  // --- the decode failure ----------------------------------------------------------------
  //
  // Reachable rather than theoretical: Rust vouches for the first 8 KiB, so a truncated PNG
  // has a perfect signature and no pixels. Without this string the pane is a blank rectangle.
  const truncated = undecodableMessage('half.png', 'png')
  ok(truncated.includes('half.png'), 'the decode failure names the file')
  ok(truncated.includes('truncated'), 'and says the likeliest cause')
  ok(
    !truncated.toLowerCase().includes('disk'),
    'and does not blame the disk for a half-written file',
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`\n${failed} check(s) failed`)
  process.exit(1)
}
console.log('imageKinds.ts: ok')
