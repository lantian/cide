/**
 * Commit drafts and one-shot results belong to a project/worktree, not the mounted panel.
 * This store lasts for one window, with no disk persistence. It owns async completion too:
 * closing the Git panel must neither lose the answer nor overwrite a newer draft.
 * Import-free so the race and retention rules can be exercised without React or IPC.
 */
export interface CommitDraft {
  readonly message: string
  readonly revision: number
  readonly generating: boolean
  readonly generated: string | null
  readonly error: string | null
}

export const EMPTY_DRAFT: CommitDraft = {
  message: '', revision: 0, generating: false, generated: null, error: null,
}

/** Shared with the panel's status/tick caches: each worktree owns a separate commit. */
export function commitDraftKey(project: string | null, worktree: string | null): string | null {
  if (project === null) return null
  return worktree === null ? project : `${project}\u0000${worktree}`
}

export class CommitDraftStore {
  private drafts = new Map<string, CommitDraft>()
  private listeners = new Map<string, Set<() => void>>()

  get(scope: string | null): CommitDraft {
    return scope === null ? EMPTY_DRAFT : (this.drafts.get(scope) ?? EMPTY_DRAFT)
  }

  subscribe(scope: string | null, listener: () => void): () => void {
    if (scope === null) return () => {}
    let listeners = this.listeners.get(scope)
    if (listeners === undefined) {
      listeners = new Set()
      this.listeners.set(scope, listeners)
    }
    listeners.add(listener)
    return () => {
      listeners.delete(listener)
      if (listeners.size === 0) this.listeners.delete(scope)
    }
  }

  private write(scope: string, draft: CommitDraft): void {
    this.drafts.set(scope, draft)
    for (const listener of this.listeners.get(scope) ?? []) listener()
  }

  setMessage(scope: string | null, message: string): void {
    if (scope === null) return
    const draft = this.get(scope)
    if (draft.message === message) return
    this.write(scope, { ...draft, message, revision: draft.revision + 1 })
  }

  clearAfterCommit(scope: string | null, revision: number): void {
    if (scope === null || this.get(scope).revision !== revision) return
    const draft = this.get(scope)
    this.write(scope, {
      ...draft, message: '', revision: revision + 1, generated: null, error: null,
    })
  }

  discardGenerated(scope: string | null): void {
    if (scope === null) return
    this.write(scope, { ...this.get(scope), generated: null })
  }

  /** A stale dialog cannot approve replacing text it never showed. */
  replaceGenerated(scope: string | null, revision: number): void {
    if (scope === null) return
    const draft = this.get(scope)
    if (draft.revision !== revision || draft.generated === null) return
    this.write(scope, {
      ...draft, message: draft.generated, revision: revision + 1, generated: null, error: null,
    })
  }

  async generate(
    scope: string | null,
    run: () => Promise<{ text: string; isError: boolean; subtype: string }>,
  ): Promise<void> {
    if (scope === null) return
    const before = this.get(scope)
    if (before.generating || before.generated !== null) return
    this.write(scope, { ...before, generating: true, error: null })
    try {
      const result = await run()
      if (result.isError) throw new Error(result.text || `Generation failed (${result.subtype})`)
      const text = result.text.trim()
      if (text === '') throw new Error('The model returned an empty commit message. Try Generate again.')
      const current = this.get(scope)
      // Even an edit back to empty is work made since the click: ask before replacing it.
      if (current.message !== '' || current.revision !== before.revision) {
        this.write(scope, { ...current, generating: false, generated: text })
      } else {
        this.write(scope, {
          ...current, generating: false, message: text, revision: current.revision + 1,
        })
      }
    } catch (error) {
      const detail = error !== null && typeof error === 'object' && 'message' in error
        ? String(error.message) : String(error)
      this.write(scope, { ...this.get(scope), generating: false, error: detail })
    }
  }
}

export const commitDrafts = new CommitDraftStore()
