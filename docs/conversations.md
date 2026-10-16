# Companion conversations

AI consoles on an updated cide-mobile open as readable conversations. Bash retains the
terminal view, and an AI conversation has a Terminal switch for menus, unsupported approvals
and exact terminal columns. Older phones and hosts retain the existing terminal protocol.

The phone reads saved messages, not screen rows or token deltas. It polls a focused, foreground
live conversation every two seconds while working and every five seconds while ready. Activity
comes from the existing session/run state channels, independently of transcript growth. Losing
the connection never means that the harness stopped working.

Claude and Qwen are read from their JSONL transcripts, Codex from its rollout, OpenCode 1 and
MiMo through `export <id>`, and OpenCode 2 through `session export <id> --standalone`.
Export helpers have a ten-second deadline and a 64 MiB output limit. Shared host caches prevent
devices reading the same conversation from launching overlapping exports. A changed activity
state invalidates the export delay so the final messages can be refreshed promptly.

Normalized entries have stable identifiers, display order and revisions. Results update their
tool card in place, including when that card precedes the latest text. Pages contain at most
50 entries, previews at most 8,000 characters; details are requested in 32,000-character chunks.
Replacing/truncating a file, selecting a different transcript branch, or replacing an export
changes the generation and invalidates cursors. Partial JSONL records are retried after growth.
Thinking, injected instructions and system bookkeeping are excluded. Saved tool arguments,
results and diffs remain available on request.

History is the existing project session journal, not a scan of every harness conversation on
the machine. Hook-reported transcript locations are retained as optional host-only locators,
so custom storage homes and removed worktrees do not erase readable history. A phone provides
project/conversation identifiers, never file paths or launch commands. Remote projections do
not expose workspace settings, exporter credentials or source paths.

Opening history never starts a model turn. Resume is explicit, checks the desktop's resume
policy and current ownership, and spawns a continuation before publishing its owning pane.
The original directory must still exist. Active agent-owned logs remain read-only. Qwen
continuations use its event FIFO for state; OpenCode and MiMo use the same authenticated native
console API adapter and retain their own environment-variable prefixes.

Send keeps the phone's multiline draft local until submission. OpenCode/MiMo use native prompt
submission; Claude/Codex/Qwen use a gated paste and separate submit into a recognized empty
composer. Desktop input uses the same gate. Unknown input layouts, a desktop draft, permission
dialogs and paused sessions produce an actionable refusal. Busy submissions follow the
harness's native behavior and never implicitly interrupt it. Operation receipts are deduplicated
within the host process; phones never automatically replay a failed or uncertain submission.
The phone retains drafts through navigation and reconnects, but does not persist offline history.

Validation: core reader/exporter and composer fixtures, host pagination tests, the full remote
loopback suite, mobile mounted-control tests, and the real sealed-socket mobile integration test.
Physical Android/iOS verification is still required before release.
