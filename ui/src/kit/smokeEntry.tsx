/**
 * `check:kit`'s entry: renders the whole kit page to a string under node and prints it, so the
 * check can assert every chapter and every component actually drew. Same shape as
 * `gitlab/smokeEntry.tsx`.
 */
import { renderToString } from 'react-dom/server'

import { CHAPTERS, Kit } from './Kit'

console.log(JSON.stringify({ html: renderToString(<Kit />), chapters: CHAPTERS.map((c) => c.id) }))
