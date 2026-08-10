/*
 * Paint the right theme on the very first frame.
 *
 * A classic <script> in <head>, so it runs while the parser is still above <body> and no
 * pixel has been drawn yet. React cannot do this job: by the time `main.tsx` has loaded its
 * module graph the window has already painted, and the flash is the whole complaint.
 *
 * The theme arrives as `?theme=` on the URL, baked in by `windows.rs` from the saved
 * settings — a webview has no other way to know it before its first frame. localStorage was
 * the alternative and lost: it would be a second copy of a setting `workspace.json` already
 * owns, written by the frontend, and it would go stale exactly when the app is launched
 * fresh after a theme change.
 *
 * Not inlined into index.html: the CSP is `script-src 'self'` with no `unsafe-inline`, so
 * an inline block would be silently dropped in the packaged app while working fine in dev.
 * A file in `public/` is served at the root by both.
 *
 * `light` is the default and is also the bare `:root` block in tokens.css, so the only
 * attribute that has to be set here is `dark`. Setting it either way anyway, because
 * `App.tsx` reads this attribute back as the pre-hydration theme and an absent one would
 * make it guess.
 */
;(function () {
  try {
    var theme = new URLSearchParams(window.location.search).get('theme')
    document.documentElement.dataset.theme = theme === 'dark' ? 'dark' : 'light'
  } catch (_) {
    // A malformed URL must not stop the app from booting; :root is already light.
  }
})()
