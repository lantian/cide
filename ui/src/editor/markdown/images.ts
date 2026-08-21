/**
 * Local images in a preview, and the one call that makes them legal. (M20)
 *
 * `![](./diagram.png)` in a document is a request to read a file off disk and put it on screen,
 * and there is exactly one route in this app that is allowed to answer it: `image.read`, the same
 * call `panes/ImagePane.tsx` makes. What that buys, from README's *Images* section:
 *
 * * The bytes go over **Tauri's asset protocol**, never the IPC control plane. `emit`/`listen`
 *   string-interpolates JSON into an `eval`'d script, and a 32 MB PNG marshalled that way is the
 *   pathological case that argument exists for.
 * * The static `assetProtocol.scope` is **empty and stays empty** — "a scope wide enough to serve
 *   a repository is a scope wide enough to serve `~/.ssh`". `image_read` grants
 *   `asset_protocol_scope().allow_file()` for the one path, and grants it *after* the size, type
 *   and header refusals have run.
 * * **A file that is not what its extension claims fails with a sentence.** A `.png` holding a
 *   private key is refused on its header, in Rust, and never reaches the protocol.
 *
 * So this module resolves paths and caches answers; it decides nothing about whether a file may
 * be read. [`MAX_PREVIEW_IMAGES`] is the one thing it does decide, and the reason is written
 * there: a document is a thing an agent can write, and each image is one more standing grant.
 *
 * Remote images are not fetched and cannot be: the CSP in `crates/cide-app/tauri.conf.json` is
 * `img-src 'self' data: blob: asset: http://asset.localhost`, with no `https:`. They render as
 * their alt text, which `MarkdownPreview.tsx` draws as a marked-up absence rather than as a
 * broken-image glyph.
 */
import { diag, image as imageApi } from '@/ipc/client'
import { MAX_PREVIEW_IMAGES } from './view'

/** A resolved image: the asset URL, or the sentence Rust refused it with. */
export type ImageState = { readonly url: string } | { readonly refused: string }

const resolved = new Map<string, ImageState>()
const inflight = new Set<string>()
const listeners = new Set<() => void>()

function emit(): void {
  for (const listener of listeners) listener()
}

export function subscribeImages(listener: () => void): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

/** What is known about `path` right now. `undefined` means "not asked yet, or still asking". */
export function imageState(path: string): ImageState | undefined {
  return resolved.get(path)
}

/**
 * Ask Rust about `path`, at most once per window.
 *
 * The answer is cached including the refusal, deliberately: a `.png` that is not a PNG will be
 * refused identically every time, and re-asking on every render of a document that references it
 * would be one IPC round trip per frame for a file that will never load.
 *
 * The cap counts *distinct paths asked about*, not images on screen, so a document that
 * references the same logo forty times costs one grant.
 */
export function requestImage(path: string): void {
  if (resolved.has(path) || inflight.has(path)) return
  if (resolved.size + inflight.size >= MAX_PREVIEW_IMAGES) {
    resolved.set(path, {
      refused: `this preview has already loaded ${MAX_PREVIEW_IMAGES} images`,
    })
    emit()
    return
  }

  inflight.add(path)
  void imageApi
    .read(path)
    .then((doc) => {
      resolved.set(path, { url: imageApi.srcUrl(doc) })
    })
    .catch((error: unknown) => {
      const message = error instanceof Error ? error.message : String(error)
      resolved.set(path, { refused: message })
      diag.log(`markdown preview: ${path}: ${message}`)
    })
    .finally(() => {
      inflight.delete(path)
      emit()
    })
}

/**
 * Forget everything.
 *
 * Called when a markdown pane closes its preview. The asset-scope grants Rust made are not
 * revoked by this — nothing in Tauri's API revokes one — so this is a cache reset and not a
 * security measure, and saying so here is the point: the cap in [`requestImage`] is what bounds
 * the grants, and clearing this map is what lets a file the user has just fixed be re-read.
 */
export function clearImages(): void {
  resolved.clear()
  inflight.clear()
  emit()
}
