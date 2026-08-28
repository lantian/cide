/**
 * The URL an installed extension's file is served from.
 *
 * `ipc/client.ts` owns the exported `extAssetUrl(id, path)` — the one place a path out of a
 * manifest becomes a URL — and the *spelling* of that URL lives here, alone, because it is a
 * platform split that cannot be seen from the platform cide is developed on.
 *
 * # Two spellings, and which platform gets which
 *
 * Tauri serves a custom scheme two ways, and the boundary is **Windows and Android on one side,
 * everything else on the other** — not "Linux versus the desktops that are not Linux":
 *
 * * Windows, Android: `http://<scheme>.localhost/<path>`
 * * macOS, iOS, Linux: `<scheme>://localhost/<path>`
 *
 * That is `tauri-2.11.5/scripts/core.js`'s injected `convertFileSrc`, which branches on
 * `osName === 'windows' || osName === 'android'` and nothing else, and `tauri::App`'s
 * `register_uri_scheme_protocol` doc says the same from the Rust side.
 *
 * Putting macOS on the Windows side is what shipped, and it is invisible here: on Linux the
 * function returns the right string, every check passes, and on a Mac **every extension with a
 * worker fails to start** with `its worker could not be loaded from
 * http://cide-ext.localhost/…` — because nothing serves that origin there, so the module fetch
 * 404s and a module worker that fails to load fires a message-less `Event` (see `host.ts`'s
 * `describeError`). Three extensions reported at once, which is what a platform-wide URL bug
 * looks like from the outside.
 *
 * # Why the user agent
 *
 * Detected the way Tauri's own `convertFileSrc` is detected from the frontend — off the user
 * agent — because `@tauri-apps/plugin-os` answers asynchronously and this is called while a
 * worker is being constructed. `windowControls.ts`'s header sets out the same argument at
 * length for the same reason.
 *
 * DOM-free and import-free on purpose: `scripts/check-ext.mjs` compiles this file standalone and
 * drives it with one user agent per platform, which is the only way the macOS answer is ever
 * asserted from a Linux machine. Keep it that way — the caller passes the user agent in.
 */

/** The scheme `cide-app` registers; `cide_ext::assets::SCHEME` is the same string in Rust. */
export const EXT_SCHEME = 'cide-ext'

/**
 * True when this user agent came from a web view whose custom schemes are spelled
 * `http://<scheme>.localhost/…`.
 *
 * `Windows` is the platform token every web view on Windows carries, WebView2 included.
 * `Android` is checked because Tauri branches on it, not because cide has an Android target —
 * a predicate that quietly disagrees with the framework it is mirroring is worse than an unused
 * arm. Everything else, an empty or unrecognised agent included, gets `<scheme>://localhost/`:
 * that is the majority spelling and the one the development platform uses, so it is the right
 * way to be wrong.
 */
export function usesLocalhostSubdomain(userAgent: string): boolean {
  return userAgent.includes('Windows') || userAgent.includes('Android')
}

/**
 * Where `<marketplace>/<extension>/<path>` is served from, for the platform this agent describes.
 *
 * The identity pair rides in the **path** and not the host — see `cide_ext::assets::split_path`,
 * which says why from the other end: the host is the scheme's own name on two of the four
 * platforms, so an identity put there survives only where it would have been tested.
 */
export function extAssetUrlFor(
  userAgent: string,
  marketplace: string,
  extension: string,
  path: string,
): string {
  const tail = `${marketplace}/${extension}/${path.replace(/^\/+/, '')}`
  return usesLocalhostSubdomain(userAgent)
    ? `http://${EXT_SCHEME}.localhost/${tail}`
    : `${EXT_SCHEME}://localhost/${tail}`
}
