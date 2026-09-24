/**
 * The demo's entry. The import order below is the whole mechanism.
 *
 * ES modules evaluate their imports depth-first in source order, so `./boot` — which installs
 * the fake backend and the scene's data — has fully run before `../main` evaluates, and so
 * before `installThemeSync()` or the first `invoke` of `App`'s boot. Folding both into one
 * module, or importing `../main` first, would let the app's first `app_get_bootstrap` reach a
 * `window.__TAURI_INTERNALS__` that does not exist yet.
 */
import './boot'
import '../main'
