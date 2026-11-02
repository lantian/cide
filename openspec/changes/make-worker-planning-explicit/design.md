## Context

See `proposal.md` for motivation and `specs/worker-planning-policy/spec.md` for the behavior contract. The user confirmed execution by default and explicit planning for work that needs it.

Current entry points independently force native planning:

- `cmd/agents.rs::plan_dispatch` sets `DispatchSpec.planning` to a new planning state for every role assignment, including external work and ad-hoc instructions.
- `cmd/session.rs` starts planning from `Voice::Worker` when a worker tab is first created. This also affects the `cide_session_open` fallback when no named roles are available.
- `agents.rs` and `native_plan.rs` coordinate the native transition and save assignment phase. Claude argv is rewritten to Plan; Codex collaboration presets and OpenCode/MiMo native agents use their adapters, and Qwen uses its interactive native entry.
- Dispatch may tell a live run, resume the last conversation, or create a fresh assignment. Worker-session creation may return an existing task session. Choice resolution must precede these routes, rather than being applied only at process spawn.
- `RunPlan.planning` currently mixes absence of the lifecycle with legacy state. Worker-tab restoration currently depends on records created by a native planning controller. Neither is enough to represent explicit direct execution durably.
- OpenSpec standalone authoring/apply sessions and automatic orchestration tabs have separate purposes and launch rules. Their use of the word planning is not a reason to change those workflows.

Durable policy belongs in Rust, following ADR 0002. Frontend selection is transient until dispatch, and generated IPC types remain the wire contract.

## Goals / Non-Goals

**Goals:**
- Resolve one assignment policy consistently before enqueueing, starting or reusing a worker.
- Separate the choice of planning stage from runtime phase and from execution permissions.
- Retain existing native planning adapters and their correlation/acknowledgement protections when explicitly requested.
- Make policy selection and recovery deterministic without extra model calls.

**Non-Goals:**
- Detecting whether a brief is complete, classifying natural-language tasks, or introducing a readiness score.
- Adding project-wide or role-wide planning defaults, changing role frontmatter, or editing the task body to encode launch state.
- A plan-only job type, a human plan-approval workflow, or general live switching between native modes.
- Changing ordinary console, OpenSpec authoring, automatic planner/reviewer, sandbox or dangerous-permission policy.

## Decisions

### 1. Use an explicit per-assignment policy

Introduce a shared `PlanningPolicy` wire enum with `required` and `skip`. Requests carry optional `planning`; new assignments resolve omission to `skip`. Keeping the request optional preserves the distinction between omitted input on reuse and an explicit attempt to change policy. Reject null, wrong types and unknown values at both MCP and IPC boundaries instead of treating them as absence.

Carry the choice through `DispatchRequest`, `DispatchArgs`, worker `SessionRequest` and tab-opening/session-launch options. Do not use `permission-mode` as the stage selector. The public request uses `planning`; persisted/run-inspection policy uses `planningPolicy` to distinguish it from existing `planning` progress state.

**Alternative:** infer readiness from task IDs, references, role names or text. Rejected because a task can describe investigation and a task-less instruction can contain a precise patch request. **Alternative:** configure the role once. Deferred because the same role can receive both kinds of work; the user's case requires a decision at handoff.

### 2. Resolve new and reused assignments differently

Use a shared pure resolution rule, with routing decisions taken under existing assignment/session exclusivity guards:

| Assignment route | Requested choice | Result |
| --- | --- | --- |
| New assignment, including accepted `fresh` launch | Omitted | `skip` |
| New assignment | Explicit | Requested policy |
| Reuse/continuation | Omitted | Retain saved policy and phase |
| Reuse/continuation | Same as saved policy | Retain policy and phase |
| Reuse/continuation | Different from saved policy | Refuse before delivery; identify the held assignment and fresh-launch route |

An ended conversation resumed for the same role/task remains a continuation. `fresh: true` requests a new assignment, subject to current held-pair refusal rules. For worker tabs whose API has no fresh option, a mismatch explains how to close/stop the existing worker and create a new assignment rather than pretending to apply the new choice.

Validate incompatibility before `Planned::Tell`, follow-up queue insertion, task turn bookkeeping or the early `OpenedSession { reused: true }` return. Do not use the new default to reinterpret a live assignment. Required assignments already implementing retain their policy for inspection but do not re-enter Plan on review send-back, retry or respawn. Follow-ups during an active planning phase retain that stage and the existing generation protections.

**Alternative:** silently apply a new choice only at the next process spawn. Rejected because it would make live tell, native resume and new-session routes behave differently. **Alternative:** force `fresh` automatically on a mismatch. Rejected because it would discard context or collide with an active assignment without the caller asking.

### 3. Gate native planning at both launch paths

Persist the resolved policy on `DispatchSpec`/live/saved run data and pass it through the admission and launch plan. Create fresh `native_plan::State` only for a new `required` assignment. A new `skip` assignment has no Cide planning controller; the existing non-planning spawn path selects the resolved execution permissions.

Worker voice/origin continues to identify scope and ownership, but no longer implies native planning. Pass policy through `agent_rpc` → `claude_tab::Open` → session launch, and create/restore the controller only for required assignments. Store worker assignment metadata for both choices independently of a controller so `skip` is durable and inspectable.

Audit planning capability preflights as well as adapter entry. A skip assignment must not require the Claude plan-question hook, Codex Plan preset, exposed OpenCode/MiMo plan agent or Qwen `/plan` support solely for a stage it will never enter. Ordinary harness availability and bridge requirements still apply.

| Harness | `skip` | `required` |
| --- | --- | --- |
| Claude | Existing execution argv; no host Plan rewrite or plan-exit accepter | Existing Plan argv rewrite and correlated automatic ExitPlanMode acceptance |
| Codex | Normal/default collaboration path | Existing native Plan preset and completed-plan transition |
| OpenCode/MiMo | Existing execution agent/transport selection | Existing exposed native plan agent and acceptance path |
| Qwen | Normal execution launch | Existing interactive native plan entry and acknowledged transition |

Do not add planning instructions to task or role prompts as an adapter fallback. Keep the current slot-retention and review-start rules for required planning. Native planning support failure is meaningful only for `required`.

**Alternative:** launch every worker in Plan and immediately accept an empty plan for `skip`. Rejected because it retains the unnecessary stage and its capability/deadlock risks.

### 4. Preserve permissions as an independent constraint

Resolve role/override permissions and project defaults through the existing code. Stage selection does not change that resolution or dangerous-permission admission. For Claude, keep `--allow-dangerously-skip-permissions` conditional on independently authorized `bypassPermissions` execution after a required plan; skip does not manufacture either flag.

For a role explicitly configured with `permission-mode: plan`, skip means Cide adds no stage controller: the role still starts with its own read-only Plan configuration. Reject `required` paired with that effective role restriction before launch, explaining that plan-then-execute needs a permitted execution mode. Do not reuse the current fallback from a role's literal `plan` to project execution permissions, which would make this policy look like permission escalation. The same incompatibility check applies on restoration before a pending transition if role configuration has changed.

The policy requires an initial native stage; it does not promise that an agent will never choose to plan or that it will never ask a question during execution. Existing worker question mechanisms remain available.

**Alternative:** make `skip` override a restrictive role or interpret `required` as authority to execute. Rejected because the assignment choice is not a permission grant.

### 5. Expose intent where work is handed off

Document `planning` in both MCP tool schemas and responses. Guidance examples:

| Work handed off | Choice | Reason |
| --- | --- | --- |
| Investigate an unclear requirement, select architecture or decompose an unresolved feature | `required` | The delegated work must establish an approach before execution |
| Implement the architect's prepared frontend or backend task | Omit or `skip` | Scope and approach have already been supplied |
| Correct an implementation after review | Omit on continuation | Retain the assignment's execution phase |
| Perform a precisely bounded check | Omit or `skip` | Reading and reasoning do not themselves require a native stage |

Add a non-sticky “Plan before execution” choice before manual task dispatch, using the existing launch surface and UI-kit controls. The normal immediate dispatch action remains direct execution; a secondary action can open the choice where a gesture currently dispatches immediately. Automatic assignment triggers and existing OpenSpec implementation handoffs use the new default. Do not add native Plan mode to standalone OpenSpec Propose/Explore sessions: those workflows write planning artifacts under their own instructions.

Return the effective policy in dispatch/session-open answers and expose it in run inspection and run details. Display policy separately from phase: “Plan before execution” plus “Implementing” is valid. Worker-tab inspection must have the same metadata even when no role run exists. Update `docs/checks.md` and orchestration guidance to remove unconditional native-plan claims.

**Alternative:** expose only a CLI flag. Rejected because dispatch happens through MCP, manual actions and restored tabs, and the orchestrator must be able to verify the result.

### 6. Migrate saved state without restarting work

Persist the new enum alongside existing progress, with compatibility decoding rather than a blanket serde default:

- Missing policy with existing planning state: infer `required` and preserve its generation and phase, including Implementing.
- Missing policy with no planning state: retain the legacy non-planning lifecycle and represent it as `skip`.
- Explicit `required` and a saved phase: preserve both; retries and startup must not reconstruct a new generation merely from the policy.
- Explicit `skip` with saved pending planning state is inconsistent; report a recoverable metadata error instead of silently choosing one and approving a transition.

Apply this to queued/live/ended role runs and private worker-conversation metadata. Existing policy-less worker planning records infer required. A legacy worker with no such record must not acquire a new stage simply because its restored pane has worker origin. Persist both kinds of new workers even when no controller exists.

New request omission defaults change at creation; saved assignment omission is a migration concern. Keep these decoding rules separate. Never rewrite role files, existing task statements, project permission settings or main console state as migration.

**Alternative:** default every absent saved field to skip. Rejected because it would interrupt pending plans and lose their guarded transition. Conversely, defaulting old workers to required would replay planning for already executing work.

## Risks / Trade-offs

- [Old callers expected compulsory planning] → Mark the new-assignment default as a behavior change, update schemas/guidance and provide explicit `required` in examples for unresolved work.
- [A ready-looking task is actually ambiguous] → Preserve worker clarification and hand-back mechanisms; orchestration guidance asks the caller to choose required when the approach is unresolved. This version deliberately does not infer intent.
- [A launch or reuse path drops the policy] → One resolver, validation before tell/reuse side effects, cross-path tests, generated contract checks and effective-policy inspection.
- [Saved absence is confused with the new default] → Test legacy fixtures for Planning, Approving, Implementing and no lifecycle, including worker tabs and process respawn.
- [Skipping planning accidentally changes permissions] → Test the policy × permission matrix, plan-only conflicts and dangerous-permission admission separately from stage entry.
- [Required means plan-then-execute, while someone expects a plan-only review] → Label the choice explicitly and document automatic acceptance; plan-only roles retain their restriction and cannot opt into this execution lifecycle.

## Migration Plan

1. Add shared request/inspection contracts and compatibility decoding; regenerate bindings and establish resolver/legacy-state fixtures.
2. Carry policy through dispatch, reuse, worker tabs and persistence, and gate each native adapter and capability preflight. Keep ordinary consoles and automated reviewers unchanged.
3. Add manual launch selection, inspection and orchestrator examples; update the native-planning check guide to exercise both modes.
4. Run focused Rust, wire and UI behavior checks, then the shared validation suite. Check both policies on installed harnesses without claiming fixture tests prove a live model/desktop transition.
5. Roll back implementation if necessary; snapshots retain legacy progress fields. Returning to an older binary restores its old compulsory-planning behavior for newly created work, so do not promise old binaries honor explicit skip. No source/task/role migration needs undoing.
