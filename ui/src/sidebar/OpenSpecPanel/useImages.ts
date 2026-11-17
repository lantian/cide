import { useCallback, useEffect, useRef, useState } from 'react'
import { image, specImages } from '@/ipc/client'
import type { ImageDoc } from '@/ipc/generated'
import { notify, notifyFailure } from '@/chrome/notices'

export function useImages(key: string) {
  const [docs, setDocs] = useState<(ImageDoc & { name: string; order: number })[]>([])
  const [pending, setPending] = useState(0)
  const owned = useRef<(ImageDoc & { name: string; order: number })[]>([])
  const generation = useRef(0)
  const sequence = useRef(0)
  const release = (docs: readonly ImageDoc[]) => {
    if (docs.length) void specImages.release(docs.map(d => d.path)).catch(notifyFailure)
  }
  const clear = useCallback(() => {
    generation.current++
    release(owned.current)
    owned.current = []
    setDocs([])
    setPending(0)
  }, [])
  useEffect(() => { clear(); return clear }, [key, clear])
  const add = async (pick: boolean, paths?: readonly string[]) => {
    const mine = generation.current
    const order = sequence.current++
    setPending(n => n + 1)
    try {
      const sources = pick ? await specImages.pick() : paths
      if (mine !== generation.current || sources?.length === 0) return
      const next = (await specImages.stage(sources)).map((doc, i) => ({ ...doc, order, name: sources?.[i]?.split(/[\\/]/).pop() ?? 'Pasted image.png' }))
      if (mine !== generation.current) { release(next); return }
      // A paste the composer claimed and Rust found no image in. Said aloud, as the task
      // composer says it: the claim is "no text", not "an image" (WebKit never names one — see
      // `wantsImagePaste`), so an empty clipboard lands here, and so does a clipboard read that
      // failed. Silence here is the bug this replaced: Ctrl+V that did nothing at all.
      if (!pick && paths === undefined && next.length === 0) notify('The clipboard holds no image.', { kind: 'warn' })
      owned.current = [...owned.current, ...next].sort((a, b) => a.order - b.order)
      setDocs(owned.current)
    } catch (error) { if (mine === generation.current) notifyFailure(error) }
    finally { if (mine === generation.current) setPending(n => n - 1) }
  }
  return {
    docs, pending: pending > 0, clear,
    paste: () => { void add(false) },
    pick: () => { void add(true) },
    drop: (paths: readonly string[]) => { void add(false, paths) },
    remove: (path: string) => {
      release(owned.current.filter(d => d.path === path))
      owned.current = owned.current.filter(d => d.path !== path)
      setDocs(owned.current)
    },
    previews: docs.map((doc, i) => ({ path: doc.path, label: `Image ${i + 1}`, name: doc.name, src: image.srcUrl(doc) })),
  }
}
