// Render the actual icon component: a passing filename lookup cannot prove the link badge draws.
import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { build } from 'vite'

const ui = resolve(import.meta.dirname, '..')
const cache = join(ui, 'node_modules/.cache')
mkdirSync(cache, { recursive: true })
const out = mkdtempSync(join(cache, 'cide-symlinks-'))
try {
  const entry = join(out, 'entry.tsx')
  writeFileSync(entry, `
    import { createElement } from 'react'
    import { renderToStaticMarkup } from 'react-dom/server'
    import { FileIcon } from ${JSON.stringify(join(ui, 'src/icons/FileIcon.tsx'))}
    export function render(row, theme) {
      return renderToStaticMarkup(createElement(FileIcon, { row, theme }))
    }
  `)
  await build({
    root: ui,
    logLevel: 'error',
    build: {
      ssr: entry,
      outDir: out,
      emptyOutDir: false,
      rollupOptions: { output: { entryFileNames: 'icons.mjs' } },
    },
  })
  const { render } = await import(pathToFileURL(join(out, 'icons.mjs')).href)
  for (const theme of ['light', 'dark']) {
    for (const expanded of [false, true]) {
      const row = { name: 'unknown-folder', kind: 'dir', expanded }
      const normal = render(row, theme)
      const linked = render({ ...row, symlink: true }, theme)
      assert.ok(!normal.includes('data-symlink'), 'ordinary folders have no link badge')
      assert.ok(linked.includes('data-symlink="true"'), 'linked folders draw their badge')
      assert.ok(linked.includes('data-icon="link"'), 'the badge uses the shared link icon')
      assert.equal((linked.match(/<img /g) ?? []).length, 1, 'preserve the folder icon')
      assert.equal(/src="([^"]+)"/.exec(linked)?.[1], /src="([^"]+)"/.exec(normal)?.[1],
        'linked folders keep their usual theme and expanded-state icon')
    }
    const file = render({ name: 'linked.rs', kind: 'file', symlink: true }, theme)
    assert.ok(file.includes('data-icon="link"') && /rust(?:_light)?\.svg/.test(file), 'linked files keep their type icon')
  }
  console.log('Symlink icon rendering passed in both themes and expansion states.')
} finally {
  rmSync(out, { recursive: true, force: true })
}
