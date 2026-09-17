/**
 * The wire's `DockerBoard` as the panel's `Board`. (M41)
 *
 * The one seam allowed to import `@/ipc/generated`, `OpenSpecPanel/adapt`'s rule: `model.ts` is
 * compiled standalone by `check:docker` and must import nothing, so the translation lives here
 * and is deliberately **not** re-exported from `index.ts`.
 */
import type { DockerBoard } from '@/ipc/generated'
import { BOARD_UNKNOWN, type Board } from './model'

/**
 * `null` from the wire as `undefined`, which is what the types claim.
 *
 * `#[ts(optional)]` in Rust changes the emitted *type* and not what serde writes, so a field
 * declared `x?: T` can still arrive as `null` unless the DTO also carries
 * `skip_serializing_if = "Option::is_none"`. Every optional field in `cide_ipc::docker` does now
 * — and this is applied on top, because the two are declared in a different repository half from
 * the code that trusts them, and the failure when they disagree is a `TypeError` that takes the
 * panel down rather than anything a type-checker can see.
 */
function opt<T>(value: T | null | undefined): T | undefined {
  return value ?? undefined
}

export function adaptBoard(wire: DockerBoard): Board {
  /*
   * **A shape this function does not recognise is an error with a name on it.** (M55)
   *
   * The `switch` below is exhaustive over `DockerBoard`, so TypeScript needs no default — and
   * that is a statement about the *types*, not about the bytes. This function is handed a value
   * from the wire: a command's result, and an event's payload. When one of those is not what the
   * type says, the switch matched nothing, this returned `undefined`, and the caller threw one
   * line later on a property of it.
   *
   * Down the command road that surfaces as a rejected `invoke`. Down the **event** road it is
   * silent — a throw inside a Tauri listener callback has nowhere to go but the webview console —
   * so the board simply stayed as it was while manual Refresh went on working, with nothing
   * anywhere saying why. Three rounds of "the panel does not update" were spent on that shape.
   *
   * So the failure is named at the point it happens, and `dockerStore.adopt` turns it into a
   * notice. `kind` is the whole of the message deliberately: the payload can be tens of
   * kilobytes, and what is worth knowing is which arm was missing.
   */
  const kind: unknown = (wire as { kind?: unknown } | undefined)?.kind
  if (typeof kind !== 'string') {
    throw new Error(
      `a Docker board arrived with no \`kind\` (got ${wire === undefined ? 'undefined' : typeof wire}) — ` +
        'the wire and `adaptBoard` disagree about the payload\'s shape',
    )
  }

  switch (wire.kind) {
    case 'absent':
      return { ...BOARD_UNKNOWN, kind: 'absent', message: wire.hint }
    case 'unusable':
      return {
        ...BOARD_UNKNOWN,
        kind: 'unusable',
        message: wire.reason,
        endpoint: wire.endpoint,
        // Carried through the failure so the switcher stays on screen — without them, picking a
        // context whose daemon is down removes the only control that could undo the switch.
        contexts: wire.contexts,
        context: opt(wire.context),
      }
    case 'ready':
      return {
        kind: 'ready',
        message: '',
        endpoint: wire.endpoint,
        context: opt(wire.context),
        contexts: wire.contexts,
        apiVersion: wire.apiVersion,
        server: wire.server,
        containers: wire.containers.map((c) => ({
          id: c.id,
          name: c.name,
          image: c.image,
          state: c.state,
          status: c.status,
          health: opt(c.health),
          created: c.created,
          ports: c.ports.map((port) => ({
            private: port.private,
            public: opt(port.public),
            protocol: port.protocol,
            hostIp: opt(port.hostIp),
          })),
          /*
           * `configFiles` is carried, and this comment used to say it was dropped because "M43's
           * stack actions will" draw it. They never did — the panel passed a hardcoded empty file
           * list, so every stack action ran `docker compose -p <project> <verb>` with **no `-f`**
           * and Compose fell back to looking for a default-named file in the working directory.
           *
           * That works by accident for `compose.yaml` and `docker-compose.yml` and fails for
           * every other name: a stack brought up from `docker.compose.yaml` answered *"no
           * configuration file provided: not found"* on Recreate. The label exists precisely so a
           * tool does not have to guess — see `cide_ipc::docker::ComposeMembership::config_files`.
           */
          compose:
            c.compose == null
              ? undefined
              : {
                  project: c.compose.project,
                  service: c.compose.service,
                  workingDir: opt(c.compose.workingDir),
                  configFiles: c.compose.configFiles,
                },
        })),
        volumes: wire.volumes.map((v) => ({
          name: v.name,
          driver: v.driver,
          mountpoint: v.mountpoint,
          project: opt(v.project),
          // `bigint` on the wire (Rust's `i64`), a `number` here: a reference count large enough
          // to lose precision is a count of containers, and there is no such machine.
          inUseBy: v.inUseBy == null ? undefined : Number(v.inUseBy),
        })),
        networks: wire.networks.map((n) => ({
          id: n.id,
          name: n.name,
          driver: n.driver,
          scope: n.scope,
          project: opt(n.project),
          subnets: n.subnets,
        })),
        compose:
          wire.compose.kind === 'present'
            ? { present: true, detail: wire.compose.version }
            : { present: false, detail: wire.compose.reason },
        images: wire.images.map((i) => ({
          id: i.id,
          tags: i.tags,
          created: i.created,
          size: i.size,
          containers: i.containers,
        })),
      }
  }

  /*
   * A `kind` that is a string and not one of the three. Unreachable through the types, which is
   * exactly why it needs to be reachable in the code: the alternative is the implicit `undefined`
   * this function used to return, which threw somewhere else and named nothing.
   */
  throw new Error(
    `a Docker board arrived with an unknown kind \`${kind}\` — ` +
      'the wire has an arm `adaptBoard` has not been taught',
  )
}
