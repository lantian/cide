## Why

Cide currently forces every new delegated assignment through native Plan mode, even when an architect has already analysed the work and supplied implementation tasks. Frontend and backend workers repeat that planning before Cide automatically accepts their plans, adding an unnecessary stage to an already prepared handoff.

## What Changes

- Make execution the default for new delegated assignments. A prepared task, external brief or ad-hoc instruction starts with the worker's normal execution permissions.
- Add an explicit per-assignment `planning` choice: `required` enters the existing native planning lifecycle before execution; `skip` starts execution directly. Omission on a new assignment means `skip`.
- Expose the same choice through role dispatch, worker-session creation and manual task launch. Teach orchestrators to request planning for investigation, unresolved architecture or approach selection, and skip it for ready implementation tasks and review corrections.
- Keep task type, role name and prompt wording out of mode inference. An architect may execute a prepared task, and an implementation role may receive work that needs planning.
- Persist the resolved choice with the assignment; preserve phase across queueing, retries, restart and continuation. An ordinary follow-up must not silently restart planning or change an existing assignment's policy.
- Keep planning independent of permission grants. Skipping this stage does not enable bypass permissions or remove a role's configured restrictions; requesting it retains current automatic native-plan acceptance, rather than adding human approval.
- **BREAKING behavior change:** newly created assignments whose callers omit `planning` no longer receive a mandatory native planning stage. Existing saved assignments retain their lifecycle.

## Capabilities

### New Capabilities

- `worker-planning-policy`: Explicit assignment-level selection, default execution, native planning when requested, consistent launch surfaces, observable policy, and continuation compatibility.

### Modified Capabilities

None. This checkout has no existing main capability specs; the new capability describes the change to existing delegated-worker behavior.

## Impact

- Rust contracts and orchestration: `cide-ipc` dispatch/run DTOs; `cide-agents` MCP schemas, validated requests and launch plans; `cide-app` dispatch, task triggers, worker tabs, persistence and native planning controllers.
- Harnesses: Claude, Codex, Qwen and OpenCode/MiMo must apply their native planning adapters only when the resolved assignment policy requires them. Existing execution sandbox/approval mappings remain authoritative.
- UI and documentation: manual task launch choice, effective policy in run details, generated TypeScript/IPC contracts, orchestrator guidance and the native-planning verification guide.
- Verification: architect-to-frontend/backend handoffs, explicit planning, invalid input, permission isolation, saved-state compatibility and continuation tests. No external service or new model inference is needed to select a mode.
