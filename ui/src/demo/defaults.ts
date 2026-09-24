/**
 * The empty answer for a command the demo has no data for, read off `client.ts`'s own signature.
 *
 * Every call site in `client.ts` spells its answer type — `invoke<TreeRow[]>('fs_tree_rows', …)`
 * — so the honest "nothing here" for a command is derivable from the one file that is allowed to
 * issue it: `[]` for an array, `0`, `false`, `''`, an empty buffer, and `null` for everything else
 * (an object the caller must then treat as absent, which every caller already does for a command
 * that failed). Guessing it from the command's *name* was the first version and lost: `app_restore_plan`
 * answers an array and reads like a noun, and `App` crashed iterating the `null` it was handed.
 *
 * Read as `?raw` text at build time, so it can never disagree with the client it describes.
 */
import clientSource from '../ipc/client.ts?raw'

const SIGNATURE = /invoke<([^>]*(?:<[^>]*>)?[^>]*)>\(\s*'([a-z0-9_:|]+)'/g

const answerType = new Map<string, string>()
for (const m of clientSource.matchAll(SIGNATURE)) {
  const [, type, cmd] = m
  if (type && cmd && !answerType.has(cmd)) answerType.set(cmd, type.trim())
}

export function emptyAnswer(cmd: string): unknown {
  const type = answerType.get(cmd)
  if (type === undefined) return null
  if (/\|\s*null\b|\bnull\s*\|/.test(type)) return null
  if (type.endsWith('[]') || type.startsWith('Array<')) return []
  if (type === 'number') return 0
  if (type === 'boolean') return false
  if (type === 'string') return ''
  if (type === 'ArrayBuffer') return new ArrayBuffer(0)
  // A map the caller indexes into (`claude_session_names`): `null` there is a TypeError in the
  // task card, `{}` is the honest "no names".
  if (type.startsWith('Record<')) return {}
  // A mutation's answer. Revision 0 is behind every snapshot, so `synced()` never waits on it.
  if (/^\{\s*rev\b/.test(type)) return { rev: 0 }
  return null
}
