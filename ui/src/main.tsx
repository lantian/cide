import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { App } from './App'
import { installThemeSync } from './settings/useSettings'
// Fonts before tokens: `tokens.css` names the families in `--font-ui` / `--font-mono`, and
// a face that is not yet declared when the first frame paints shows the fallback stack for
// a beat. Both are bundled locally — the CSP names no external font host.
import './styles/fonts.css'
import './styles/tokens.css'

const root = document.getElementById('root')
if (!root) throw new Error('#root is missing from index.html')

// Make this window follow a theme change made in a *different* window. `applyTheme` moves
// the local field and persists; every other window learns about it only through
// `cide://workspace-changed`, and that event replaces `boot` — a field the code driving
// `documentElement.dataset.theme` does not read. Without this subscription a second window
// keeps its old palette until it is restarted, which is exactly what was reported.
//
// Here rather than in an `App` effect because it is window-lifetime, not component
// lifetime: module scope runs once per webview, so StrictMode's double-mount cannot install
// it twice and no unmount can tear it down while the window is still open. (It is
// idempotent either way, so an `App`-level install added later is harmless.)
installThemeSync()

// StrictMode double-invokes effects in development, which is exactly the pressure a pane
// host registry should be under: spawning a PTY twice per pane would be a real bug, and
// `TerminalPane`'s per-pane spawn guard is what prevents it. Leaving StrictMode on keeps
// that guard honest.
createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
