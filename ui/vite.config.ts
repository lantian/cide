import { fileURLToPath, URL } from 'node:url'
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// The repository this `ui/` sits in. The audits (`run.sh --audit-*`) need a real project to
// open, and the only one guaranteed to exist on the machine running them is the checkout
// they were launched from — so it is computed here rather than written down. It was written
// down once, as one author's absolute home path, which made every audit a silent no-op on
// any other clone.
const repoRoot = fileURLToPath(new URL('..', import.meta.url)).replace(/\/$/, '')

export default defineConfig(({ command }) => ({
  plugins: [react()],

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
