/// <reference types="vite/client" />

// Vite's client types cover `*.module.css` as `Record<string, string>`, which is enough
// for correctness but gives no autocomplete on class names. That trade is deliberate:
// generating per-file typed CSS modules would add a build step to every stylesheet edit.
