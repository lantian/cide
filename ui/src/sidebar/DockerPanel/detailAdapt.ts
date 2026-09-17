/**
 * The wire's `DockerDetail` as the pane's `Detail`. (M47)
 *
 * The one seam allowed to import `@/ipc/generated` for this pane — `detailModel.ts` is compiled
 * standalone by `check:docker` and must import nothing. `adapt.ts`'s rule, for the same reason.
 *
 * This is also where the *shape* of the pane is decided: which facts come first, what a group is
 * called, what an empty value reads as. The model holds the rules, the wire holds the data, and
 * this is the only place that knows both.
 */
import type { DockerDetail, Pair } from '@/ipc/generated'

import type { Detail, DetailRow } from './detailModel'

const rows = (pairs: readonly Pair[]): readonly DetailRow[] =>
  pairs.map((pair) => ({ name: pair.name, value: pair.value }))

/** An empty value drawn as a word rather than as nothing, so a row is never a blank line. */
const orNone = (value: string): string => (value === '' ? '—' : value)

export function adaptDetail(wire: DockerDetail): Detail {
  switch (wire.kind) {
    case 'missing':
      return { kind: 'missing', reason: wire.reason }

    case 'container': {
      // `ts-rs` flattens a newtype variant, so the fields sit on the variant itself.
      const it = wire
      return {
        kind: 'container',
        id: it.id,
        title: it.name,
        facts: [
          // The one fact that names another object. `docker inspect` reports whichever tag
          // was used to start the container, which is why `resolveRef` matches an image by any
          // of its tags and then by id prefix.
          { name: 'Image', value: it.image, ref: { kind: 'image', text: it.image } },
          { name: 'State', value: orNone(it.state) },
          { name: 'Command', value: orNone(it.command) },
          { name: 'Restart', value: it.restartPolicy },
          // The short id, because that is what a person types into `docker` and the full one is
          // 64 characters of hex nobody reads.
          { name: 'Id', value: it.id.slice(0, 12) },
          ...(it.compose === undefined || it.compose === null
            ? []
            : [{ name: 'Compose', value: `${it.compose.project} / ${it.compose.service}` }]),
        ],
        ports: it.ports.map((port) => ({
          private: port.private,
          public: port.public ?? undefined,
          protocol: port.protocol,
          hostIp: port.hostIp ?? undefined,
        })),
        groups: [
          {
            label: 'Mounts',
            rows: it.mounts.map((mount) => ({
              name: mount.destination,
              value: `${mount.name === '' ? mount.source : mount.name}${mount.readOnly ? '  (read-only)' : ''}`,
              /*
               * Only a **named** volume is a link. An empty `name` is a bind mount — a path on the
               * host — and there is no volume row for it, not now and not after any refresh. So it
               * carries no ref at all rather than one `resolveRef` would answer `null` to: the
               * two render identically, and saying it here is saying it once instead of relying
               * on a lookup to fail in the right way.
               */
              ...(mount.name === '' ? {} : { ref: { kind: 'volume' as const, text: mount.name } }),
            })),
          },
          {
            label: 'Networks',
            rows: it.networks.map((name) => ({
              name,
              value: '',
              ref: { kind: 'network' as const, text: name },
            })),
          },
          { label: 'Environment', rows: rows(it.env) },
          { label: 'Labels', rows: rows(it.labels) },
        ],
      }
    }

    case 'image': {
      // `ts-rs` flattens a newtype variant, so the fields sit on the variant itself.
      const it = wire
      return {
        kind: 'image',
        id: it.id,
        title: it.tags[0] ?? '<none>',
        /*
         * The size is **not** a fact here any more. (M58)
         *
         * `GET /images/{id}/json`'s `Size` and `GET /images/json`'s are different measurements
         * under the containerd image store — content versus unpacked — and this adapter sees only
         * the first. Which rows to draw is a decision that needs both, so it is `imageSizeRows`'
         * to make and the pane's to render. See its note for the measurement that settled it.
         */
        contentSize: Number(it.size),
        facts: [
          { name: 'Platform', value: orNone([it.os, it.architecture].filter(Boolean).join('/')) },
          { name: 'Command', value: orNone(it.command) },
          { name: 'Id', value: it.id.replace('sha256:', '').slice(0, 12) },
        ],
        groups: [
          { label: 'Tags', rows: it.tags.map((tag) => ({ name: tag, value: '' })) },
          { label: 'Exposed', rows: it.exposed.map((port) => ({ name: port, value: '' })) },
          { label: 'Environment', rows: rows(it.env) },
          { label: 'Labels', rows: rows(it.labels) },
        ],
      }
    }

    case 'volume': {
      // `ts-rs` flattens a newtype variant, so the fields sit on the variant itself.
      const it = wire
      return {
        kind: 'volume',
        id: it.name,
        title: it.name,
        facts: [
          { name: 'Driver', value: it.driver },
          { name: 'Scope', value: orNone(it.scope) },
          // On a remote or virtualised daemon this is a path on *that* machine. Said in the row
          // name rather than left to be assumed.
          { name: 'Mountpoint', value: it.mountpoint },
        ],
        groups: [
          { label: 'Options', rows: rows(it.options) },
          { label: 'Labels', rows: rows(it.labels) },
        ],
      }
    }

    case 'network': {
      // `ts-rs` flattens a newtype variant, so the fields sit on the variant itself.
      const it = wire
      return {
        kind: 'network',
        id: it.id,
        title: it.name,
        facts: [
          { name: 'Driver', value: it.driver },
          { name: 'Scope', value: orNone(it.scope) },
          { name: 'Internal', value: it.internal ? 'yes' : 'no' },
          { name: 'Id', value: it.id.slice(0, 12) },
        ],
        groups: [
          { label: 'Subnets', rows: it.subnets.map((net) => ({ name: net, value: '' })) },
          {
            label: 'Containers',
            rows: it.attached.map((name) => ({
              name,
              value: '',
              ref: { kind: 'container' as const, text: name },
            })),
          },
          { label: 'Labels', rows: rows(it.labels) },
        ],
      }
    }
  }
}
