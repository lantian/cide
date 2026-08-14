/**
 * One row's icon: a single `<img>` pointing at a vendored file.
 *
 * **Why `<img>` and not inline SVG or a sprite.** Three shapes were on the table and the tree
 * makes the choice for us, because it is virtualized: ~40 rows are mounted at any moment out
 * of a possible 100k, and rows are remounted continuously while scrolling.
 *
 *   * *Inline SVG components* — one module per icon, each rendering 2–10 elements. Best fidelity
 *     (CSS can reach inside) and the worst scaling: every icon's markup is in the JS bundle
 *     whether or not it is ever shown, so growing the set from 60 to upstream's 1,500 is paid
 *     for on every cold start, and 40 rows becomes several hundred extra DOM nodes to diff.
 *   * *A sprite sheet* — one `<svg><use/></svg>` per row, 2 nodes, and the whole sheet in the
 *     bundle. `<use href="/sprite.svg#rust">` would keep it out of the bundle, but WebKit has
 *     never implemented external-document `use` and this app runs on WebKitGTK: every row
 *     would be empty, which is precisely the silent failure this file is trying not to have.
 *     An *inlined* sheet works but is back to paying for the whole set up front.
 *   * *`<img src>` to a file* — 1 node, zero bytes in the JS bundle, and the decoded image is
 *     shared by every row that uses it. Vendoring 1,500 icons instead of 60 would cost nothing
 *     at runtime, which is the property that stops the next person having to rewrite this.
 *
 * So: `<img>`. The price is that an icon is opaque to the page's stylesheet — no `currentColor`,
 * no theming by CSS variable. That is not a loss here: these icons are multi-colour by design
 * and a single tint would be a different icon set. The light theme is handled the way upstream
 * handles it, with a second file — see `iconFor`.
 *
 * The CSP is satisfied without a special case: `img-src 'self'` covers `public/icons/`, and no
 * external host is named anywhere in this module.
 */
import { iconFor, themed, type IconRow, type IconTheme } from './iconFor'
import styles from './FileIcon.module.css'

/**
 * `./icons/<stem>.svg`.
 *
 * Built off `BASE_URL` rather than written as a root-absolute `/icons/...`: `vite.config.ts`
 * pins `base: './'` because Tauri serves the bundle from a custom protocol, and a root-absolute
 * path is the thing that works in `vite dev` and 404s in the packaged app. Every window loads
 * the same top-level `index.html` (`windows.rs`), so the relative form has one meaning.
 */
export function iconUrl(stem: string): string {
  return `${import.meta.env.BASE_URL}icons/${stem}.svg`
}

export interface FileIconProps {
  /** The row. `TreeRow` satisfies this structurally — pass it whole. */
  row: IconRow
  /**
   * Threaded from the panel, never subscribed to per row.
   *
   * `FileTree` already makes this argument about the git status map: the rows are not
   * memoized, so a subscription inside the row component is one store listener created and
   * torn down per visible row on every scroll tick. Use {@link useIconTheme} once, at the
   * panel, and pass the value down.
   */
  theme: IconTheme
  /** Extra class for layout — the caller owns where the icon sits in its row. */
  className?: string | undefined
  /**
   * An icon stem chosen by the caller, bypassing the name lookup.
   *
   * For the one row that has no filename to look up: the file tree's synthetic group headers
   * (`sidebar/groupRows.ts`'s `groupIcon`). The theme suffix is still applied here, so the
   * override cannot forget it and 404 into a blank row.
   *
   * Deliberately not "put `folder-lib` in the Material tables": `iconFor`'s header says the
   * associations are transcribed upstream's and an association invented here would be a bug
   * even if it looked reasonable. A group header is not a filename association at all.
   */
  stem?: string | undefined
}

export function FileIcon({ row, theme, className, stem }: FileIconProps) {
  return (
    <img
      className={className === undefined ? styles.icon : `${styles.icon} ${className}`}
      src={iconUrl(stem === undefined ? iconFor(row, theme) : themed(stem, theme))}
      /*
       * Decorative. The row already carries the filename as text and the git status as an
       * aria-label; an alt string here would make a screen reader read every row twice, once
       * for the icon's guess at the file type and once for its actual name.
       */
      alt=""
      aria-hidden="true"
      /* Intrinsic size, so the row does not reflow between the layout pass and the decode.
         It has to be the number `FileIcon.module.css` sets, and it was left at 16 when that
         moved to 15 — harmless while the stylesheet is there, since a CSS `width` beats a
         presentational attribute, and a 1px reflow of every row in the tree on the one pass
         where it is not. Kept in step by hand: an `<img>` cannot read a CSS module. */
      width={15}
      height={15}
      /* A file tree row drags as a row, if it ever drags; the icon must not drag as a picture. */
      draggable={false}
    />
  )
}
