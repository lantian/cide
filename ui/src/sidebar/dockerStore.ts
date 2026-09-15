/**
 * The machine's Docker board, and what keeps it fresh. (M41)
 *
 * `specStore`'s shape, with one difference that removes a whole class of bug: **there is no
 * project here.** A daemon belongs to the machine, so there is no attach, no per-project reset,
 * and no "did the project change during the await" re-check — every window is looking at the
 * same thing.
 *
 * The rules that do carry over:
 *
 * **Every write path goes through `adopt`, and `adopt` goes through `newerBoard`.** The drop that
 * matters is that a real board must never be replaced by "nobody has asked". `newerBoard` returns
 * the identical object when it drops, so a reader selecting `s.board` sees the same reference and
 * does not re-render; nothing here may defeat that by calling `set` unconditionally.
 *
 * **The subscription lives in `App.tsx`, not in the host.** `gitCountStore`'s reason: the rail's
 * badge has to stay live while the sidebar is shut or showing Files, and a listener registered
 * inside a panel goes stale the moment that panel unmounts.
 *
 * # The event's payload is adopted directly, and Rust is never asked in response
 *
 * `tasksStore`'s rule, and it is load-bearing here for a second reason: a board is a round trip to
 * a daemon that may be a virtual machine, and `docker_state`'s coalescer guarantees exactly one
 * read in flight. A store that answered every event with a fresh `board()` call would defeat the
 * coalescing that makes `cide://docker-changed` safe to send without a revision.
 */
import { create } from 'zustand'
import { docker as dockerApi, type DockerBoard as WireBoard } from '@/ipc/client'
import { notifyFailure } from '@/chrome/notices'
import { adaptBoard } from './DockerPanel/adapt'
import type { DetailPort } from './DockerPanel/detailModel'
import {
  BOARD_UNKNOWN,
  newerBoard,
  type Action,
  type Board,
  type Removable,
  type StackAction,
} from './DockerPanel/model'

interface DockerStore {
  /** What the daemon last said. **Never null** — see `BOARD_UNKNOWN`. */
  board: Board
  /** True while a gesture is in flight, so a button cannot be pressed twice. */
  busy: boolean
  /** The daemon's own words when a gesture failed, cleared by the next one. */
  refresh: () => Promise<void>
  adopt: (board: WireBoard) => void
  act: (container: string, action: Action) => Promise<void>
  use: (endpoint?: string) => Promise<void>
  /**
   * Replace a container with one published on these ports.
   *
   * `busy` for the whole call, which is a remove plus a create plus a start — genuinely a second
   * or two — and a second Apply pressed meanwhile would act on a container that is already gone.
   */
  recreate: (container: string, ports: readonly DetailPort[]) => Promise<void>
  compose: (
    project: string,
    action: StackAction,
    workingDir: string | undefined,
    files: readonly string[],
  ) => Promise<void>
  /**
   * Remove an image, a volume or a network. (M56)
   *
   * Refusing is the ordinary answer, not an error to be tidied away — Docker declines while
   * anything is using the thing — so the sentence it gives is raised as a notice, exactly as a
   * container action's is. See [`reportDockerFailure`].
   */
  remove: (target: Removable) => Promise<void>
}

/**
 * Report a gesture that the daemon refused. (M57)
 *
 * # Why a notice and not a line in the panel
 *
 * It was a line in the panel — `failure`, drawn at the top of the list column — and that is
 * exactly where the user is not looking. Scroll down to the Images section, press Remove on an
 * image a container is using, and Docker's refusal is rendered a few hundred pixels above the
 * fold. Reported as **"no error notification when trying to remove image that is used"**: the
 * sentence existed and had nowhere to be seen.
 *
 * `chrome/notices.ts` is the surface for this and says so — *the outcome of something somebody
 * just did* — and a cide toast never dismisses itself, so a refusal naming a container id can be
 * read at leisure. All five gestures route here, not just removal: stopping a container while
 * scrolled to the bottom had the same defect and nobody had happened to hit it.
 *
 * `project: null` because a daemon belongs to the **machine**. Stamping it with whatever project
 * the window happens to show would hide it from every other one — and `docker_remove` is reachable
 * from a panel that is deliberately project-less.
 */
function reportDockerFailure(error: unknown): void {
  notifyFailure(error, { project: null })
}

export const useDocker = create<DockerStore>((set, get) => ({
  board: BOARD_UNKNOWN,
  busy: false,

  refresh: async () => {
    const wire = await dockerApi.board()
    if (wire === null) return
    get().adopt(wire)
  },

  adopt: (wire) => {
    /*
     * **The translation is guarded, and a failure is shown.** (M55)
     *
     * `adaptBoard` walks a wire shape it did not construct, and its `switch` has no default —
     * a payload with an unexpected `kind` returns `undefined` and the `newerBoard` beneath it
     * throws on the next property access. That is correct for a command, whose rejection reaches
     * a caller; it is *silent* for an event, because a throw inside a Tauri listener callback has
     * nowhere to go but the webview console.
     *
     * Silent is the worst available outcome here and it is the one that cost three rounds of
     * "the panel does not update": the board simply stays as it was, manual Refresh still works
     * because it is a different call, and nothing anywhere says why. A notice names it.
     */
    let next: Board
    try {
      next = newerBoard(get().board, adaptBoard(wire))
    } catch (error) {
      notifyFailure(error)
      return
    }
    set((state) => (next === state.board ? state : { board: next }))
  },

  /**
   * Do one thing to one container.
   *
   * The failure is **kept and shown**, never swallowed: this is a mutation, and `client.ts`'s
   * note on `runCommand` applies — a button whose failure is caught reports success and does
   * nothing, which shipped three times before that rule was written down.
   */
  act: async (container, action) => {
    if (get().busy) return
    set({ busy: true })
    try {
      get().adopt(await dockerApi.action(container, action))
    } catch (error) {
      // `errorText` and not `String(error)`: a rejected `invoke` is a tagged object, and
      // `String()` of one is `[object Object]` — see that module's header.
      reportDockerFailure(error)
    } finally {
      set({ busy: false })
    }
  },

  /**
   * Bring a stack up, down, or restart it.
   *
   * `busy` for the whole call, which for `up` on a stack that pulls images is genuinely a long
   * time — that is the honest state, and a panel that let a second `up` be pressed meanwhile
   * would run two Compose invocations against one project.
   */
  compose: async (project, action, workingDir, files) => {
    if (get().busy) return
    set({ busy: true })
    try {
      get().adopt(await dockerApi.compose(project, action, workingDir, files))
    } catch (error) {
      // Compose's own last line — "port is already allocated" — which is the sentence that says
      // what went wrong. Never swallowed: see `act` above.
      reportDockerFailure(error)
    } finally {
      set({ busy: false })
    }
  },

  remove: async (target) => {
    if (get().busy) return
    set({ busy: true })
    try {
      get().adopt(await dockerApi.remove(target))
    } catch (error) {
      // Docker's own words — "image is being used by stopped container a1b2c3", "volume is in
      // use", "has active endpoints". Kept and shown: this is the sentence the user asked for by
      // pressing the button, and it is more useful than the removal would have been.
      reportDockerFailure(error)
    } finally {
      set({ busy: false })
    }
  },

  recreate: async (container, ports) => {
    if (get().busy) return
    set({ busy: true })
    try {
      get().adopt(
        await dockerApi.recreate(container, {
          // Built key by key rather than spread, because `exactOptionalPropertyTypes` is on and
          // the wire's `public?: number` will not take an explicit `undefined` — which is the
          // same distinction the DTO's `skip_serializing_if` makes on the way back.
          ports: ports.map((port) => ({
            private: port.private,
            protocol: port.protocol,
            ...(port.public === undefined ? {} : { public: port.public }),
            ...(port.hostIp === undefined ? {} : { hostIp: port.hostIp }),
          })),
        }),
      )
    } catch (error) {
      // The old container may already be gone by the time this fails — Rust's sentence says so
      // when that is the case, which is why it is shown rather than swallowed.
      reportDockerFailure(error)
    } finally {
      set({ busy: false })
    }
  },

  /** Point at another daemon. `undefined` restores cide's own ladder. */
  use: async (endpoint) => {
    if (get().busy) return
    set({ busy: true })
    try {
      get().adopt(await dockerApi.use(endpoint))
    } catch (error) {
      // `errorText` and not `String(error)`: a rejected `invoke` is a tagged object, and
      // `String()` of one is `[object Object]` — see that module's header.
      reportDockerFailure(error)
    } finally {
      set({ busy: false })
    }
  },
}))
