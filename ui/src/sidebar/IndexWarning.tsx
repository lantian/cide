import { useState } from 'react'
import { settings, type FsStatus, type ProjectId } from '@/ipc/client'
import { notifyFailure } from '@/chrome/notices'
import { Banner } from '@/kit/components/Feedback'
import { Button } from '@/kit/components/Button'
import { IndexDiagnosticsDialog } from './IndexDiagnosticsDialog'
import { indexLimitLabel } from './indexDiagnostics'
import styles from './IndexDiagnosticsDialog.module.css'

/** The host keys this by project so switching projects also dismisses its dialog. */
export function IndexWarning({ project, status }: { project: ProjectId; status: FsStatus }) {
  const [details, setDetails] = useState(false)
  const openSettings = () => {
    setDetails(false)
    void settings.openTab(project, 'files').catch((reason: unknown) => notifyFailure(reason, { project }))
  }
  return <>
    {status.indexLimit && <Banner tone="warn" action={<div className={styles.actions}>
      <Button size="sm" variant="quiet" onClick={() => setDetails(true)}>Details</Button>
      <Button size="sm" variant="quiet" onClick={openSettings}>Settings</Button>
    </div>}>
      Index incomplete: {indexLimitLabel(status.indexLimit).toLowerCase()}
    </Banner>}
    {details && <IndexDiagnosticsDialog project={project} generation={status.generation} indexing={status.indexing}
      onDismiss={() => setDetails(false)} onSettings={openSettings} />}
  </>
}
