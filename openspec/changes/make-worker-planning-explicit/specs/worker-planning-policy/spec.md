## Purpose

Let callers choose whether a delegated assignment needs native planning before execution, so workers can implement prepared tasks immediately while open-ended work can still use the existing planning lifecycle.

## ADDED Requirements

### Requirement: New assignments execute by default

New delegated assignments SHALL accept an optional `planning` value of `required` or `skip`. Omission SHALL resolve to `skip` for a new assignment. `skip` SHALL start the worker using its normal resolved execution configuration without Cide forcing a native planning stage or registering automatic plan-exit acceptance for that assignment. This contract SHALL apply to tracked tasks, external work and ad-hoc instructions, including projects without task tracking. Other launch requirements SHALL remain applicable.

#### Scenario: Architect delegates prepared implementation work
- **WHEN** an architect dispatches a prepared frontend task and a prepared backend task without `planning`
- **THEN** both assignments resolve to `skip` and begin implementation without a Cide-imposed native planning stage
- **AND** each retains its existing task scope, checkout and concurrency rules

#### Scenario: External task without a board
- **WHEN** an orchestrator delegates an external title and brief in a project with task tracking disabled and omits `planning`
- **THEN** the worker starts directly in its resolved execution configuration

#### Scenario: Skip does not require a native planning adapter
- **WHEN** an assignment specifies `planning: skip` on an otherwise supported installation lacking native planning support
- **THEN** Cide does not refuse the assignment solely because native planning support is unavailable

### Requirement: Native planning is explicitly requested

An assignment specifying `planning: required` SHALL enter the harness's native planning lifecycle before execution. Cide SHALL retain the existing completed-plan correlation, automatic acceptance, execution-mode selection, cancellation and stale-event protections. Planning SHALL remain part of the same assignment and retain its concurrency slot; completion of a planning turn SHALL NOT itself complete the assignment or start implementation review. An unsupported native planning capability SHALL produce an actionable error rather than silently falling back to execution or a prompt-based substitute. Requiring planning SHALL NOT imply human review of the plan or a plan-only outcome.

#### Scenario: Approach still needs to be designed
- **WHEN** an orchestrator delegates investigation and approach selection with `planning: required`
- **THEN** the worker enters native planning before Cide permits the automatic transition to execution
- **AND** a successfully completed native plan is automatically accepted in the same assignment using its configured execution permissions

#### Scenario: Plan capability is unavailable
- **WHEN** `planning: required` is requested but the installed harness cannot support the native planning lifecycle
- **THEN** the caller receives a planning-specific failure and Cide does not start execution as a fallback

### Requirement: Selection follows the assignment intent

Cide's orchestration guidance SHALL direct callers to request `required` when the delegated work includes unresolved investigation, architecture, decomposition or approach selection, and to use `skip` for prepared implementation tasks and corrections within an agreed approach. Cide SHALL NOT infer this choice from role names, the existence of a task or reference, prompt keywords or a model classifier. A worker starting directly in execution SHALL retain the ability to inspect code, reason about local steps and ask for clarification through existing worker question mechanisms.

#### Scenario: Architect and executor use different policies
- **WHEN** an architect receives an open-ended design assignment with `required` and later delegates agreed implementation tasks with `skip`
- **THEN** only the design assignment receives a mandatory native planning stage

#### Scenario: Role name does not choose the policy
- **WHEN** a role named architect receives a prepared task with `skip` or a backend role receives an investigation with `required`
- **THEN** Cide follows the explicit assignment choice in each case

### Requirement: All worker launch surfaces agree

Role-dispatch and worker-session creation APIs SHALL expose the same `planning` values and omission rules. Manual task launch SHALL allow selecting planning before enqueueing; its normal launch action SHALL default to execution. Automatic assignment triggers and prepared OpenSpec implementation handoffs SHALL use the execution default. Ordinary user consoles, automatic orchestration planner/reviewer tabs and OpenSpec authoring workflows SHALL retain their existing lifecycle unless they explicitly create a delegated worker assignment under this contract. Invalid values, including null or values of another type, SHALL be rejected before enqueueing, creating a session or delivering instructions to an existing worker.

#### Scenario: Named role and worker tab behave consistently
- **WHEN** the same prepared assignment is launched through role dispatch or through worker-session creation with no `planning` choice
- **THEN** each uses `skip` and neither forces native planning merely because the session is a worker

#### Scenario: Manual launch with planning
- **WHEN** a user selects planning before dispatching a task
- **THEN** the assignment is enqueued with `required`, and subsequent ordinary dispatches still default to execution

#### Scenario: Malformed planning selection
- **WHEN** a caller supplies `planning: auto`, null or a boolean
- **THEN** the request fails with the supported values and no worker is enqueued, opened or given that request's instructions

### Requirement: Planning does not grant execution permissions

The planning policy SHALL be independent of role permissions, sandbox restrictions, dangerous-permission admission and model selection. `skip` SHALL remove only Cide's mandatory planning lifecycle. It SHALL NOT remove a configured permission mode, grant bypass permissions or auto-answer ordinary tool permission requests. A role explicitly restricted by `permission-mode: plan` SHALL retain that restriction under `skip`. `required` SHALL be rejected for such a plan-only role because it requests an automatic transition to execution without an execution mode permitted by that role. In Claude, the optional availability of bypass after planning SHALL remain limited to assignments whose independently resolved execution mode permits it.

#### Scenario: Default execution with ordinary permissions
- **WHEN** a worker with execution permissions resolved to `auto` receives `planning: skip`
- **THEN** it retains `auto` and its configured sandbox rather than receiving bypass permissions

#### Scenario: Explicitly restricted role
- **WHEN** a role declares `permission-mode: plan` and receives `skip`
- **THEN** its own plan-mode restriction is preserved and Cide does not automatically exit it

#### Scenario: Conflicting plan-only and plan-then-execute settings
- **WHEN** `required` is requested for a role declaring `permission-mode: plan`
- **THEN** Cide rejects the incompatible request before starting the assignment and explains that the role has no permitted execution mode

### Requirement: Assignment policy survives continuation and recovery

Cide SHALL persist the resolved policy with a new assignment and preserve it across queueing, retries, pause/resume and application restart. Reuse or continuation of an existing assignment with omitted `planning` SHALL retain its policy and current phase. An explicit matching choice SHALL NOT restart planning. An explicit conflicting choice SHALL fail before delivering the new instructions or altering the assignment, identifying the existing assignment and how to start fresh. Selecting a fresh assignment SHALL apply the new-assignment default and normal active-assignment exclusivity rules. An assignment already implementing SHALL NOT re-enter planning solely because it receives review corrections, is redispatched, or is resumed in another process. Legacy saved assignments with an existing planning phase SHALL preserve it; those without planning state SHALL retain their previous lifecycle without acquiring mandatory planning.

#### Scenario: Review sends implementation back for correction
- **WHEN** a worker that has finished the planning stage is continued with correction instructions and omitted or matching `planning`
- **THEN** it continues in execution without repeating the native plan stage

#### Scenario: In-progress plan after restart
- **WHEN** an assignment persisted in native planning is restored after upgrading
- **THEN** it retains its planning lifecycle and phase rather than adopting the new execution default

#### Scenario: Conflicting request for a live worker
- **WHEN** a caller reuses a `skip` assignment with `planning: required`
- **THEN** Cide refuses the policy change before sending the instruction and explains the fresh-assignment route

#### Scenario: New assignment with a fresh context
- **WHEN** a caller requests a fresh assignment after the existing assignment has released its slot and omits `planning`
- **THEN** the new assignment resolves to `skip` independently of the previous assignment's policy

### Requirement: Effective policy is inspectable

Dispatch and session-creation responses, run inspection and run details SHALL distinguish direct execution from required planning. Reused assignments SHALL report their retained policy. The configured policy SHALL remain distinguishable from current phase, so a `required` assignment already implementing is not presented as still awaiting a plan.

#### Scenario: Inspecting a continued planned assignment
- **WHEN** a caller or user inspects a `required` assignment after its plan was accepted
- **THEN** Cide reports required planning as the assignment policy and implementation as its current phase

#### Scenario: Reused worker response
- **WHEN** a worker-session request reuses an existing assignment without specifying `planning`
- **THEN** the response reports the retained effective policy instead of claiming that a new default was applied
