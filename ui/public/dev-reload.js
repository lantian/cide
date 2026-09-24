/*
 * Ctrl+Shift+R (⌘⇧R) reloads the webview — dev builds only.
 *
 * The escape hatch for the white window a half-edited module leaves behind: Vite's HMR throws,
 * React unmounts the root, and there is nothing on screen to click and no browser chrome to
 * reload from. Rust owns every durable piece of state and sessions live outside the tree
 * (ADR 0002), so a reload loses nothing — it is the same full-page reload Vite itself does
 * when HMR gives up.
 *
 * Why this is a classic script in `public/` and not a command in `cide-core::commands`: the
 * registry is dispatched by `keys/gate.ts`, which is part of the module graph that just failed
 * to evaluate. A binding that only works while the UI works is no use for the one case it is
 * for. This file has no imports and is loaded by `index.html` before `main.tsx`, so it is
 * listening even when the bundle never ran. Not inlined for the CSP reason `theme-boot.js`
 * gives.
 *
 * Why not plain Ctrl+R: it is two things already. In a shell or a Claude pane it is reverse
 * history search, which xterm must receive, and in the file tree it is rename
 * (`sidebar/clickSemantics.ts`, which keeps `ctrl+shift+r` free on purpose). A capture
 * listener registered here runs before the gate's and before xterm's, so taking Ctrl+R would
 * have silently stolen both.
 *
 * Why dev only: a packaged build loads from `ui/dist`, which cannot be half-edited, and a
 * stray reload there is just a flicker for nothing. The dev server is `devUrl` in
 * `tauri.conf.json`; the packaged origin is `tauri://localhost` / `http://tauri.localhost`,
 * neither of which has port 1420.
 */
;(function () {
  if (window.location.port !== '1420') return
  window.addEventListener(
    'keydown',
    function (ev) {
      /*
       * `code`, not `key`: with Shift held `key` is `R`, and on a Cyrillic layout it is `К` —
       * `latin.ts` has the long version of that story, and this file runs before its rewrite.
       */
      if (ev.code !== 'KeyR' || !ev.shiftKey || ev.altKey) return
      if (!(ev.ctrlKey || ev.metaKey)) return
      ev.preventDefault()
      ev.stopImmediatePropagation()
      window.location.reload()
    },
    true,
  )
})()
