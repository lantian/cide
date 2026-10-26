import { useState } from 'react'
import { useDesktopDrop } from '@/files/desktopDrop'
import { dragDrop, type ProjectId } from '@/ipc/client'
import { useWorkspace } from '@/store/workspace'
import { notifyFailure } from './notices'

/** A desktop drop opens the original paths; it never copies them into the project. */
export function useTabFileDrop(container: React.RefObject<HTMLDivElement | null>, project: string | undefined): boolean {
  const [hot, setHot] = useState(false)
  useDesktopDrop(container, {
    hover: (event) => setHot(event !== null && project !== undefined),
    drop: (event) => {
      if (project === undefined) return
      void (async () => {
        const receipt = await dragDrop.claim(event.paths)
        for (const path of event.paths) {
          try { await dragDrop.open(project as ProjectId, receipt, path) }
          catch (error) { notifyFailure(error, { project }) }
        }
        await useWorkspace.getState().hydrate()
      })().catch((error: unknown) => notifyFailure(error, { project }))
    },
  })
  return hot
}
