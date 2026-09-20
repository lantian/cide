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
import { useEffect, useRef, useState } from 'react'
import { OverlayCard } from '@/overlays/ModalShell'
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

  return (
    <OverlayCard label={label} onDismiss={close}>
      <div className={styles.dialog} onKeyDown={onKeyDown} data-audit="fileProperties">
        <Head
          props={props}
          path={path}
          roots={roots}
          failure={failure}
        />

        <div className={styles.body}>
          {failure !== null && (
            <p className={styles.failure} data-audit="filePropertiesFailure">
              {failure}
            </p>
          )}

          {props !== null && (
            <>
              <Section>
                <Row
                  label="Size"
                  value={
                    props.kind === 'dir'
                      ? dirSizeLine(dir)
                      : formatSize(props.len)
                  }
                  pending={props.kind === 'dir' && dir === null ? 'Measuring…' : null}
                  note={sizeNote(hasDirtyBuffer(path))}
                />

                {props.kind === 'dir' && (
                  <Row
                    label="Entries"
                    value={entriesLine(dir)}
                    pending={dir === null ? 'Counting…' : null}
                  />
                )}

                {props.kind !== 'dir' && (
                  <Row label="Lines" value={textLine(props as PropertiesLike)} />
                )}

                {props.symlinkTarget !== undefined && (
                  <Row label="Target" value={props.symlinkTarget} mono />
                )}
              </Section>

              <Section>
                <Row label="Modified" value={formatTime(props.modifiedUnixMs)} />
                <Row label="Changed" value={formatTime(props.changedUnixMs)} />
                {props.createdUnixMs !== undefined && (
                  <Row label="Created" value={formatTime(props.createdUnixMs)} />
                )}
              </Section>

              <Section>
                {props.modeString !== undefined && (
                  <Row label="Permissions" value={props.modeString} mono />
                )}
                <Row label="Owner" value={formatOwner(props.owner)} />
                {props.readonly && (
                  <Row label="Writable" value="No — the mode bits carry no write permission" />
                )}
              </Section>
            </>
          )}

          {/*
            * The Git block. Absent entirely when the path is in no repository — an empty one
            * would be a heading making claims about a file git has never heard of.
            */}
          {gitAsked && gitLike !== null && (
            <section className={styles.git} data-audit="filePropertiesGit">
              <h3 className={styles.sectionTitle}>Git</h3>
              <Row
                label="Status"
                value={statusLabel(statusAt(status.statuses, path), status.truncated)}
              />
              <Row label="Tracked" value={trackedSince(gitLike)} />

              {shown.length > 0 && (
                <>
                  <div className={styles.recentHead}>
                    <span className={styles.recentLabel}>Recent</span>
                    {onOpenHistory !== undefined && (
                      <button
                        type="button"
                        className={styles.link}
                        data-audit="filePropertiesHistory"
                        onClick={() => {
                          onOpenHistory(gitLike.repo, gitLike.relPath)
                          close()
                        }}
                      >
                        Open full history
                      </button>
                    )}
                  </div>
                  <ul className={styles.commits}>
                    {shown.map((row) => (
                      <li key={row.oid} className={styles.commit} title={row.summary}>
                        <span className={styles.oid}>{row.shortOid}</span>
                        <span className={styles.commitWhen}>{when(row.authored, now)}</span>
                        <span className={styles.commitAuthor}>{row.author}</span>
                        <span className={styles.commitSummary}>{row.summary}</span>
                      </li>
                    ))}
                  </ul>
                  {footnote !== null && <p className={styles.more}>{footnote}</p>}
                </>
              )}
            </section>
          )}

          {!gitAsked && props !== null && (
            <p className={styles.waiting} data-audit="filePropertiesGitPending">
              Reading history…
            </p>
          )}
        </div>

        <div className={styles.footer}>
          <button
            ref={closeButton}
            type="button"
            className={styles.close}
            onClick={close}
            data-audit="filePropertiesClose"
          >
            Close
          </button>
        </div>
      </div>
    </OverlayCard>
  )
}

/** The name, the kind and the path, with a copy button for the path. */
function Head({
  props,
  path,
  roots,
  failure,
}: {
  props: FilePropertiesDto | null
  path: string
  roots: readonly string[]
  failure: string | null
}) {
  const [copied, setCopied] = useState(false)
  const shown = displayPath(path, roots)

  // The heading is available before the stat answers, because the path is what the gesture
  // carried. A card that showed nothing at all for its first frame reads as a card that failed.
  const name = props?.name ?? path.slice(path.lastIndexOf('/') + 1)
  const kind = props === null ? null : kindLabel(props as PropertiesLike)
  const language = props?.kind === 'file' ? typeLabel(path) : null

  return (
    <div className={styles.head}>
      <div className={styles.titleRow}>
        <h2 className={styles.title} data-audit="filePropertiesTitle">
          {name}
        </h2>
        {(language ?? kind) !== null && (
          <span className={styles.kind}>{language ?? kind}</span>
        )}
      </div>
      <div className={styles.pathRow}>
        <span className={styles.path} title={path}>
          {failure === null ? shown : path}
        </span>
        <button
          type="button"
          className={styles.copy}
          data-audit="filePropertiesCopy"
          onClick={() => {
            // The absolute path, never the displayed relative one: a path copied out of this
            // dialog is pasted into a terminal or another tool, where a project-relative one
            // resolves against the wrong directory or not at all.
            void navigator.clipboard.writeText(path).then(
              () => setCopied(true),
              () => setCopied(false),
            )
          }}
        >
          {copied ? 'Copied' : 'Copy path'}
        </button>
      </div>
    </div>
  )
}

/**
 * What the chip beside the name says for a file: the image format, else the language.
 *
 * `imageKindFor` decides by **name**, which is the same rule `PaneBody` uses to pick a viewer, so
 * the chip and the pane agree about what a `.png` is. The *bytes* may disagree — `cide_core`
 * sniffs them for the image pane — and that disagreement is the image pane's to report, not a
 * properties card's.
 */
function typeLabel(path: string): string {
  const image = imageKindFor(path)
  return image === null ? languageName(path) : imageLabel(image)
}

/** A group of rows with a rule under it. */
function Section({ children }: { children: React.ReactNode }) {
  return <section className={styles.section}>{children}</section>
}

/**
 * One `label: value` line.
 *
 * Renders **nothing** when the value is `null` and there is no pending sentence — that is how an
 * inapplicable fact (a `Created` row on a filesystem with no birth time) disappears rather than
 * showing a label with a blank beside it. A fact that is merely *unknown* never reaches here as
 * `null`: `filePropertiesModel.ts` turns those into `Not recorded`, which is a sentence.
 */
function Row({
  label,
  value,
  pending = null,
  note = null,
  mono = false,
}: {
  label: string
  value: string | null
  pending?: string | null
  note?: string | null
  mono?: boolean
}) {
  const shown = pending ?? value
  if (shown === null) return null
  return (
    <div className={styles.row} data-audit="filePropertiesRow">
      <span className={styles.rowLabel}>{label}</span>
      <span className={`${styles.rowValue} ${mono ? styles.mono : ''}`}>
        {shown}
        {note !== null && <span className={styles.note}>{note}</span>}
      </span>
    </div>
  )
}
