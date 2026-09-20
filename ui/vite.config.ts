import { cpSync, createReadStream, existsSync, statSync } from 'node:fs'
import { resolve, sep } from 'node:path'
import { fileURLToPath, URL } from 'node:url'
import { defineConfig, type Plugin } from 'vite'
import react from '@vitejs/plugin-react'

// The repository this `ui/` sits in. The audits (`run.sh --audit-*`) need a real project to
// open, and the only one guaranteed to exist on the machine running them is the checkout
// they were launched from — so it is computed here rather than written down. It was written
// down once, as one author's absolute home path, which made every audit a silent no-op on
// any other clone.
const repoRoot = fileURLToPath(new URL('..', import.meta.url)).replace(/\/$/, '')

/**
 * Excalidraw's fonts, served from the package in dev and copied beside the bundle at build. (M63)
 *
 * `@excalidraw/excalidraw` loads its hand-drawn faces (Excalifont, Virgil, Cascadia, …) at
 * runtime, lazily per font, from `window.EXCALIDRAW_ASSET_PATH` — 234 woff2 files, 12.5 MiB,
 * under `dist/prod/fonts/<Family>/`. With that global unset it falls back to a CDN, and the CSP
 * (`font-src 'self' data:`) blocks the CDN silently: every label renders in a fallback face and
 * nothing is logged anywhere. So the fonts must be reachable under the app's own origin.
 *
 * Two roads were rejected. Committing them under `public/` puts 12.5 MiB of binaries in the
 * repository for a directory `pnpm install` already provides. Serving them only in dev from
 * `/node_modules/...` and only at build from a copy gives the two launch paths two different
 * URL shapes, which is the class of break `scripts/check-css-prefix.mjs` exists for: right on
 * whichever half the author tried, wrong on the other. So one prefix, `/excalidraw/fonts/`,
 * answered by this middleware under `vite dev` and by the copied directory under `vite build`,
 * and `panes/ExcalidrawPane.tsx` spells the same `excalidraw/` once, relative to the document.
 *
 * `closeBundle` throws when the source is missing rather than building without it, because a
 * build with no fonts is not an error anybody would otherwise see. It is skipped for SSR builds:
 * every `check:*-render` script runs `vite build --ssr` through this same config into a cache
 * directory, and copying fonts into each of those would be a silent tax on every check.
 */
function excalidrawFonts(): Plugin {
  const fonts = fileURLToPath(
    new URL('./node_modules/@excalidraw/excalidraw/dist/prod/fonts', import.meta.url),
  )
  const prefix = '/excalidraw/fonts'
  let outDir = 'dist'
  let ssr = false
  return {
    name: 'cide:excalidraw-fonts',
    configResolved(config) {
      outDir = config.build.outDir
      ssr = Boolean(config.build.ssr)
    },
    configureServer(server) {
      server.middlewares.use(prefix, (req, res, next) => {
        const rel = decodeURIComponent((req.url ?? '/').split('?')[0] ?? '/')
        const file = resolve(fonts, `.${rel}`)
        // Inside the fonts directory, a real file, and a font: anything else is not ours.
        if (
          !file.startsWith(fonts + sep)
          || !file.endsWith('.woff2')
          || !existsSync(file)
          || !statSync(file).isFile()
        ) {
          next()
          return
        }
        res.setHeader('Content-Type', 'font/woff2')
        res.setHeader('Cache-Control', 'max-age=31536000, immutable')
        createReadStream(file).pipe(res)
      })
    },
    closeBundle() {
      if (ssr) return
      if (!existsSync(fonts)) {
        throw new Error(
          `${fonts} is missing — run \`pnpm --dir ui install\`. A build without Excalidraw's `
            + 'fonts renders every drawing label in a fallback face and reports nothing.',
        )
      }
      cpSync(fonts, resolve(outDir, 'excalidraw', 'fonts'), { recursive: true })
    },
  }
}

export default defineConfig(({ command }) => ({
  plugins: [react(), excalidrawFonts()],

  // Pre-bundled up front. The package is reached only through a dynamic `import()` one chunk
  // deep (`panes/ExcalidrawPane.tsx`), and a dependency the scanner misses is discovered on
  // first use — at which point Vite logs *new dependencies optimized, reloading* and reloads
  // the page. In this app a page reload mid-session drops every terminal's attachment, so the
  // optimizer is told rather than left to find out.
  optimizeDeps: {
    include: ['@excalidraw/excalidraw'],
  },

  // Substituted into `App.tsx`'s `AUDIT_PROJECT_ROOT`. Only while *serving*: a production
  // build gets the empty string, so no machine's directory layout is ever baked into
  // `ui/dist` — which ships to users. `command === 'serve'` is the whole guard, and the
  // audits are dev-only, so nothing that reads it loses anything.
  define: {
    __CIDE_REPO_ROOT__: JSON.stringify(command === 'serve' ? repoRoot : ''),
  },

  // Declared here as well as in tsconfig `paths`. tsconfig covers typechecking, but
  // Vite's dependency pre-bundling scan does not read it, and without this the scan fails
  // on every `@/` import and silently skips pre-bundling.
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },

  // `strictPort` matters: Tauri's `devUrl` is a fixed http://localhost:1420. If Vite
  // silently moved to 1421 because something else held the port, the webview would load a
  // blank page and the failure would look like a Tauri problem rather than a port clash.
  server: {
    port: 1420,
    strictPort: true,
    host: '127.0.0.1',
    watch: {
      // Rust rebuilds are driven by cargo, and watching target/ on a workspace this size
      // is a reliable way to exhaust inotify watches.
      ignored: ['**/target/**', '**/crates/**'],
    },
  },

  // Tauri serves the built assets from a custom protocol, so relative paths are required.
  base: './',
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    // WebKitGTK 2.52 is comfortably modern; targeting it directly avoids shipping
    // transpiled-down output that the engine would have handled natively.
    target: 'safari18',
    sourcemap: true,
  },

  clearScreen: false,
}))
