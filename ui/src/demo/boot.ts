/**
 * Pick the scene from the URL, build its command table, and install the fake backend.
 *
 * Side effects only, and imported first by `demo/main.tsx` — see that file for why the order is
 * the mechanism.
 */
import { emptyAnswer } from './defaults'
import { installFakeTauri } from './fakeTauri'
import { buildScene } from './scenes'

// Captures run at a device pixel ratio of 2 for crisp text, and the app's terminal arithmetic is
// only right at 1: `settings/fontScale.ts::codeMetrics` divides a CSS-pixel leading by a
// device-pixel glyph box, so at dpr 2 it hands xterm a multiplier under 1, which xterm rejects
// ("lineHeight cannot be less than 1") and every terminal keeps its default leading. Reporting 1
// here keeps the demo on the arithmetic the app is tested at; the DOM renderer draws text as
// CSS, so the picture is still rendered at the real ratio.
Object.defineProperty(window, 'devicePixelRatio', { get: () => 1 })

const params = new URLSearchParams(location.search)
const scene = buildScene(params.get('scene') ?? 'claude', params.get('theme') === 'light' ? 'light' : 'dark')
installFakeTauri(scene.window, scene.handlers, emptyAnswer)
void scene.ready()
