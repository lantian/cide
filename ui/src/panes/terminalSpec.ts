/** Pure launch decisions, shared by terminal mounting and restoration checks. */
import type { DockerStream, HarnessSession, Pane, PaneRestore } from '@/ipc/client'

export interface TerminalSpec {
  program: string
  args: string[]
  cwd: string
  /** Decides which IDE server this child is told about. See `session.spawn`. */
  project?: string | undefined
  /**
   * This pane is continuing session X.
   *
   * For `claude` that is `--resume X` — with `fork`, `--fork-session` beside it. **For
   * anything else it is a screen replay**, because a shell has no such flag: `session_spawn`
   * seeds the new child's screen mirror with the screen X left behind at the last quit, and
   * writes a line under it saying the text is dead. The two readings are the same sentence;
   * only what a program can do about it differs. See `cmd/session.rs`.
   */
  resume?: string | undefined
  fork?: boolean | undefined
  resumePicker?: boolean | undefined
  /**
   * Put the real harness back on a conversation whose run child has ended. (M42)
   *
   * Rust overrides `program`, `args`, `cwd` and `resume` from it — the harness spells the
   * command, the conversation carries the directory it was filed under — so a pane that has
   * one sends `program: ''` and lets `session_spawn` decide. Read off `Pane.continues`, which
   * is durable, so a restart and the exit bar's restart re-open the same conversation from the
   * same place as the first mount did.
   */
  continues?: HarnessSession | undefined
  /**
   * This pane's bytes come from a container, not from a child on this machine. (M42)
   *
   * When set, `program`, `args`, `cwd`, `resume` and `fork` are all ignored — the session is
   * opened by `docker.openSession` instead of `session.spawn`. They are still *filled in* by
   * `specFor` because a `TerminalSpec` with holes in it would make every reader of this type
   * check which kind it was holding, and only `sessionFor` actually needs to know.
   */
  docker?: { container: string; stream: DockerStream } | undefined
}

/**
 * What a shell pane asks for as its program: nothing, meaning *the user's login shell*.
 *
 * A webview cannot read `$SHELL`, and this file used to answer that by naming `/bin/bash`
 * outright — which on macOS is a shell nobody configures (see `cide_core::shell` for the whole
 * report), so the pane ran without the user's `~/.zshrc` and therefore without nvm or
 * Homebrew on its `PATH`. The decision belongs where the fork is; `session_spawn` reads an
 * empty program as this request and supplies the login flag with it.
 */
const LOGIN_SHELL = ''

/**
 * What a pane of each kind runs. A diff pane has no process at all.
 *
 * `restore` is what makes a restored Claude pane pick its conversation up rather than start
 * a new one. `plan_restore` has already checked that Claude Code holds a transcript for that
 * id under this cwd, so `Resumable` is a claim about the filesystem and not a guess; a
 * `Fresh` entry — a shell, or a Claude pane whose transcript is gone — spawns as usual.
 *
 * A shell never resumes a *conversation* — `--resume` means nothing to bash — but a restored
 * one does name the session it is continuing, and Rust replays that session's parting screen
 * into the new child's mirror. See `TerminalSpec.resume`, and the decision in
 * `lifecycle::restore_notice`: what comes back is the visible screen, it is dead text, and the
 * line printed under it says so.
 *
 * `pane.session` rather than anything in `restore`, because `SessionRestore` deliberately
 * answers `Fresh` for every shell — `plan_restore` is about *conversations*, and a shell has
 * none. Guarded on `restore` being present at all, which is the only thing that says this
 * pane's `session` names a child from a previous process rather than a live one.
 */
export function specFor(
  pane: Pane,
  cwd: string,
  project?: string,
  restore?: PaneRestore | undefined,
  recovery?: 'picker' | 'fresh' | undefined,
): TerminalSpec | null {
  /*
   * A pane opened onto an agent run's conversation re-opens **that** conversation, whatever
   * its kind says the program is (M42): Rust spells `claude --resume <id>` or the opencode TUI
   * from `continues`, in the run's directory. The one case it does not is a restored Claude
   * pane whose transcript the launch plan could not find (`Fresh`): the conversation is gone,
   * so the pane starts a new one — still in the run's directory, which `cwd` already is —
   * rather than asking the CLI to resume a file that is not there and failing after it opened.
   * A Shell-kind pane (an opencode TUI) keeps its conversation through a restore regardless:
   * `plan_restore` answers `Fresh` for every shell and cannot look inside opencode's store, so
   * opencode's own answer is the honest one.
   */
  if (restore?.restore.kind === 'missingConversation') {
    return { program: 'codex', args: [], cwd, project, resumePicker: recovery !== 'fresh' }
  }
  const continues = pane.continues == null ? undefined : (
    restore?.restore.kind === 'resumable' && pane.continues.harness === 'codex'
      ? { ...pane.continues, id: restore.restore.session }
      : pane.continues
  )
  if (
    continues !== undefined &&
    !(pane.kind === 'claude' && restore !== undefined && restore.restore.kind === 'fresh')
  ) {
    return { program: '', args: [], cwd, project, continues }
  }
  switch (pane.kind) {
    case 'claude': {
      const resume = restore?.restore.kind === 'resumable' ? restore.restore.session : undefined
      return { program: 'claude', args: [], cwd, project, resume }
    }
    case 'shell': {
      const prior = restore !== undefined ? (pane.session ?? undefined) : undefined
      /*
       * A container's pane is a shell pane with `pane.docker` set — see `cide_ipc::Pane`'s field
       * for why it is not a `PaneKind` of its own. The check is here, inside the `shell` arm,
       * rather than as a case above it, because that is what the durable state actually says:
       * every gesture on this pane behaves like a shell's, and only where the bytes come from
       * differs.
       *
       * **No `resume`.** A screen replay is right for a local shell — the text the child left
       * behind is the text that was true — and wrong for a container: the pane comes back after
       * a restart, opens a *new* exec, and a replayed screen above it would be a transcript of a
       * shell session that no longer exists, in a container that may not either.
       */
      if (pane.docker) {
        return { program: LOGIN_SHELL, args: [], cwd, project, docker: pane.docker }
      }
      return { program: LOGIN_SHELL, args: [], cwd, project, resume: prior }
    }
    default:
      return null
  }
}

