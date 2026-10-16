# Experimental two-agent chat

Open a project, then choose **Experimental: Open Two-Agent Chat** from the command palette.
The action has no default shortcut or toolbar/menu entry. It opens one closable tab with two
side-by-side panels and the normal draggable vertical divider.

The left panel starts a fresh session of the project's configured console harness. The right
panel first asks for the peer's harness and model. Claude Code and Codex inherit their configured
provider and credentials. OpenCode offers its native provider/model catalog; an explicit model
uses `provider/model`. Leave the model blank for the configured default. Both agents can use
the same harness. Setup starts both children together and cleans up a partial startup failure.

Enter an ordinary task on the left. The main agent's instructions tell it to consult the peer
and request review automatically. Only the left writer raises Waiting badges, notification
dots, tab/project waiting counts and desktop attention. The right reviewer stays quiet when
it finishes a turn or reaches a permission prompt; its native status and permission controls
remain available inside the panel.

The main agent is the writer; the peer has invocation-local
reviewer permissions and no cide dispatch/edit tools. Reviewers can inspect files outside
the project, execute inspection commands (Git status/diff, metadata, hashes, disk usage),
open web URLs and search the internet using their harness's native tools and configured
search integration. Claude exposes WebFetch/WebSearch, OpenCode exposes webfetch/websearch,
and Codex enables live web search. Provider availability and managed network restrictions
still apply.

OpenCode 2 reviewers select OpenCode's native automatic keyless search-provider pool for
their invocation. This avoids the first-use provider form, whose cancellation or one-minute
timeout otherwise returns `Web search cancelled`. OpenCode handles provider selection and
rate-limit failover. No API key, global provider selection, form auto-answering or permission
bypass is added; other OpenCode sessions retain their usual search configuration.

The reviewer may create and execute temporary analysis scripts, including Python scripts
that scan large codebases or datasets. Each invocation gets a private directory outside
the project, identified by `CIDE_PEER_SCRATCH` and the temporary-directory environment.
Scripts, intermediate files and generated results belong there. It is removed on session
exit or failed startup; a restored reviewer receives a fresh directory and updated
instructions. Python bytecode writes beside source files and Git's optional index writes
are disabled for the reviewer.

Project/source/data edits, Git mutations, commits, builds, tests, dependency installation
and worker dispatch remain outside the reviewer's role. Ask the main writer to perform
builds or tests and share the results. Claude and OpenCode block dedicated edit/delegation
tools; their shell/script restrictions follow reviewer instructions and are not an OS
filesystem sandbox. Codex uses its native read-only profile with writes granted only to
scratch and no approval escalation. Conflicting legacy Codex sandbox settings or additional
write grants reject reviewer startup. Project and global launch settings are not rewritten.
Claude reviewers require a CLI supporting `--system-prompt-snapshot off` (verified on
2.1.301), so restored sessions receive current reviewer instructions and scratch paths.
Restart existing paired sessions after updating Cide to receive the new tools and policy.

Agents communicate through the pair-scoped `cide_peer_chat_send(message)` MCP tool. Messages
are labelled by side and harness and routed to the currently bound peer session. A terminal
reply alone is not sent. Receipts distinguish queued, writing, delivered, failed, cancelled and
uncertain delivery; delivered means the harness accepted the prompt, not that it answered.
The header tracks **Main → Peer** and **Peer → Main** separately. **No reply sent yet**
means no peer message has been queued; text printed in the peer terminal is not forwarded.
Queues are bounded to 16 messages per direction. Messages wait until the recipient is
ready; Claude delivery also requires an empty input. They deliver automatically. A long turn
does not time out or pause the pair. Pause or close the tab to cancel pending messages.
If an older build already paused the pair after its former 40-second timeout, restart
with the updated build and submit a continuation task on the left to resume it. A reply
rejected before queueing needs to be requested again; it is not replayed automatically.

Paired Codex consoles attach their native TUI to a private authenticated local app-server.
Peer messages use `turn/start` directly and wait for the current turn to finish; they are
never pasted into the composer. The terminal keeps its draft and handles its own approvals
and questions. Delivery is confirmed by the server's acknowledgement. Native protocol events
keep Cide's thread identity, status and file reloads current even when remote Codex does not
emit invocation-local CLI hooks. Changing the native conversation pauses the pair and cancels
pending messages. Both the TUI and its server stop when the paired pane closes.

This requires a Codex version supporting `--remote`, `--remote-auth-token-env` and authenticated
`app-server` connections (verified on 0.162.1). Unsupported versions fail setup with an upgrade
message. Restart existing paired Codex consoles after updating Cide. An older reply already
left in a composer is not submitted or replayed automatically.

Claude delivery waits for a recognized empty composer, keeps a visible witness between
UTF-8 chunks so trailing spaces can be verified, then removes it and
submits separately. Unknown layouts, drafts, shell mode and permission dialogs are refused.
Manual input is briefly suspended while composing a Claude peer message. OpenCode uses its private
authenticated API. A failed or uncertain delivery pauses the pair; inspect the recipient before
resuming. A partially composed body is left visible for manual recovery. It is never retried
blindly or cleared automatically.

Paired OpenCode 2 consoles expose cide's message tool directly rather than through Code Mode.
This keeps the reply tool available while the reviewer retains its read-only permissions.
After updating this backend behavior, restart Cide so paired consoles start with the new
configuration, restore/resume the pair, and request the review again.

**Pause conversation** cancels queued deliveries, while the terminals remain usable. Resume
with the button or submit a new task on the left. Closing the tab ends its paired processes;
**Reopen closed tab** restores both native conversations. Workspace restart and project reopen
restore the pair paused and do not replay messages. A missing transcript requires an explicit
choice to start that side fresh. Saved native conversation IDs retain each side's harness.
The panels stay together: split, mirror, detach, move, maximize and individual close are refused.

This is experimental. CLI composer layouts can change, and the conservative recognizer may
withhold a message that needs manual recovery. Collaboration follows harness instructions;
peer agreement does not grant user approval. Paid end-to-end model conversations are not part
of the automated checks.

The concept is adapted from [agterm's two-agent chat cookbook](https://github.com/umputun/agterm/tree/master/cookbook/two-agent-chat).

For a browser audit, run `node ui/scripts/audit-peer-chat.mjs` from the repository root
with Chromium installed (`CHROMIUM` can override its path). It mounts the real paired
terminal components, verifies their layout and left-panel keyboard input, and fakes only
the Tauri boundary. It starts no harness and makes no model calls. The picker-only
`check:peer-chat` does not check browser layout.

`python3 scripts/audit-opencode-peer-tools.py` checks an installed OpenCode 2 against a
temporary local model endpoint and fake MCP server. It reproduces the missing reply tool
under Code Mode, then verifies direct visibility and execution while mutation tools remain
hidden. It also reproduces the former external-directory denial and verifies external read/glob
with the reviewer policy. It makes no paid model calls and sends no messages to real agents.
It also verifies native shell execution of a temporary analysis script and URL fetching from
`https://example.com`. It reproduces a cancelled first-use search-provider form, then executes
`websearch` against the public internet and requires a successful search response without a
selection form. Providers may return result URLs or a valid empty-result response. Search-provider
keys are removed from its environment so this exercises the keyless pool.

`cargo test --locked -p cide-core --test real_peer_reviewer -- --ignored --nocapture`
passes production reviewer policies into installed OpenCode, Claude and Codex audits.
Claude executes a scratch Python script and fetches a URL through a local fake model;
its search tool must be exposed and edit/delegation tools hidden. Codex's native command
API executes a scratch script and must reject writes to project files and Git metadata.
Its effective configuration must enable live web search. These audits use temporary state,
no account credentials and no paid model calls. Fetch checks read `https://example.com`.
Local sockets, network access for URL fetching/search and Codex's OS sandbox support are
required. OpenCode search results are
exercised against real keyless providers; Claude/Codex search-provider results remain outside
this audit.

`cargo test --locked -p cide-app --test real_peer_composer -- --ignored` exercises the
shared delivery chunks and screen verification against the installed Codex terminal. It
checks complete long replies, wrapped numeric witnesses, Unicode and a follow-up while the
provider is unavailable, then confirms submitted text in Codex's input history. Its state
is temporary and its only model provider points at
an unused loopback port; no paid model endpoint is contacted.

`cargo test --locked -p cide-app --lib codex_console::tests::real_codex_native -- --ignored`
exercises the native Codex transport with the installed TUI, isolated temporary state and a
local fake model provider. It verifies a fresh reviewer's first message, long Unicode replies,
busy-turn gating, draft preservation and restoration without replay. It uses no paid endpoints.
It also checks `/new`, stale-reply rejection and server/credential cleanup on close.
Fresh and restored reviewers use the production scratch profile, receive current scratch
instructions, and expose the live web-search tool to the local fake model.
The ordinary `codex_console` tests also verify authentication, cancellation and native approval
forwarding with fake protocol endpoints. These audits need permission to open local sockets.
