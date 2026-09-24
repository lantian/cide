/**
 * Re-renders when `<html>`'s `data-theme` or inline style (`--ui-scale`) changes, so a specimen
 * that prints a computed value — a swatch's hex, a size in pixels — prints the current one.
 */
import { useEffect, useState } from 'react'

export function useRootTick(): number {
  const [tick, setTick] = useState(0)
  useEffect(() => {
    const observer = new MutationObserver(() => setTick((t) => t + 1))
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ['data-theme', 'style'],
    })
    return () => observer.disconnect()
  }, [])
  return tick
}

/** A custom property's value on `<html>`, trimmed; empty under node (`check:kit`). */
export function rootValue(name: string): string {
  if (typeof document === 'undefined') return ''
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim()
}
