/**
 * The keymap lookup — ONE table, built once from what `app_get_bootstrap` returned.
 *
 * The table itself lives in Rust (`cide-core::keymap`), which layers defaults → platform →
 * user and hands the frontend an already-resolved, already-normalised list. Nothing here
 * invents a binding; this module only indexes that list so a keystroke sequence can be
 * answered in constant time and so multi-stroke sequences can be told apart from unbound
 * ones.
 *
 * The binding type is structural rather than `ResolvedBinding` imported from
 * `@/ipc/client`. Two reasons, both real: this module and its dependents must stay
 * compilable standalone for `ui/scripts/check-key-gate.mjs`, and the palette also wants to
 * ask "what key runs this command" from a keymap it built out of a fixture. `ResolvedBinding`
 * is assignable to [`KeyBinding`], so nothing is lost at the call site.
 */
import { normalizeSequence, prefixesOf, chipLabel } from './chords'
import { evaluateWhen, type KeyContext } from './when'

export type { KeyContext }

/** A binding, structurally. `ResolvedBinding` from the generated IPC types satisfies this. */
export interface KeyBinding {
  key: string
  command: string
  when?: string | null | undefined
  args?: unknown
}

/** What a keystroke sequence resolved to. */
export type Resolution =
  /** Nothing is bound here and nothing could become bound by pressing more keys. */
  | { kind: 'unbound' }
  /** A live prefix: some binding continues from here, so the next stroke belongs to it. */
  | { kind: 'prefix'; sequence: string }
  | { kind: 'run'; sequence: string; command: string; args: unknown }

export interface Keymap {
  resolve(sequence: string, ctx: KeyContext): Resolution
  /** The key sequence bound to a command, canonically spelled, or `null`. */
  keyFor(command: string): string | null
  /** [`keyFor`] rendered as the palette's chip text, or `null`. */
  chipFor(command: string): string | null
  /** Every binding, normalised. Exposed for the gate's self-check and for Settings → Keymap. */
  all(): readonly NormalizedBinding[]
}

/** A binding whose `key` has been through [`normalizeSequence`]. */
export interface NormalizedBinding {
  key: string
  command: string
  when: string | null
  args: unknown
}

export function buildKeymap(bindings: readonly KeyBinding[]): Keymap {
  const all: NormalizedBinding[] = []
  const exact = new Map<string, NormalizedBinding[]>()
  const continuations = new Map<string, NormalizedBinding[]>()

  for (const binding of bindings) {
    const key = normalizeSequence(binding.key)
    if (key === '') continue
    // A `-command` entry is a removal directive that Rust has already applied. One reaching
    // the frontend would bind a command literally named `-picker.files`, which exists
    // nowhere; drop it rather than index a key that can only ever be a dead end.
    if (binding.command.startsWith('-')) continue

    const normalized: NormalizedBinding = {
      key,
      command: binding.command,
      when: binding.when ?? null,
      args: binding.args,
    }
    all.push(normalized)

    const sameKey = exact.get(key)
    if (sameKey) sameKey.push(normalized)
    else exact.set(key, [normalized])

    for (const prefix of prefixesOf(key)) {
      const following = continuations.get(prefix)
      if (following) following.push(normalized)
      else continuations.set(prefix, [normalized])
    }
  }

  /**
   * Scanned from the end for the same reason resolution takes the last match: the user
   * layer is appended last, so their rebinding is what a chip should show.
   *
   * A free function rather than a method so `chipFor` can call it without `this` — a
   * destructured `const { chipFor } = keymap` is an ordinary thing to write and would
   * otherwise throw.
   */
  function keyFor(command: string): string | null {
    for (let i = all.length - 1; i >= 0; i--) {
      const binding = all[i] as NormalizedBinding
      if (binding.command === command) return binding.key
    }
    return null
  }

  return {
    all: () => all,
    keyFor,

    chipFor(command) {
      const key = keyFor(command)
      return key === null ? null : chipLabel(key)
    },

    resolve(sequence, ctx) {
      /*
       * Prefix before exact, and only when a *continuation* is applicable.
       *
       * If `ctrl+k` were bound on its own and `ctrl+k ctrl+s` also existed, preferring the
       * exact match would make the two-stroke sequence unreachable — the first stroke would
       * fire and clear. Preferring the prefix unconditionally has the opposite failure: a
       * chord whose continuations are all gated off by `when` would swallow the stroke and
       * then resolve to nothing. Checking applicability first is what gets both right.
       */
      const following = continuations.get(sequence)
      if (following?.some((b) => evaluateWhen(b.when, ctx))) {
        return { kind: 'prefix', sequence }
      }

      const candidates = exact.get(sequence)
      if (candidates) {
        // Last applicable wins. `keymap.rs` states it: within `conflicts`, "the last one is
        // the one that will win", because later layers are appended after earlier ones.
        for (let i = candidates.length - 1; i >= 0; i--) {
          const binding = candidates[i] as NormalizedBinding
          if (evaluateWhen(binding.when, ctx)) {
            return {
              kind: 'run',
              sequence,
              command: binding.command,
              args: binding.args,
            }
          }
        }
      }

      return { kind: 'unbound' }
    },
  }
}
