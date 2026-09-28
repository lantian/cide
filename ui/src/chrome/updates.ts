/**
 * The self-update notices: *a newer cide is out*, the download, and *restart to finish*.
 *
 * The check, the download and the install are all Rust (`cide-app`'s `updater` module); this
 * module only turns what it reports into toasts in `notices.ts`, app-wide (`project: null`) so a
 * project switch does not hide them. Three answers to the first toast, and each is a different
 * promise:
 *
 * - **Update** downloads and installs, then asks about the restart instead of doing it — a
 *   restart stops every session and run this cide hosts, and that is the user's call.
 * - **Skip this version** is stored (`settings.update.skippedVersion`), and silences that
 *   version only. The next release is announced as usual.
 * - The toast's own ✕ is *not now*: nothing is stored, and the next start asks again.
 *
 * A build that cannot replace itself (the tarball, a `.deb`, the macOS disk image) gets
 * **Open release page** instead of **Update** — `UpdateInfo.inPlace`, decided in Rust.
 *
 * Only shell windows draw these. A detached pane's window is a view of one pane, and a second
 * copy of an app-wide question in it would be answered twice.
 */
import { useEffect } from 'react'
import {
  app as appApi,
  settings as settingsApi,
  update,
  type UpdateInfo,
  type UpdateProgress,
} from '@/ipc/client'
import { useWorkspace } from '@/store/workspace'
import { atRisk } from './closeConfirmModel'
import { requestCloseConfirm } from './closeConfirmStore'
import { describe, dismiss, getSnapshot, notify, notifyFailure } from './notices'

/** The offer's headline. Also its identity: `notices.ts` de-duplicates by text, which is what
 *  makes the start-up event and the mount-time `status()` pull land as one toast, not two. */
const offerText = (version: string): string => `cide ${version} is available`
const DOWNLOADING = 'Downloading the cide update…'

function dismissByText(text: string): void {
  for (const notice of getSnapshot()) {
    if (notice.text === text) dismiss(notice.id)
  }
}

function openReleasePage(info: UpdateInfo): void {
  appApi.openUrl(info.releaseUrl).catch((reason: unknown) => notifyFailure(reason, { project: null }))
}

/** Show the offer. Safe to call twice for one version — see `offerText`. */
export function offerUpdate(info: UpdateInfo): void {
  notify(offerText(info.version), {
    kind: 'ok',
    project: null,
    hint: info.inPlace
      ? `You have ${info.current}.`
      : `You have ${info.current}. This copy cannot update itself — download the new one from the release page.`,
    detail: info.notes ?? undefined,
    actions: [
      info.inPlace
        ? { label: 'Update', run: () => void installUpdate(info) }
        : { label: 'Open release page', run: () => openReleasePage(info) },
      { label: 'Skip this version', run: () => void skipVersion(info.version) },
    ],
  })
}

async function skipVersion(version: string): Promise<void> {
  const current = useWorkspace.getState().boot?.workspace.settings.update
  try {
    await settingsApi.set({
      update: { checkOnStart: current?.checkOnStart ?? true, skippedVersion: version },
    })
  } catch (reason) {
    notifyFailure(reason, { project: null })
  }
}

async function installUpdate(info: UpdateInfo): Promise<void> {
  showProgress(null)
  try {
    await update.install()
    // Nothing else to do here: `cide://update-ready` reaches every window, this one included,
    // and `onReady` below draws the restart question from it.
  } catch (reason) {
    dismissByText(DOWNLOADING)
    notify(`cide ${info.version} could not be installed`, {
      kind: 'error',
      project: null,
      hint: 'Nothing was changed. You can download it from the release page.',
      detail: describe(reason),
      actions: [{ label: 'Open release page', run: () => openReleasePage(info) }],
    })
  }
}

function mib(kib: number): string {
  return (kib / 1024).toFixed(kib < 10 * 1024 ? 1 : 0)
}

function showProgress(progress: UpdateProgress | null): void {
  let hint = 'Starting…'
  if (progress !== null) {
    const { receivedKib, totalKib } = progress
    hint =
      totalKib !== null && totalKib > 0
        ? `${Math.floor((receivedKib * 100) / totalKib)}% — ${mib(receivedKib)} of ${mib(totalKib)} MB`
        : `${mib(receivedKib)} MB`
  }
  // `actions: []` is how the same toast is *updated* rather than left as it was: `admit` keeps a
  // same-text notice unchanged only when neither side has actions and the detail is equal, and
  // the progress lives in `hint`, which that comparison does not look at. An empty list draws
  // no button row (`Failures.tsx` renders actions only when there are some).
  notify(DOWNLOADING, { kind: 'ok', project: null, hint, actions: [] })
}

function offerRestart(version: string): void {
  dismissByText(DOWNLOADING)
  notify(`cide ${version} is installed`, {
    kind: 'ok',
    project: null,
    hint: 'Restart to run it. Otherwise it runs from the next start.',
    actions: [{ label: 'Restart now', run: () => void restartNow() }],
  })
}

/**
 * Restart, asking first exactly when a quit would: unsaved edits, or a session mid-turn. The same
 * dialog and the same answer (`app.quitRequested`), so a restart is never the one road out of cide
 * that skips the question.
 */
async function restartNow(): Promise<void> {
  try {
    const decision = await appApi.quitRequested()
    const risk = { unsaved: decision.unsaved, sessions: decision.blocking }
    if (atRisk(risk)) {
      requestCloseConfirm({
        scope: 'app',
        unsaved: risk.unsaved,
        sessions: risk.sessions,
        proceed: () => update.restart(),
      })
      return
    }
    await update.restart()
  } catch (reason) {
    notifyFailure(reason, { project: null })
  }
}

/**
 * *Check for updates* — the command, the About card's button, Settings' Version row and the
 * macOS menu bar's item. Always answers.
 */
export async function checkForUpdates(): Promise<void> {
  const answer = await update.check()
  switch (answer.kind) {
    case 'available':
      offerUpdate(answer.update)
      return
    case 'upToDate':
      notify(`cide ${answer.current} is the latest version`, { kind: 'ok', project: null })
      return
    case 'unavailable':
      notify('Could not check for updates', { kind: 'warn', project: null, hint: answer.reason })
      return
  }
}

/**
 * Subscribe this window to the update events, and pull what the start-up check already found.
 * `enabled` is false outside shell windows.
 */
export function useUpdateNotices(enabled: boolean): void {
  useEffect(() => {
    if (!enabled) return
    const unlisteners: (() => void)[] = []
    let gone = false
    const keep = (fn: () => void): void => {
      if (gone) fn()
      else unlisteners.push(fn)
    }
    void update.onAvailable(offerUpdate).then(keep)
    void update.onProgress(showProgress).then(keep)
    void update.onReady(offerRestart).then(keep)
    // The macOS menu bar's *Check for Updates…* (`cide-app`'s `app_menu`). Rust picked this
    // window; the check and its answer are the same as the command's.
    void update
      .onCheckRequested(() => {
        checkForUpdates().catch((reason: unknown) => notifyFailure(reason, { project: null }))
      })
      .then(keep)
    // The pull covers a window whose listener was not up when the start-up event went out. A
    // failure here is a build without the command, which has nothing to announce anyway.
    void update
      .status()
      .then((info) => {
        if (!gone && info !== null) offerUpdate(info)
      })
      .catch(() => undefined)
    return () => {
      gone = true
      for (const fn of unlisteners) fn()
    }
  }, [enabled])

  // A skip answered in *another* window reaches this one as a settings change; the offer on
  // screen here is for the version just declined and must go too.
  const skipped = useWorkspace((s) => s.boot?.workspace.settings.update.skippedVersion ?? null)
  useEffect(() => {
    if (skipped !== null) dismissByText(offerText(skipped))
  }, [skipped])
}
