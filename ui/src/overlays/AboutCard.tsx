/**
 * About cide — the version this window is running, and the `claude` it will spawn.
 *
 * Reads `Bootstrap.capabilities` and nothing else. The version is `env!("CARGO_PKG_VERSION")`
 * on the Rust side (`cmd/app.rs::capabilities`), so this shows the same string `cide --version`
 * prints, spelled the same way (`cli.rs::version`) — a number pasted from here and a number
 * pasted from a terminal must never disagree in a bug thread. The `claude` line is the raw
 * `claude --version` output, which already names itself, with the wording Settings uses when
 * there is none.
 *
 * `OverlayCard` for the ground — the same scrim, width and radius as the palette that opened
 * it — and `CloseConfirm`'s two rules inside it: focus lands on the one button before the first
 * paint (`ModalShell` says why: a frame in which nothing here has focus is a frame in which the
 * next keystroke goes to the terminal underneath), and Escape is answered on a wrapper *inside*
 * the card rather than on `document`, which would also answer for that terminal.
 */
import { useLayoutEffect, useRef } from 'react'
import { Modal } from './ModalShell'
import { Button } from '@/kit/components/Button'
import { Dialog } from '@/kit/components/Overlay'
import { Summary } from '@/kit/components/Surface'
import { useWorkspace } from '@/store/workspace'
import styles from './AboutCard.module.css'

export function AboutCard({ onDismiss }: { onDismiss: () => void }) {
  const version = useWorkspace((s) => s.boot?.capabilities.version ?? null)
  const claude = useWorkspace((s) => s.boot?.capabilities.claudeVersion ?? null)
  const close = useRef<HTMLButtonElement>(null)

  useLayoutEffect(() => {
    close.current?.focus()
  }, [])

  const onKeyDown = (ev: React.KeyboardEvent) => {
    if (ev.key === 'Escape') {
      ev.stopPropagation()
      onDismiss()
    }
  }

  return (
    <Modal onDismiss={onDismiss}>
      <Dialog
        title="cide"
        width="narrow"
        onKeyDown={onKeyDown}
        data-audit="about"
        actions={
          <Button ref={close} variant="primary" onClick={onDismiss} data-audit="aboutClose">
            Close
          </Button>
        }
      >
        <Summary
          rows={[
            {
              label: 'Version',
              // `null` only before bootstrap resolves, and the host is mounted with `boot`, so
              // in practice the version is always there; the bare name is the honest fallback.
              value: (
                <span className={styles.mono} data-audit="aboutVersion">
                  {version === null ? 'cide' : `cide ${version}`}
                </span>
              ),
            },
            {
              label: 'Claude Code',
              value: (
                <span className={styles.mono} data-audit="aboutClaude">
                  {claude === null ? 'claude is not on PATH' : `claude ${claude}`}
                </span>
              ),
            },
          ]}
        />
      </Dialog>
    </Modal>
  )
}
