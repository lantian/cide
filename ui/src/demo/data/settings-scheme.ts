/**
 * Colour schemes for the demo's Settings ▸ Appearance: what `scheme_import` would have produced
 * from a handful of popular VS Code themes.
 *
 * Each palette is written once, as the dozen colours a theme's author actually picks, and spread
 * over every role by `scheme()` — the importer's own output is total against `SCHEME_ROLES` after
 * `ColorScheme::normalise`, and a scheme missing a role would fall back to the builtin's colour
 * for it, which is a scheme that is quietly half somebody else's.
 */
import type { ColorScheme, GraphicsStatus } from '../../ipc/generated'

interface Palette {
  bg: string
  fg: string
  sel: string
  gutter: string
  caret: string
  comment: string
  keyword: string
  string: string
  number: string
  fn: string
  type: string
  property: string
  punctuation: string
  macro: string
}

function scheme(id: string, name: string, polarity: 'dark' | 'light', source: string, p: Palette): ColorScheme {
  return {
    id,
    name,
    polarity,
    source,
    colors: {
      bg: p.bg,
      fg: p.fg,
      sel: p.sel,
      gutter: p.gutter,
      caret: p.caret,
      doc: p.comment,
      comment: p.comment,
      control: p.keyword,
      constant: p.number,
      escape: p.macro,
      regexp: p.string,
      attribute: p.macro,
      string: p.string,
      macro: p.macro,
      label: p.property,
      function: p.fn,
      namespace: p.type,
      type: p.type,
      keyword: p.keyword,
      number: p.number,
      property: p.property,
      variable: p.fg,
      bracket: p.punctuation,
      punctuation: p.punctuation,
      operator: p.keyword,
      heading: p.fn,
      strong: p.fg,
      emphasis: p.fg,
      link: p.type,
    },
  }
}

/** Already on disk before the scene starts: an earlier afternoon's imports. */
export const SCHEMES: ColorScheme[] = [
  scheme('tokyo-night', 'Tokyo Night', 'dark', 'enkia.tokyo-night-1.1.2.vsix', {
    bg: '#1a1b26', fg: '#a9b1d6', sel: '#283457', gutter: '#3b4261', caret: '#c0caf5',
    comment: '#565f89', keyword: '#bb9af7', string: '#9ece6a', number: '#ff9e64', fn: '#7aa2f7',
    type: '#2ac3de', property: '#73daca', punctuation: '#89ddff', macro: '#e0af68',
  }),
  scheme('tokyo-night-light', 'Tokyo Night Light', 'light', 'enkia.tokyo-night-1.1.2.vsix', {
    bg: '#e6e7ed', fg: '#343b58', sel: '#c4c8da', gutter: '#9699a8', caret: '#343b58',
    comment: '#888b94', keyword: '#65359d', string: '#385f0d', number: '#965027', fn: '#2959aa',
    type: '#006c86', property: '#33635c', punctuation: '#4c505e', macro: '#8f5e15',
  }),
  scheme('one-dark-pro', 'One Dark Pro', 'dark', 'zhuangtongfa.material-theme-3.19.0.vsix', {
    bg: '#282c34', fg: '#abb2bf', sel: '#3e4451', gutter: '#495162', caret: '#528bff',
    comment: '#7f848e', keyword: '#c678dd', string: '#98c379', number: '#d19a66', fn: '#61afef',
    type: '#e5c07b', property: '#e06c75', punctuation: '#abb2bf', macro: '#56b6c2',
  }),
  scheme('github-light-default', 'GitHub Light Default', 'light', 'github.github-vscode-theme-6.3.5.vsix', {
    bg: '#ffffff', fg: '#1f2328', sel: '#cce3fd', gutter: '#8c959f', caret: '#0969da',
    comment: '#59636e', keyword: '#cf222e', string: '#0a3069', number: '#0550ae', fn: '#8250df',
    type: '#953800', property: '#0550ae', punctuation: '#1f2328', macro: '#116329',
  }),
  scheme('github-dark-dimmed', 'GitHub Dark Dimmed', 'dark', 'github.github-vscode-theme-6.3.5.vsix', {
    bg: '#22272e', fg: '#adbac7', sel: '#2e4b6f', gutter: '#636e7b', caret: '#539bf5',
    comment: '#768390', keyword: '#f47067', string: '#96d0ff', number: '#6cb6ff', fn: '#dcbdfb',
    type: '#f69d50', property: '#6cb6ff', punctuation: '#adbac7', macro: '#8ddb8c',
  }),
  scheme('solarized-light', 'Solarized Light', 'light', 'solarized-light-color-theme.json', {
    bg: '#fdf6e3', fg: '#657b83', sel: '#eee8d5', gutter: '#93a1a1', caret: '#657b83',
    comment: '#93a1a1', keyword: '#859900', string: '#2aa198', number: '#d33682', fn: '#268bd2',
    type: '#b58900', property: '#268bd2', punctuation: '#657b83', macro: '#cb4b16',
  }),
  scheme('night-owl', 'Night Owl', 'dark', 'night-owl-color-theme.json', {
    bg: '#011627', fg: '#d6deeb', sel: '#1d3b53', gutter: '#4b6479', caret: '#80a4c2',
    comment: '#637777', keyword: '#c792ea', string: '#ecc48d', number: '#f78c6c', fn: '#82aaff',
    type: '#ffcb8b', property: '#7fdbca', punctuation: '#d6deeb', macro: '#addb67',
  }),
]

/** What pressing Import… brings in: a `.vsix` carrying a dark and a light variant. */
export const IMPORTED: ColorScheme[] = [
  scheme('catppuccin-mocha', 'Catppuccin Mocha', 'dark', 'catppuccin.catppuccin-vsc-3.17.0.vsix', {
    bg: '#1e1e2e', fg: '#cdd6f4', sel: '#45475a', gutter: '#7f849c', caret: '#f5e0dc',
    comment: '#9399b2', keyword: '#cba6f7', string: '#a6e3a1', number: '#fab387', fn: '#89b4fa',
    type: '#f9e2af', property: '#b4befe', punctuation: '#9399b2', macro: '#94e2d5',
  }),
  scheme('catppuccin-latte', 'Catppuccin Latte', 'light', 'catppuccin.catppuccin-vsc-3.17.0.vsix', {
    bg: '#eff1f5', fg: '#4c4f69', sel: '#ccd0da', gutter: '#8c8fa1', caret: '#dc8a78',
    comment: '#7c7f93', keyword: '#8839ef', string: '#40a02b', number: '#fe640b', fn: '#1e66f5',
    type: '#df8e1d', property: '#7287fd', punctuation: '#7c7f93', macro: '#179299',
  }),
]

/** The Linux webview workarounds, as a KDE/Wayland machine with the launcher's default reports. */
export const GRAPHICS: GraphicsStatus = {
  automatic: true,
  suppressedByEnv: false,
  rungs: [
    {
      variable: 'WEBKIT_DISABLE_DMABUF_RENDERER',
      label: 'Disable the DMABUF renderer',
      cost: 'A little compositing throughput. Applied automatically on Linux: without it this app exits with a Wayland protocol error on a stock KDE desktop.',
      setting: null,
      active: true,
    },
    {
      variable: '__NV_DISABLE_EXPLICIT_SYNC',
      label: 'Disable NVIDIA explicit sync',
      cost: 'Free on other hardware. A frequent cause of hangs on the proprietary driver.',
      setting: null,
      active: false,
    },
    {
      variable: 'WEBKIT_DISABLE_COMPOSITING_MODE',
      label: 'Disable accelerated compositing',
      cost: 'Turns off accelerated compositing for the whole webview. A four-terminal grid feels it immediately. Last resort.',
      setting: null,
      active: false,
    },
  ],
}
