/**
 * The UI kit's entry — `kit.html`, not the app.
 *
 * The same four global stylesheets `main.tsx` loads, in the same order and for the same reasons
 * (see the comments there), so a specimen here paints with exactly the fonts, tokens and icon box
 * a cide window does. Nothing from `ipc/` is imported anywhere under `kit/`: the page runs in a
 * plain browser tab, where there is no Tauri to call.
 */
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import '../styles/fonts.css'
import '../styles/tokens.css'
import '../styles/iconMasks.css'
import '../icons/icon.css'
import { Kit } from './Kit'

const root = document.getElementById('root')
if (!root) throw new Error('#root is missing from kit.html')

createRoot(root).render(
  <StrictMode>
    <Kit />
  </StrictMode>,
)
