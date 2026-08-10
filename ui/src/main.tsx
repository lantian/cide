import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { App } from './App'
// Fonts before tokens: `tokens.css` names the families in `--font-ui` / `--font-mono`, and
// a face that is not yet declared when the first frame paints shows the fallback stack for
// a beat. Both are bundled locally — the CSP names no external font host.
import './styles/fonts.css'
import './styles/tokens.css'
import { installThemeSync } from './settings/useSettings'

const root = document.getElementById('root')
if (!root) throw new Error('#root is missing from index.html')

// Before the first render, and here rather than in an `App` effect. It settles what theme
// this window starts on — `<html data-theme>` from `theme-boot.js` if there is one — and then
// follows `workspace.settings.theme`, so a switch made in ANY window repaints this one's
// document and its live terminals. An effect would run after the first render, by which time
// a `TerminalPane` can already have built a terminal off the wrong palette; and `App.tsx`
// belongs to another author, so a call that has to exist cannot live there.
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
