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
and request review automatically. The main agent is the writer; the peer has invocation-local
read-only permissions and no cide dispatch/edit tools. Claude and OpenCode reviewers use file
inspection tools, including sources outside the project; ask the main agent to run checks
and share the results. External-directory access does not grant write or shell permissions.
Project and global launch settings are not rewritten.

Agents communicate through the pair-scoped `cide_peer_chat_send(message)` MCP tool. Messages
are labelled by side and harness and routed to the currently bound peer session. A terminal
reply alone is not sent. Receipts distinguish queued, writing, delivered, failed, cancelled and
uncertain delivery; delivered means the harness accepted the prompt, not that it answered.
The header tracks **Main → Peer** and **Peer → Main** separately. **No reply sent yet**
means no peer message has been queued; text printed in the peer terminal is not forwarded.
Queues are bounded to 16 messages per direction. Messages wait while the recipient is
working or its input is occupied, then deliver automatically when ready. A long turn
does not time out or pause the pair. Pause or close the tab to cancel pending messages.
If an older build already paused the pair after its former 40-second timeout, restart
with the updated build and submit a continuation task on the left to resume it. A reply
rejected before queueing needs to be requested again; it is not replayed automatically.

Claude/Codex delivery waits for a recognized empty composer, keeps a visible witness between
UTF-8 chunks so trailing spaces can be verified, then removes it and
submits separately. Unknown layouts, drafts, shell mode and permission dialogs are refused.
Manual input is briefly suspended while composing a peer message. OpenCode uses its private
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

`cargo test --locked -p cide-app --test real_peer_composer -- --ignored` exercises the
shared delivery chunks and screen verification against the installed Codex terminal. It
checks complete long replies, wrapped numeric witnesses, Unicode and a follow-up while the
provider is unavailable, then confirms submitted text in Codex's input history. Its state
is temporary and its only model provider points at
an unused loopback port; no paid model endpoint is contacted.
