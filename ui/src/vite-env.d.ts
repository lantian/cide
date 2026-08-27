/// <reference types="vite/client" />

// Vite's client types cover `*.module.css` as `Record<string, string>`, which is enough
// for correctness but gives no autocomplete on class names. That trade is deliberate:
// generating per-file typed CSS modules would add a build step to every stylesheet edit.

/**
 * The repository root, substituted by `vite.config.ts`.
 *
 * Empty in a production build — see the comment on the `define` there, and on
 * `AUDIT_PROJECT_ROOT` in `App.tsx`. Anything reading it must treat `''` as "no repository",
 * not as a path.
 */
declare const __CIDE_REPO_ROOT__: string
