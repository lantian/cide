## 1. Shared policy and contracts

- [x] 1.1 Add the shared `required`/`skip` planning policy and optional request field to role dispatch and worker-session contracts; reject unknown values, null and wrong types at MCP and IPC boundaries. Verify omission remains distinguishable from explicit skip.
- [x] 1.2 Implement a pure new-assignment/reuse policy resolver and policy/phase compatibility checks. Cover new omission, explicit choices, matching reuse, mismatches and fresh-assignment semantics with table-driven tests.
- [x] 1.3 Add effective policy to dispatch/session-open results, run inspection and worker-session metadata, and regenerate/check TypeScript and tool contracts without conflating policy with the existing planning phase.

## 2. Role assignment and continuation

- [x] 2.1 Carry the resolved policy through `DispatchSpec`, live/saved run data, admission and launch plans. Replace unconditional planning-state creation with required-only initialization; verify tracked, external and ad-hoc new assignments default to direct execution.
- [x] 2.2 Resolve retained policy before live tell, follow-up queueing, task turn bookkeeping and ended-conversation continuation. Reject explicit mismatches without side effects; verify omitted/matching corrections preserve execution phase and that accepted fresh dispatch uses the new default.
- [x] 2.3 Implement saved-role-run compatibility decoding: existing planning state implies required, absent state preserves legacy non-planning behavior, and inconsistent explicit policy/progress produces an actionable error. Cover queued, Planning, Approving, Implementing and process-respawn fixtures.

## 3. Worker sessions without named roles

- [x] 3.1 Carry the request choice through `agent_rpc`, tab options and session launch; remove worker voice/origin as an implicit planning trigger. Verify role dispatch and worker-session creation agree in tracker-enabled and tracker-disabled projects.
- [x] 3.2 Persist worker assignment policy independently of native planning controllers, restore both modes by conversation identity, and migrate existing planning records without inventing new planning for legacy workers lacking such records.
- [x] 3.3 Validate policy on worker-session reuse before the existing-session early return; report retained choice or a mismatch with recovery guidance, without opening a duplicate tab or delivering conflicting instructions.

## 4. Native adapters and permission separation

- [x] 4.1 Gate Claude's Plan argv rewriting, question hook and plan-exit watcher on required planning. Verify skip preserves execution argv, auto/manual/acceptEdits behavior and dangerous-permission admission; keep bypass availability conditional on independently resolved permissions.
- [x] 4.2 Gate Codex native Plan collaboration selection and OpenCode/MiMo plan-agent selection/preflights on required policy. Exercise both choices through existing fake app-server/authenticated HTTP fixtures and verify unsupported planning does not block skip.
- [x] 4.3 Gate Qwen's interactive native-plan entry and event-tap expectations on required policy; verify skip follows normal launch and required retains its acknowledged native transition.
- [x] 4.4 Preserve explicit role/override plan-only restrictions under skip and reject required for that incompatible execution configuration, including a changed configuration before a restored pending transition. Verify ordinary tool permissions are never accepted by stage selection.
- [x] 4.5 Verify required planning still retains its slot, starts review only after implementation, and rejects stale/cancelled/conversation-switched acceptance events; preserve ordinary main-console, automated planner/reviewer and standalone OpenSpec lifecycle behavior.

## 5. Launch controls, inspection and guidance

- [x] 5.1 Add the non-sticky “Plan before execution” manual launch choice using the existing launch surface and UI-kit controls; keep the normal dispatch action on execution and carry the selected choice through `agentsStore` to IPC. Cover request payloads, reset between launches and launch errors with mounted UI checks.
- [x] 5.2 Show effective policy separately from current phase in run details and inspection, including a required assignment already implementing and a reused worker with no named role. Verify persisted metadata survives window/session restoration.
- [x] 5.3 Update MCP schemas/responses and orchestrator guidance with investigation-versus-prepared-handoff examples. Verify automatic assignment triggers and prepared OpenSpec implementation handoffs use the execution default while authoring workflows keep their existing instructions.
- [x] 5.4 Update `docs/checks.md` and relevant autonomy guidance, and append the implementation/validation record to `docs/journal.md`, including the new default, legacy recovery rules, explicit role restrictions and automatic plan acceptance.

## 6. Acceptance and validation

- [x] 6.1 Add an integration fixture for an architect assignment with required planning handing ready frontend/backend tasks to separate skip workers, followed by review corrections and restart. Assert the observed native mode transitions, retained policies/phases and existing isolation/concurrency rules; include the no-tracker worker-session route.
- [x] 6.2 Run focused `cide-agents`/`cide-app` policy, dispatch, native-plan, session and recovery checks; run strict Clippy, codegen/contract checks, relevant mounted task/agent UI checks, `check:kit`, theme/scale checks, TypeScript and production UI build. Complete the shared `./test.sh` validation and record remaining failures accurately.
- [ ] 6.3 Exercise skip and required on each installed harness with isolated disposable work: prepared work starts directly, explicit native planning transitions once, corrections/restoration do not replay planning, and stop/switch prevents stale acceptance. Record unsupported installations and distinguish fake-provider/desktop evidence from live-model behavior; do not restart the user's working sessions for verification.

## 7. Task discovery permission regression

- [x] 7.1 Preapprove only the trusted Cide bridge's task_get/task_list for fresh and resumed Claude role workers and anonymous workers under either planning policy; preserve explicit denies, execution permissions, other MCP servers and ordinary consoles. Add focused regression coverage and rebuild the local binary.

- [x] 7.2 Extend the trusted worker grants to task_update and task_comment as requested; retain other permissions and scope checks, update regression expectations and rebuild the local binary.
