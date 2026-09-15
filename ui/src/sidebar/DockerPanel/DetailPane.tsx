/**
 * The right half of the Docker panel: what the selected thing is. (M47)
 *
 * Pure — no store, no IPC, no clock — so `check:docker-render` can drive it. The detail arrives
 * as a prop and the host fetches it.
 *
 * # Why a pane and not a tab
 *
 * Both exist, deliberately. This answers *what is this* while the list is still on screen, which
 * is the question a row-click asks; `TabKind::Docker` answers *everything about this* in a
 * full-width editor with search and folding, which is the question a double-click asks. A panel
 * that only had the tab would make "which port is it on" a tab-open and a tab-close.
 *
 * # The editing, and what it really does
 *
 * Ports are editable and nothing else is — not because the rest is harder, but because the rest
 * has the same cost and less value. **There is no endpoint that changes a running container.**
 * Applying an edit removes the container and creates another from the same image under the same
 * name, which is what every tool that appears to edit one does. `DetailPaneProps.onApplyPorts`
 * carries that; `cide_docker::detail::recreate` performs it.
 */
import { useEffect, useState } from 'react'

import { Icon } from '@/icons/Icon'
import { sizeText } from './model'

import styles from './DetailPane.module.css'
import {
  parsePorts,
  portsText,
  type Detail,
  type DetailPort,
  imageSizeRows,
  type DetailRef,
  type VolumeSize,
  volumeSizeText,
} from './detailModel'

export interface DetailPaneProps {
  /** `null` while nothing is selected — the pane draws its own empty state. */
  readonly detail: Detail | null
  /** True while a gesture is in flight; every control is disabled. */
  readonly busy?: boolean
  /**
   * Replace the container with one published on these ports.
   *
   * Absent for anything that is not a container, and for a read-only placement. The word is
   * `Apply` on the button and `recreate` in every name behind it, because that is what happens.
   */
  readonly onApplyPorts?: ((ports: readonly DetailPort[]) => void) | undefined
  /**
   * Follow a reference to another object — the image this container runs, a volume it mounts, a
   * network it is on, a container attached to this network. (M50)
   *
   * Absent when nothing can be followed, which is the shape rather than a `disabled` flag:
   * `DetailPane` draws a link only where this is given *and* the row carries a ref, so a
   * reference that resolves to no row is plain text and not a dead control.
   */
  readonly onFollow?: ((ref: DetailRef) => void) | undefined
  /**
   * A volume's size, once somebody has paid for it. (M57)
   *
   * Absent for every other kind. It is a prop rather than something this pane fetches, for the
   * reason every other value here is one: the pane is pure, and `check:docker-render` drives it
   * by passing values rather than by mocking a daemon.
   */
  readonly volumeSize?: VolumeSize | undefined
  /**
   * What the **list** says this image occupies, from the board. (M58)
   *
   * Passed in rather than read here, because the pane sees one image's inspect and the answer
   * needs both measurements — see `imageSizeRows`. `undefined` when the board has no row for it,
   * which is an ordinary outcome for an image removed between the click and the answer.
   */
  readonly imageOnDisk?: number | undefined
  /** Open the full `inspect` document in a tab. */
  readonly onOpenRaw?: (() => void) | undefined
}

export function DetailPane({
  detail,
  busy = false,
  onApplyPorts,
  onOpenRaw,
  onFollow,
  volumeSize,
  imageOnDisk,
}: DetailPaneProps) {
  if (detail === null) {
    return (
      <div className={styles.pane} data-audit="dockerDetail">
        <div className={styles.empty}>Select a row to see what it is.</div>
      </div>
    )
  }
  if (detail.kind === 'missing') {
    return (
      <div className={styles.pane} data-audit="dockerDetail">
        {/* The ordinary outcome of clicking a row for something that has just been removed. */}
        <div className={styles.empty}>{detail.reason}</div>
      </div>
    )
  }

  return (
    <div className={styles.pane} data-audit="dockerDetail">
      <div className={styles.header}>
        <span className={styles.title} title={detail.title}>
          {detail.title}
        </span>
        {onOpenRaw !== undefined && (
          <button
            type="button"
            className={styles.headerAction}
            onClick={onOpenRaw}
            title="Open the full inspect document"
            aria-label="Open the full inspect document"
          >
            <Icon name="file-code" size={1} />
          </button>
        )}
      </div>

      <div className={styles.body}>
        {detail.facts.length > 0 && (
          <Section label={detail.kind === 'container' ? 'Container' : detail.kind}>
            {detail.facts.map((fact) => (
              <Row
                key={fact.name}
                name={fact.name}
                value={fact.value}
                hint={fact.hint}
                ref={fact.ref}
                onFollow={onFollow}
              />
            ))}
          </Section>
        )}

        {detail.kind === 'container' && (
          <PortsSection ports={detail.ports} busy={busy} onApply={onApplyPorts} />
        )}

        {/*
          * A volume's size, which the wire does not carry. (M57)
          *
          * Its own row under the facts rather than one of them, because it arrives separately and
          * later: `GET /volumes` has no size and `/system/df` walks the filesystem. Drawn in all
          * three states — see `volumeSizeText` — so a volume nobody measured never reads as empty.
          */}
        {/*
          * An image's size, which is **two** numbers whenever the daemon means two things by it.
          * (M58) `imageSizeRows` carries the measurement that settled why.
          */}
        {detail.kind === 'image' && (
          <Section label="Disk">
            {imageSizeRows(imageOnDisk, detail.contentSize, (n) => sizeText(BigInt(n))).map(
              (row) => (
                <Row key={row.name} name={row.name} value={row.value} hint={row.hint} />
              ),
            )}
          </Section>
        )}

        {detail.kind === 'volume' && volumeSize !== undefined && (
          <Section label="Disk">
            {/*
              * The formatter is passed in rather than duplicated: `detailModel.ts` is import-free
              * so `check:docker` can compile it standalone, and `sizeText` lives beside the rows
              * that already draw byte counts. `BigInt` because that is the shape it takes — the
              * wire's `i64` — and a volume large enough to lose precision is nine petabytes.
              */}
            <Row name="Size" value={volumeSizeText(volumeSize, (n) => sizeText(BigInt(n)))} />
          </Section>
        )}

        {detail.groups.map((group) => (
          <Section key={group.label} label={group.label}>
            {group.rows.length === 0 ? (
              <div className={styles.none}>none</div>
            ) : (
              group.rows.map((row, index) => (
                <Row
                  key={`${row.name}:${index}`}
                  name={row.name}
                  value={row.value}
                  hint={row.hint}
                  ref={row.ref}
                  onFollow={onFollow}
                />
              ))
            )}
          </Section>
        ))}
      </div>
    </div>
  )
}

function Section({ label, children }: { readonly label: string; readonly children: React.ReactNode }) {
  return (
    <section className={styles.section}>
      <h3 className={styles.sectionLabel}>{label}</h3>
      {children}
    </section>
  )
}

function Row({
  name,
  value,
  hint,
  ref: target,
  onFollow,
}: {
  readonly name: string
  readonly value: string
  /** A sentence for the tooltip, where the label is a term of art. See `DetailRow.hint`. */
  readonly hint?: string | undefined
  readonly ref?: DetailRef | undefined
  readonly onFollow?: ((ref: DetailRef) => void) | undefined
}) {
  /*
   * A link, only where there is somewhere to go.
   *
   * `onFollow` is absent when the host cannot resolve anything, and `ref` is absent on the
   * overwhelming majority of rows — an environment variable, a label, a port. Between them they
   * decide whether this row is a button at all: a row that *looked* like a link and selected
   * nothing is the listed-and-inert control `cide_core::commands` makes unrepresentable one layer
   * down, and here it would be worse, because the thing it fails to find is a thing the user can
   * see is missing.
   *
   * The **host** decides too, and has to: whether `nginx:latest` is a row in the left column is a
   * question about the board, which this component has never seen. `DockerPanelHost` passes
   * `onFollow` only for refs `model.resolveRef` answers — so a bind mount, a removed image and a
   * container that has gone since the snapshot all render as the plain text they are.
   */
  const linked = target !== undefined && onFollow !== undefined
  return (
    <div className={styles.row}>
      <span className={styles.rowName} title={hint ?? name}>
        {name}
      </span>
      {linked ? (
        <button
          type="button"
          className={`${styles.rowValue} ${styles.rowLink}`}
          title={`Show ${value === '' ? name : value} in the list`}
          onClick={() => onFollow(target)}
        >
          {/*
            * A network and an attached container carry their name in `name` and nothing in
            * `value`; an image carries it in `value`. Drawing whichever is there keeps one Row
            * component instead of two that differ by which field they read.
            */}
          {value === '' ? name : value}
        </button>
      ) : (
        /*
         * Selectable, and that is the point of the pane: the answer to "which port" or "what is
         * this mounted from" is usually about to be pasted somewhere.
         */
        <span className={styles.rowValue} title={value}>
          {value}
        </span>
      )}
    </div>
  )
}

/**
 * The one editable section.
 *
 * A free-text field in `docker run` syntax — `8080:80`, `127.0.0.1:5433:5432/tcp` — rather than a
 * grid of number inputs. Two reasons: it is the spelling every user of Docker already knows, and
 * a grid would need add/remove rows, reordering and per-field validation to express what one line
 * of text expresses exactly.
 */
function PortsSection({
  ports,
  busy,
  onApply,
}: {
  readonly ports: readonly DetailPort[]
  readonly busy: boolean
  readonly onApply?: ((ports: readonly DetailPort[]) => void) | undefined
}) {
  const current = portsText(ports)
  const [draft, setDraft] = useState(current)
  // Re-seeded whenever the container's own ports change — a refresh, or the recreate landing.
  // Without this the field would keep showing what the user typed against a container that no
  // longer exists.
  useEffect(() => setDraft(current), [current])

  const parsed = parsePorts(draft)
  const dirty = draft.trim() !== current.trim()

  if (onApply === undefined) {
    return (
      <Section label="Ports">
        {ports.length === 0 ? <div className={styles.none}>none published</div> : null}
        {ports.map((port) => (
          <Row key={`${port.private}/${port.protocol}:${port.public ?? ''}`} name={`${port.private}/${port.protocol}`} value={port.public === undefined ? 'not published' : String(port.public)} />
        ))}
      </Section>
    )
  }

  return (
    <Section label="Ports">
      <textarea
        className={styles.ports}
        value={draft}
        spellCheck={false}
        rows={Math.max(2, draft.split('\n').length)}
        disabled={busy}
        aria-label="Published ports"
        placeholder="8080:80&#10;127.0.0.1:5433:5432"
        onChange={(event) => setDraft(event.target.value)}
      />
      {parsed.problem !== null && <div className={styles.problem}>{parsed.problem}</div>}
      <div className={styles.applyRow}>
        {/*
          * "Apply & recreate", not "Apply". No dialog — but the button says what pressing it
          * does, because a container being destroyed and rebuilt is not what "apply" means
          * anywhere else in this application.
          */}
        <button
          type="button"
          className={styles.apply}
          disabled={busy || !dirty || parsed.problem !== null}
          onClick={() => onApply(parsed.ports)}
        >
          Apply &amp; recreate
        </button>
        {dirty && parsed.problem === null && (
          <span className={styles.applyNote}>
            replaces this container from the same image
          </span>
        )}
      </div>
    </Section>
  )
}
