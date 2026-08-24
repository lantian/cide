# ADR 0011 — cide ships its own rust-analyzer, resolved by a ladder, configured by provenance

**Status:** accepted (M25)
**Date:** 2026-08-24

## Context

rust-analyzer holds its entire index in memory by design — crate graph, item trees, def maps,
macro expansions, every memoized salsa query. On real workspaces that is the 2–8 GB this
codebase's own comments budget (`cide-lsp/src/discover.rs`, `cide-app/src/lsp.rs`), one server
per open project, and cide has no say in any of it: until M25 it sent rust-analyzer **zero
configuration** (no `initializationOptions`; `workspace/configuration` answered `{}`) and
resolved the binary from PATH alone.

The decision: fork rust-analyzer, add disk-backed indexing to the fork, and build the fork into
cide as the rust-analyzer it ships and prefers. The fork lives in a **sibling repository**
`../forks/rust-analyzer` (branch `cide`, cut from an upstream release tag), beside `../forks/salsa`
— not in this workspace, because 400k lines of rust-analyzer joining every
`cargo build --workspace` and every clippy run was the cost nobody wanted, and CI must stay
green on a checkout that has never heard of the fork.

The fork's roadmap is phased (see the milestone in `README.md`): phase 0 ships a stock-behaving
fork bundled and configured; phase 1 persists the index to disk (salsa's experimental
`persistence` feature — serialize on shutdown/idle, load on start); phase 2 bounds memory (LRU +
interned-value GC + a cide-side RSS watchdog whose restart is then a warm load); phase 3, if it
is ever needed, is a lazy disk-backed memo store via a salsa fork. This ADR records the
**cide-side** decisions, which are the ones a refactor would otherwise undo.

## Decision 1 — two names, two layers

The registry keeps calling the server `rust-analyzer`; the shipped file is `cide-rust-analyzer`.

The registry name is load-bearing in four places: `discover`'s merge is keyed by binary name,
`DiagnosticSourceId::for_server("rust-analyzer")` is what the Problems panel prints,
`cide_ext::contribute`'s conflict rule refuses a second server per binary name, and the
`rustup component add rust-analyzer` install hint stays true. Renaming the builtin to
`cide-rust-analyzer` would have let an extension contribute a plain `rust-analyzer` and produce
two servers claiming `languageId: rust`.

The file name is forced by the tarball: its documented contract is "put `bin/` on your PATH",
and a file there named `rust-analyzer` would shadow the user's own for every shell on the
machine — exactly the toolchain-selection decision `cide_core::toolchain`'s append-never-prepend
note says cide must never make. A distinct file name makes shadowing impossible in every
packaging format at once.

The mapping is a const table in `discover.rs` (`BUNDLED`), **not** a `LanguageServerDef` field:
a manifest field would let an extension name an arbitrary sibling binary — `cide` itself, say —
as its server, and nothing needs the generality.

## Decision 2 — the ladder: override → bundled → PATH, steered by a setting

`discover::locate` returns candidates best-first: the `CIDE_RA_PATH` env override (wins alone;
set-but-broken is a refusal naming the variable, never a silent fall-through), the sidecar
beside `current_exe()`, then PATH. The user's say is
`InspectionSettings::server_binaries` — per-server `Builtin | System`, absent means Builtin —
which only skips the bundled rung; it is a **map keyed by binary name** rather than a
rust-analyzer boolean so a bundled gopls later costs a UI row and no DTO change. A settings
flip restarts the server everywhere (`DiagnosticsRegistry::restart_everywhere`), because the
alternative is a settings row that lies until the next restart.

There is deliberately **no `--version` handshake** and no marker file. A version probe costs a
spawn per project open, parses prose, and catches only version skew — not the real failure
classes (wrong glibc, corrupt copy, a fork bug at startup). Compatibility is packaging's job
(the sidecar is built from the rev `packaging/rust-analyzer.lock` pins, by the same run that
bundles it); robustness is the supervisor's: a candidate that dies **before** `initialize`
advances to the next rung (`next_candidate`), so a broken sidecar degrades to exactly the
pre-M25 behaviour. A crash **after** the handshake never advances — an OOM-killed fork would
only OOM harder as the stock build indexing the same workspace, and advancing would dodge the
three-crashes-in-five-minutes give-up that exists for precisely that machine.

## Decision 3 — configuration is gated on provenance

`cide_lsp::config::init_options` produces a config only for `Provenance::Bundled | Override`.
A stock PATH rust-analyzer — including one chosen via the System setting — gets byte-for-byte
the handshake it always got, because cide cannot know what version it is or which keys it would
misread. The config is sent twice on purpose: as `initializationOptions` and as every
`workspace/configuration` answer, because rust-analyzer reads the first at startup and re-asks
through the second.

The keys are a **two-repo contract with one meeting point per side**: `config.rs` here, the
fork's config module there. A rename on either side does not error — it silently degrades the
fork to its defaults, which looks exactly like the feature working. That is why the vocabulary
lives in one commented function and starts minimal: `cide.diskIndex.dir`, the per-profile cache
directory (`cide_core::persist::cache_dir()/rust-analyzer`). **Ownership split:** cide names
and creates the directory (it must move with the profile); the fork owns everything inside it,
including eviction. Nothing in cide may treat that directory's contents as data it manages.

## Decision 4 — packaging pins the fork; absence is a failure

`packaging/rust-analyzer.lock` (`CIDE_RA_URL`/`CIDE_RA_REV`, shell-sourceable) is the one pin
read by xtask's preflight, the generated Flatpak manifest, and release.yml. Preflight **fails**
when `../forks/rust-analyzer` is absent — unlike `../cide-marketplace`'s test skip — because a bundle
built without the fork ships without it and then *works*, on the PATH fallback, so nobody ever
notices what the package is missing. A wrong-rev sibling only warns: a local `--run` on a
work-in-progress fork is legitimate, and release builds always check out the exact pin.

## Decision 5 — the disk tier reloads only what shallow-verifies

Phase 3 adds a second file beside the snapshot: `memos.redb`, a key-value store of every
persistable memo the LRU caps evict, written at eviction and consulted on a fetch that finds
nothing usable in RAM. Two rules bound it:

- **Same validity ride as the snapshot.** The store carries a stamp (`FORMAT_VERSION` + build
  stamp) and is deleted unless this process *also* restored the snapshot — its keys are salsa's
  positional ingredient indices and its values carry revision numbers, and without the
  snapshot's restored revision counters neither can be verified. A store from another epoch is
  worse than an empty one.
- **A stored memo is reinstalled only when it shallow-verifies** — no input of its durability
  changed since it was verified. One that would need deep verification is dropped and the query
  recomputed, for soundness (deep verification walks edges that may name ids of non-persisted
  ingredients whose restored table pages are untyped placeholders — touching one panics, and no
  ordering of `persist` attributes rules that out while the persist set is partial) and for
  economics (those edges' memos — parse, above all — are absent in a restored process, so
  "verifying" means re-executing the expensive substrate the reload was meant to skip).

The consequence worth stating: **the idle save runs before GC and the LRU sweep**, so the
snapshot carries the whole resident set and the store only ever serves what memory pressure
pushed out between saves. Saving after eviction was measured (the analysis-stats harness still
does it, deliberately): the snapshot loses the evicted values and a warm start pays recompute
for all of them.

Later refinements on the same decision: the store expires (a save-generation epoch, entries
untouched for 12 generations dropped) and compacts (`Database::compact()` at the sweep, whose
commit is the store's one durable commit per save — redb pins `Durability::None` transactions
until a durable one lands, so a never-durable store can never compact); snapshot and store
values are named MessagePack, not JSON (format 3 — named mode deliberately, because
struct-as-array breaks any type whose Serialize/Deserialize halves have different authors);
and the four def-map/item-tree caps arrive from cide as
`cide.diskIndex.lru.{fileItemTree,blockItemTree,crateDefMap,blockDefMap}` — a single
"index working set" percentage on cide's side, scaled off the compiled defaults in the one
producer function, because stored absolutes would pin stale ratios across a fork retune.
Inference is explicitly *outside* the persist set: `InferenceResult` holds `rustc_type_ir`
arena-interned types with no serde surface, so hir-ty's memory story is LRU caps plus the GC
tick's arena collection, and its disk story is none.

## Decision 6 — the second server: gopls, and what it proved

M26 adds gopls to the `BUNDLED` table: sidecar `cide-gopls`, override `CIDE_GOPLS_PATH`,
built from a pinned golang/tools checkout (`../forks/tools`, branch `cide` from a gopls
release tag, `packaging/gopls.lock`). The promises this ADR made about generality were
tested and held: the ladder, the start-failure fallback, the Built-in/System map, the
restart-on-flip reaction and the RSS watchdog all took the second server with **zero
mechanism changes** — one table row, one settings row, and packaging.

What was genuinely new is a **second configuration lane**: `cide_lsp::config::extra_env`,
environment variables beside `init_options`, because gopls's cache location is an env var
(`GOPLSCACHE`), not an LSP setting. Same provenance gate, same one-producer rule, applied
after `child_env::prepare_command` so the scrub cannot undo it. The shipped gopls keeps its
file cache in `persist::cache_dir()/gopls` — per-profile, cide names the directory, gopls
owns the contents *and the eviction budget* (it garbage-collects itself). A System gopls is
deliberately untouched: its machine-global cache is shared with the user's other editors and
is not cide's to move.

The honest scope, which shaped the phasing: upstream gopls has persisted its *derived*
per-package indexes since v0.12 — the core of what the rust-analyzer fork had to build. What
it still lacked was the **metadata graph** (the whole-workspace `go list` result: recomputed
every start), and the measure-first gate — the rust-analyzer road's lesson — held: at
medium scale upstream's caches already deliver, and only kubernetes-scale (5,771 packages,
~5.7 s of warm blocking `go list`) justified the patch. The fork's one patch persists the
graph into gopls's own filecache and **serves-then-reconciles**: a seeded start answers
immediately and still runs the real load off the critical path, reconciling differences
through the ordinary invalidation machinery — never trusting the cache outright, never
blocking on it. Kubernetes warm start: 5.2 s → 0.495 s. The in-memory-layer caps remain
unattempted (not the measured wall), and the lock keeps pinning the upstream tag until the
fork has a remote.

## What lost

- **A distinct registry name** (`cide-rust-analyzer` end to end) — see Decision 1.
- **A `--version` handshake** — see Decision 2.
- **A rust-analyzer-only boolean setting** — the per-server map is what lets gopls join later.
- **A settings field holding a binary path** — that is a spawn-anything surface in a file
  extensions can patch; the developer escape hatch is `CIDE_RA_PATH`, which never touches disk.
- **Vendoring the fork into this workspace** — CI cost, clippy cost, and upstream rebases
  churning this repository's history.

## Consequences

- Every packaged target now requires the sibling checkout at build time; CI does not.
- The tarball and AppImage grow by a ~50 MiB release rust-analyzer; the README's old
  "a tenth the size" ratio is retired.
- Upstream tracking is deliberate: the fork rebases onto release tags, the lock moves only when
  packaging passes. If upstream ships persistence itself, the fork retires and this
  infrastructure — ladder, setting, provenance-gated config, packaging pin — is exactly what a
  configured stock rust-analyzer would still need.
