# Experimental decision assistance

Decision assistance uses a small decision model to guide Codex and OpenCode 2. The
experiment is **off by default, separately for every project**. Turning it on does not
enable it in another project. Agent worktrees use their owning project's settings.

Open Settings → **Decision assistance**. Enable the project, configure the shared API
connection and use **Test connection**. The default URL is `http://127.0.0.1:11434` and
the default model name is `d1`; use the name installed in your Ollama instance. Cide
does not download models or start the Ollama server. The connection uses Ollama's typed
[`/v1/systemone` API](https://docs.ollama.com/capabilities/decision), available from
Ollama 0.35, rather than the chat completion API.

**Test connection** allows up to 120 seconds overall for loading an installed model and
checking its response. It then measures a warm request using the configured runtime
timeout. The result shows both timings and warns if the warm request exceeds that budget.
The default two-second runtime timeout remains unchanged: a coding turn must not wait
for a long model load. A cold model can therefore pass the connection test while its first
runtime request would have timed out. The tiny warm probe is not a latency guarantee for
larger inputs. Ollama may unload the model again according to its
[keep-alive setting](https://docs.ollama.com/api/systemone).

Start a new Codex/OpenCode console, or resume a conversation in a newly launched
console, after enabling the experiment. Existing processes are not restarted.
An already running OpenCode server that was started without the plugin needs a fresh
server launch. The **Sessions** tab in Insights reports adapter availability. Unsupported
CLI versions and connection failures leave ordinary harness operation available.

## Scenarios

Each scenario has **Off**, **Observe** and **Automatic** modes and its own probability
threshold. Observe records decisions without applying them. Defaults are
Automatic with an 80% threshold; completion uses 90%. These modes remain inactive
while the project experiment is off.

| Scenario | Behavior |
| --- | --- |
| Repeated failures | After three similar failed tool actions without progress, suggest inspecting the error or changing approach. A cooldown limits repeated interventions. |
| Context ranking | Rank up to 32 retrieved candidates and retain a configurable number (default 8). Conversation history, tool results and durable instructions are preserved. |
| Research route | At each new user turn, choose a focused research step or proceed without extra research. Also select text search, indexed symbols, diagnostics or local documentation for the session-scoped `cide_context` MCP tool. Worktrees use their own checkout; an index for another checkout is not substituted. |
| Verification | Discover package scripts, Cargo, Go and Python checks and combine them with project commands. Retain commands marked required. Suggest checks through normal harness tools and permissions; Cide does not execute these commands itself. |
| Completion | Consider an additional turn only for a specific outstanding check or plan item. At most two continuations per user request; interruption, permission/input waits and reported blockers prevent continuation. |
| Model and effort | At a user-turn boundary, choose among permitted named model/effort profiles. An explicit model selection stays pinned until the session is put in Auto. |

Repeated failures uses two policy steps from one model response. First, add the
probabilities of `inspect` and `change` and compare that combined probability with
the configured threshold. Intervention must also be more likely than `keep`; ties
leave the harness alone. Then select the more likely intervention, without another
threshold or model request. For example, `inspect 38% / change 32% / keep 30%` gives
70% support for intervention. At a 50% threshold this proposes inspecting the error;
at the default 80% it remains below threshold. Insights shows this gate, the
conditional preference among interventions (54% / 46% here), and the original
distribution. Passing the gate does not imply delivery: Observe, cancellation,
cooldown and native acceptance still determine whether a suggestion is applied.

Verification is evaluated at most once per observed file revision and configuration
period, including below-threshold, Observe and error results. Unrelated tools and a
new prompt alone do not repeat that evaluation; another observed edit or settings
change allows it again. A skipped request (disabled, busy or queue full) does not
consume the evaluation. Successful native check commands are recognised even when
the agent ran them independently. Matching is by exact command after trimming
whitespace; compound shell commands and equivalent spellings are not inferred.
The bounded success cache is cleared after an observed edit, and a failed rerun
invalidates that command's previous success.

Research routing runs at a native user-turn boundary even with no model profiles or
verification commands configured. A decision to proceed without extra research is
recorded as **unchanged**. A selected research suggestion is added as a separate input
item to the Codex turn, or to the next OpenCode request's system context. Original user
input and attachments are retained. Native acceptance is recorded separately from the
decision. Off, Observe, A/B control and probability thresholds still apply.

Context ranking requires retrieved candidates from `cide_context`; ordinary native
search tools do not trigger ranking. The retrieved results and research suggestions
remain advisory: the harness can choose a different next step.
Each evaluated candidate adds one Context ranking record, including decisions below
the threshold or in Observe mode. An empty result adds none. The tool's `note` reports
why routing or ranking was skipped or left unapplied, including interruption, pending
approval, an unavailable adapter, an occupied decision slot, timeout or disabled mode.
Codex helper threads do not contribute to the visible conversation's decision state.

Zero counts in other scenarios can be expected: verification needs an observed edit
and at least one configured or discovered check; completion needs an outstanding check
or plan item; model routing needs permitted profiles and an unpinned session; repeated
failures needs three similar failures. A single HTML file without package scripts has
no automatically discovered verification commands. Insights shows these trigger
conditions on its scenario cards and adapter availability under Sessions.

Profiles are defined in the shared connection section, then individually allowed in
each project. OpenCode profiles use `provider/model`; effort is its native variant.
Codex profiles use the native model name and optional effort. There are no hardcoded
price claims or automatic provider downloads. In **Insights → Sessions**, select
**Auto** to allow routing, or pin the current session again. An invalid routed Codex
model falls back to the original user request and records the rejection.

Additional verification commands accept one command per line. Prefix a command with
`!` to mark it required. After an observed edit in the current user turn, required
checks become outstanding obligations independently of Verification suggestions.
Even if Verification is Off, Observe, below threshold or returns an error, Completion
can evaluate those obligations using its own mode, threshold and continuation limit.
A free-text progress report alone does not create an obligation; required project
checks, accepted optional suggestions and native unfinished plan items do.
Removing a required setting removes that configured obligation, and a new user turn
without edits does not inherit an obligation solely from an earlier edit.
An exact duplicate discovered in `package.json` remains one required check, rather
than appearing as an optional choice too.

A successful check is deduplicated only when the native event
reports that exact command on the same observed file revision. A new file edit makes
the check eligible again. Shell wrappers or different command spellings are not assumed
equivalent. File revisions here describe observed edit-tool events, not a filesystem snapshot.

## Decision Insights

Run **Experimental: Decision Insights** from the Command Palette, or use the button in
settings. The dialog contains six scenario summaries, a virtualized decision history,
input state, named alternatives and their probabilities, policy/delivery results,
subsequent observed outcomes and optional Helpful/Not helpful feedback. Probability is
the model's output distribution, not measured correctness or a generated explanation.

The history belongs to the **open Cide window**, including while the dialog is closed.
Closing the window drops its journal. Clear history resets the window's measurements.
Nothing in this journal is written to disk or restored after restarting Cide. The
backend retains at most 10,000 detailed records or 64 MiB; scenario totals survive
detail eviction, and p50/p95 use the last 2,048 decisions per scenario. Settings and
the bundled OpenCode plugin asset are persistent; prompts, decision responses,
feedback and analytics are not saved by this feature. Native harnesses and Ollama
retain their own normal logging policies.

Comparison shows observed turns, tools/failures, active time and reported token/cost
usage by project, harness, group and configuration period. Missing usage is **—**.
Passive sessions without the owned adapter have partial event coverage, explicitly
shown in the comparison. Their counters are not equivalent to full native telemetry.

Optional A/B assigns 50/50 by session lineage and can isolate one scenario or all six.
Paired sessions and native OpenCode child sessions share their parent's group. Agent
runs notified back to their source session inherit that source's group; other runs
use their own run identity. Changing settings starts another measurement period.
For consistent instrumentation, compare native control/treatment groups in an enabled
project. Comparisons across projects and tasks are observational, not proof of saved
time, tokens or improved correctness.

## Runtime limits and validation

One decision may be outstanding per session, with two inference requests running and
at most eight sessions admitted to the broker. The default timeout is two seconds,
including queue time, per inference. A new user turn evaluates research and, if profiles
are configured and the session is unpinned, model routing sequentially. The state sent
to the model is bounded (default 24 KiB).
Three consecutive endpoint failures pause inference for 30 seconds. Changes to the
user request, file revision or settings invalidate queued decisions. The OpenCode
bridge binds to loopback and authenticates each launched session.

The automated checks use fake local System One/native endpoints and a mocked OpenCode
plugin host. They cover typed responses, malformed probabilities, timeouts, cancellation,
loop/completion policy, usage deduplication, project settings, history bounds, the dialog
and normal harness regressions. The connection-test handler has also been exercised
against a local d1 on Ollama 0.40.5, including model loading and a warm request.
Live Tauri interaction and end-to-end harness effectiveness still need manual validation;
measured effectiveness depends on the workload and profiles.
