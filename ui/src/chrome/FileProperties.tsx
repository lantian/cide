/**
 * The file properties card. (M70)
 *
 * > *"Need a file properties modal window that will show OS stats and git history if applicable
 * > and other useful stuff"*
 *
 * `filePropertiesModel.ts` is the decisions and every sentence; this is the markup and the three
 * fetches. `PasteConfirm.tsx` is the house precedent and this follows it: the same `OverlayCard`,
 * the same Escape-on-the-card rule, the same division of labour.
 *
 * # Three calls, not one, and what each is allowed to block
 *
 * The card opens on `files.properties` alone — one `symlink_metadata` and at most one bounded
 * byte scan. The directory walk and the git walk land afterwards into rows that say they are
 * working. A single call returning all three would make every card wait on the slowest half of
 * itself, and for a right-click on `target/` that is a card which appears seconds later or not
 * at all.
 *
 * So there are three pending states and each renders as a **sentence**, never as a blank row:
 * an empty value beside a label reads as a broken dialog, which is the same rule
 * `TaskDetailPending` follows for the same reason.
 *
 * # What is deliberately not here
 *
 * **A Blame button.** `git.blame` toggles a gutter in an open editor, so from a card about a
 * file that is not open it would have to open the file first — a surprising side effect from a
 * dialog whose whole job is to describe rather than to do. *Open full history* stays because the
 * tab it opens is itself a description, and because the card has already resolved the repository
 * and the repo-relative path that command would otherwise have to look up again.
 */
import { type ReactNode, useEffect, useRef, useState } from 'react'
import { Modal } from '@/overlays/ModalShell'
import { Button } from '@/kit/components/Button'
import { Note, Spinner } from '@/kit/components/Feedback'
import { Dialog } from '@/kit/components/Overlay'
import { Badge } from '@/kit/components/Status'
import { Section, Summary } from '@/kit/components/Surface'
import { file as fileApi, history as historyApi } from '@/ipc/client'
import type { DirSummary, FileProperties as FilePropertiesDto, FilePropertiesGit } from '@/ipc/generated'
import { errorText } from '@/ipc/errorText'
import { languageName } from '@/editor/languages'
import { imageKindFor, imageLabel } from '@/panes/imageKinds'
import { hasDirtyBuffer } from '@/editor/blameStore'
import { useGitStatus } from '@/sidebar/gitStatusStore'
import { statusAt } from '@/sidebar/treeStatus'
import { useWorkspace } from '@/store/workspace'
import { useShallow } from 'zustand/react/shallow'
import { when } from '@/gitlog/logModel'
import { useFileProperties } from './filePropertiesStore'
import {
  cardLabel,
  displayPath,
  dirSizeLine,
  entriesLine,
  formatOwner,
  formatSize,
  formatTime,
  kindLabel,
  moreLine,
  sizeNote,
  statusLabel,
  textLine,
  trackedSince,
  type GitLike,
  type PropertiesLike,
} from './filePropertiesModel'
import styles from './FileProperties.module.css'

/** One frozen empty, so the no-project fallback is a stable reference for `useShallow`. */
const NO_ROOTS: readonly string[] = []

/**
 * The card, or `null` when none is open.
 *
 * Mounted unconditionally in **both** `App.tsx` branches — the detached-pane window and the
 * shell — because `file.properties` carries no `shellWindow` clause and is therefore live in a
 * detached window. Mounted in one branch only, the keystroke there would ask nobody at all, with
 * nothing on screen and nothing logged.
 */
export function FileProperties({
  onOpenHistory,
}: {
  /**
   * Open the tool window's history tab for this path.
   *
   * Optional, and its absence is the whole guard: a detached-pane window has no tool window, so
   * `App.tsx` passes this only from the shell branch and the button is simply not drawn
   * elsewhere. The same shape as `FileTree`'s `onShowHistory`, and for the same reason — a
   * button that reports "this window has no tool window" after being pressed is a button that
   * should not have been offered.
   */
  onOpenHistory?: ((repo: string, relPath: string) => void) | undefined
}) {
  const pending = useFileProperties((s) => s.pending)
  const close = useFileProperties((s) => s.close)

  const [props, setProps] = useState<FilePropertiesDto | null>(null)
  const [dir, setDir] = useState<DirSummary | null>(null)
  const [git, setGit] = useState<FilePropertiesGit | null>(null)
  const [gitAsked, setGitAsked] = useState(false)
  const [failure, setFailure] = useState<string | null>(null)

  const closeButton = useRef<HTMLButtonElement>(null)

  const path = pending?.path ?? null
  const project = pending?.project ?? null

  // A selector returning a stable reference, per `check:selectors`: `status` is one object the
  // store replaces wholesale, never rebuilt here. Returning `Object.keys(...)` or a literal
  // would re-render for ever and end at *Maximum update depth exceeded*, which unmounts the root.
  const status = useGitStatus((s) => s.status)
  // `useShallow` over the mapped array, `FileTree.tsx`'s rule: the store hands back a fresh
  // `ProjectRoot[]` on every accepted mutation in any window, and a selector that returns one
  // would re-render on each — which `check:selectors` refuses outright, because the failure mode
  // is *Maximum update depth exceeded* and an unmounted root rather than a slow card.
  const roots = useWorkspace(
    useShallow((s) =>
      project === null
        ? NO_ROOTS
        : (s.boot?.workspace.projects[project]?.roots.map((r) => r.path) ?? NO_ROOTS),
    ),
  )

  // Every answer is re-fetched per path, and every one is discarded if the card moved on while
  // it was in flight. Without the generation guard a slow directory walk from the previous card
  // lands on the current one and reports another folder's size under this folder's name — the
  // same class `gitStatusStore`'s counter exists for.
  useEffect(() => {
    if (path === null) return
    let live = true
    setProps(null)
    setDir(null)
    setGit(null)
    setGitAsked(false)
    setFailure(null)

    void fileApi
      .properties(path)
      .then((answer) => {
        if (!live) return
        setProps(answer)
        // The directory walk is asked only once the kind is known, because it is the slow call
        // and a file must never pay for it.
        if (answer.kind === 'dir') {
          void fileApi
            .propertiesDir(path)
            .then((summary) => {
              if (live) setDir(summary)
            })
            .catch(() => {
              // A directory that cannot be walked keeps its other rows. The entries row stays
              // on its pending sentence, which is true: nothing counted it.
            })
        }
      })
      // `errorText`, never `String(e)` — a `CoreError` is a tagged enum and `String()` of it is
      // `[object Object]`, which is what the colour-scheme import showed for a year.
      .catch((e: unknown) => {
        if (live) setFailure(errorText(e))
      })

    if (project !== null) {
      void historyApi
        .pathProperties(project, path)
        .then((answer) => {
          if (!live) return
          setGit(answer)
          setGitAsked(true)
        })
        .catch(() => {
          // A git failure is not a card failure: the OS half is still correct and worth showing.
          // `gitAsked` without a `git` renders as "no repository", which is what a failed locate
          // effectively means for this card's purposes.
          if (live) setGitAsked(true)
        })
    }

    return () => {
      live = false
    }
  }, [path, project])

  // Focus the one control that always exists. A card with nothing focused swallows Escape in
  // WebKitGTK when the click that opened it landed on a menu that has since gone.
  useEffect(() => {
    if (pending !== null) closeButton.current?.focus()
  }, [pending])

  if (pending === null || path === null) return null

  // Escape closes, caught on the card rather than on `document`: a window listener would also
  // answer for the terminal underneath and for any other overlay that happens to be open.
  const onKeyDown = (ev: React.KeyboardEvent) => {
    if (ev.key === 'Escape') {
      ev.stopPropagation()
      close()
    }
  }

  const label = props === null ? 'Properties' : cardLabel(props as PropertiesLike)
  const gitLike: GitLike | null = git as GitLike | null
  const shown = git?.recent ?? []
  const footnote = moreLine(gitLike, shown.length)
  const now = Date.now()

  const name = props?.name ?? path.slice(path.lastIndexOf('/') + 1)
  const kind = props === null ? null : kindLabel(props as PropertiesLike)
  const language = props?.kind === 'file' ? typeLabel(path) : null
  const kindText = language ?? kind

  return (
    <Modal onDismiss={close}>
      <Dialog
        title={props === null ? name : label}
        titleAside={
          kindText !== null && (
            <Badge tone="neutral" soft squared>
              {kindText}
            </Badge>
          )
        }
        head={<PathLine path={path} roots={roots} failure={failure} />}
        onKeyDown={onKeyDown}
        data-audit="fileProperties"
        actions={
          <Button
            ref={closeButton}
            variant="primary"
            onClick={close}
            data-audit="filePropertiesClose"
          >
            Close
          </Button>
        }
      >
        <div className={styles.body}>
          {failure !== null && (
            <span data-audit="filePropertiesFailure">
              <Note tone="bad">{failure}</Note>
            </span>
          )}

          {props !== null && (
            <>
              <Summary
                rows={compact([
                  row(
                    'Size',
                    props.kind === 'dir' && dir === null
                      ? 'Measuring…'
                      : props.kind === 'dir'
                        ? dirSizeLine(dir)
                        : formatSize(props.len),
                    sizeNote(hasDirtyBuffer(path)),
                  ),
                  props.kind === 'dir'
                    ? row('Entries', dir === null ? 'Counting…' : entriesLine(dir))
                    : row('Lines', textLine(props as PropertiesLike)),
                  props.symlinkTarget !== undefined
                    ? row('Target', props.symlinkTarget, null, true)
                    : null,
                ])}
              />
              <Summary
                rows={compact([
                  row('Modified', formatTime(props.modifiedUnixMs)),
                  row('Changed', formatTime(props.changedUnixMs)),
                  props.createdUnixMs !== undefined
                    ? row('Created', formatTime(props.createdUnixMs))
                    : null,
                ])}
              />
              <Summary
                rows={compact([
                  props.modeString !== undefined
                    ? row('Permissions', props.modeString, null, true)
                    : null,
                  row('Owner', formatOwner(props.owner)),
                  props.readonly
                    ? row('Writable', 'No — the mode bits carry no write permission')
                    : null,
                ])}
              />
            </>
          )}

          {/*
            * The Git block. Absent entirely when the path is in no repository — an empty one
            * would be a heading making claims about a file git has never heard of.
            */}
          {gitAsked && gitLike !== null && (
            <div data-audit="filePropertiesGit">
              <Section
                caption="Git"
                aside={
                  shown.length > 0 &&
                  onOpenHistory !== undefined && (
                    <Button
                      variant="link"
                      size="sm"
                      data-audit="filePropertiesHistory"
                      onClick={() => {
                        onOpenHistory(gitLike.repo, gitLike.relPath)
                        close()
                      }}
                    >
                      Open full history
                    </Button>
                  )
                }
              >
                <Summary
                  rows={compact([
                    row(
                      'Status',
                      statusLabel(statusAt(status.statuses, path), status.truncated),
                    ),
                    row('Tracked', trackedSince(gitLike)),
                  ])}
                />
                {shown.length > 0 && (
                  <ul className={styles.commits} aria-label="Recent commits">
                    {shown.map((c) => (
                      <li key={c.oid} className={styles.commit} title={c.summary}>
                        <span className={styles.oid}>{c.shortOid}</span>
                        <span className={styles.commitWhen}>{when(c.authored, now)}</span>
                        <span className={styles.commitAuthor}>{c.author}</span>
                        <span className={styles.commitSummary}>{c.summary}</span>
                      </li>
                    ))}
                  </ul>
                )}
                {footnote !== null && <p className={styles.more}>{footnote}</p>}
              </Section>
            </div>
          )}

          {!gitAsked && props !== null && (
            <span className={styles.waiting} data-audit="filePropertiesGitPending">
              <Spinner label="Reading history…" />
            </span>
          )}
        </div>
      </Dialog>
    </Modal>
  )
}

/** One key → value line; `null` when there is nothing to say, which `compact` drops. */
function row(
  label: string,
  value: string | null,
  note: string | null = null,
  mono = false,
): { label: string; value: ReactNode } | null {
  if (value === null) return null
  return {
    label,
    value: (
      <span className={mono ? styles.mono : undefined}>
        {value}
        {note !== null && <span className={styles.note}>{note}</span>}
      </span>
    ),
  }
}

function compact<T>(rows: ReadonlyArray<T | null>): T[] {
  return rows.filter((r): r is T => r !== null)
}

/**
 * The path under the title, and the one act on it. Shown relative to a project root when it
 * is under one — the full path is on hover and is what Copy copies, whatever is shown.
 */
function PathLine({
  path,
  roots,
  failure,
}: {
  path: string
  roots: readonly string[]
  failure: string | null
}) {
  const [copied, setCopied] = useState(false)
  const shown = displayPath(path, roots)
  return (
    <div className={styles.pathRow}>
      <span className={styles.path} title={path}>
        {failure === null ? shown : path}
      </span>
      <Button
        variant="link"
        size="sm"
        data-audit="filePropertiesCopy"
        onClick={() => {
          void navigator.clipboard.writeText(path).then(
            () => setCopied(true),
            () => setCopied(false),
          )
        }}
      >
        {copied ? 'Copied' : 'Copy path'}
      </Button>
    </div>
  )
}

function typeLabel(path: string): string {
  const image = imageKindFor(path)
  return image === null ? languageName(path) : imageLabel(image)
}

/** A group of rows with a rule under it. */
