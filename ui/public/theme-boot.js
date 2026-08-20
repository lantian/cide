/*
 * Paint the right theme and the right chrome size on the very first frame.
 *
 * A classic <script> in <head>, so it runs while the parser is still above <body> and no
 * pixel has been drawn yet. React cannot do this job: by the time `main.tsx` has loaded its
 * module graph the window has already painted, and the flash is the whole complaint.
 *
 * Both values arrive on the URL, baked in by `windows.rs` from the saved settings — a webview
 * has no other way to know them before its first frame. localStorage was the alternative and
 * lost: it would be a second copy of settings `workspace.json` already owns, written by the
 * frontend, and it would go stale exactly when the app is launched fresh after a change.
 *
 * Not inlined into index.html: the CSP is `script-src 'self'` with no `unsafe-inline`, so
 * an inline block would be silently dropped in the packaged app while working fine in dev.
 * A file in `public/` is served at the root by both.
 *
 * The name is still `theme-boot.js` after it grew the second job. Renaming a file in
 * `public/` means moving a path that `index.html`, the dev server and the packaged bundle all
 * resolve independently, which is a bigger surface than the word "theme" is worth.
 */
;(function () {
  try {
    var theme = new URLSearchParams(window.location.search).get('theme')
    /*
     * `light` is the default and is also the bare `:root` block in tokens.css, so the only
     * attribute that has to be set here is `dark`. Setting it either way anyway, because
     * `App.tsx` reads this attribute back as the pre-hydration theme and an absent one would
     * make it guess.
     */
    document.documentElement.dataset.theme = theme === 'dark' ? 'dark' : 'light'
  } catch (_) {
    // A malformed URL must not stop the app from booting; :root is already light.
  }

  /*
   * The chrome scale, and why this file does arithmetic at all.
   *
   * `--ui-scale` is a *multiplier*, not a size: `tokens.css` builds fourteen font sizes and
   * every text-bearing box out of `calc(<design px> * var(--ui-scale))`, so what has to reach
   * the document is the ratio and not the point size. The division is here rather than in CSS
   * because dividing a length by a length is CSS Values 4, and this app's engine
   * (WebKitGTK 2.52) is the one place in this repo where a support table is not evidence —
   * see `check-css-prefix.mjs` for what that costs when it is wrong. `fontScale.ts` does the
   * same arithmetic for the same reason.
   *
   * The three literals below are duplicated on purpose and pinned by `check-ui-scale.mjs`:
   * this file is loaded as a bare classic script with no module graph, so it cannot import
   * `fontScale.ts`, and a silent disagreement here would be a first frame at one scale and
   * every frame after it at another.
   */
  var BASE = 13
  var MIN = 9
  var MAX = 20
  try {
    var size = parseFloat(new URLSearchParams(window.location.search).get('ui'))
    /*
     * A missing or unparseable parameter leaves the property alone rather than writing 1.
     * `tokens.css` already declares `--ui-scale: 1`, so the fallback is the stylesheet's, and
     * an inline property written here would outrank anything `paintUiScale` later sets only
     * if it were wrong — this way the absent case and the default case are the same case.
     */
    if (isFinite(size)) {
      var clamped = Math.min(MAX, Math.max(MIN, size))
      document.documentElement.style.setProperty('--ui-scale', String(clamped / BASE))
    }
  } catch (_) {
    // As above: :root already carries `--ui-scale: 1`.
  }
})()
