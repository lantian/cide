# The forks

cide ships two binaries it did not write — `cide-rust-analyzer` and `cide-gopls` — and one of
them is built against a third repository nobody sees. This file is the record of what those
repositories are, how a build reaches them, and what has to happen when one of them moves.

`docs/adr/0011` argues *why* rust-analyzer is forked at all (Decision 5 for the salsa half,
Decision 6 for gopls). This file is the mechanics.

## The three

| checkout | upstream | branch | what cide's copy adds |
| --- | --- | --- | --- |
| `../forks/rust-analyzer` | `rust-lang/rust-analyzer` | `cide` | the disk index — a persisted salsa database, its memo tier, and two memory levers |
| `../forks/salsa` | `salsa-rs/salsa` | `cide` | tracked-struct and memo persistence the disk index cannot exist without |
| `../forks/tools` | `golang/tools` (gopls is its `gopls/` module) | `cide` | a persisted metadata graph, so a warm gopls start stops paying the `go list` wall |

All three are ordinary GitHub forks with two remotes: `origin` is cide's fork, `upstream` is
the project it was forked from. They are **never workspace members** — 400k lines of
rust-analyzer joining every `cargo build --workspace` was the option that lost — so nothing in
CI compiles them and a dev build ignores them entirely. Only `cargo xtask package` needs them.

**The two-sibling rule is load-bearing.** `../forks/rust-analyzer/Cargo.toml` carries

```toml
[patch.crates-io]
salsa = { path = "../salsa" }
```

and cargo resolves that path against *the fork's own manifest*, not against this repository.
So rust-analyzer and salsa must be siblings of each other wherever the pair lives. Everything
that names a directory here — the preflight, `scripts/clone-forks.sh`, `release.yml`, the
Flatpak manifest's `dest:` keys — is arranged to keep that true, and each of them says so
where it would otherwise look arbitrary.

### salsa is a fork, and was not always

Until 2026-08-28 `../forks/salsa` was the crates.io 0.28.2 *tarball* with a `git init` over
it: seven commits sharing no ancestor with `salsa-rs/salsa`, so `git merge upstream` was
impossible and a version bump was a re-application by hand. It was also carrying 361 MiB of
committed `target/` in two of those commits.

It is now a real fork: the six cide patches cherry-picked onto the `salsa-v0.28.2` tag, with
`src/` and `tests/` byte-identical across the move. Only `Cargo.toml` had to be re-expressed,
because a published tarball's manifest is cargo's *normalised rendering* of the one in git —
the deps come out as `[dependencies.x]` tables and the workspace inheritance is flattened.

That layout difference reaches one file in the consumer. In git, `salsa` depends on
`salsa-macros` and `salsa-macro-rules` through `components/` paths where the tarball took them
from the registry; `[patch.crates-io]` names only `salsa`, so those two now resolve through
the patched crate's own manifest and lose their `source`/`checksum` lines in the
rust-analyzer fork's `Cargo.lock`. Every build of that fork passes `--locked`, so the lockfile
had to move with it — which is the shape of this whole file: a change in one repository is
only finished when the other two agree.

## The pins

Each fork is pinned by a lock file in `packaging/`:

```
packaging/rust-analyzer.lock   CIDE_RA_URL     CIDE_RA_REV
packaging/salsa.lock           CIDE_SALSA_URL  CIDE_SALSA_REV
packaging/gopls.lock           CIDE_GOPLS_URL  CIDE_GOPLS_REV
```

`KEY=VALUE`, `#` comments, blank lines ignored — the subset of shell that is also trivially a
config format, because there are four consumers and they must read the same bytes:

1. `cargo xtask package` preflight (`read_pins`), which compares each lock against the local
   checkout's HEAD;
2. the generated Flatpak manifest, which embeds all three as `type: git` sources;
3. `.github/workflows/release.yml`, which sources the files as shell and fetches each
   revision;
4. `scripts/clone-forks.sh`, so a contributor's checkout and a release's are the same thing.

Each lock uses **its own key names**. Release.yml sources all three into one shell, and a
shared `CIDE_URL` would have the last `source` silently clobber the earlier pins.

### Pin with a tag, and prefix it with `cide-`

A rev may be a tag or a 40-hex commit, and it should be a tag:

- `checkout-fork` fetches it with `git fetch --depth 1 <url> <rev>`. GitHub serves a reachable
  commit id there, but "reachable" is a property somebody else can revoke by rewriting a
  branch; a tag is a ref.
- The Flatpak manifest has to say `tag:` or `commit:` — flatpak-builder refuses each under the
  other — so the generator sniffs the rev's shape (`ra_source_key`). Both work; only one reads.
- The rev is stamped into the built binary as `CFG_RELEASE` / `-X main.version`, which is what
  a bug report quotes. A name reads there and a hash does not.

The `cide-` prefix is not decoration. `git fetch upstream --tags` inside a fork brings
upstream's whole tag namespace with it, so a pin living in that namespace is one upstream can
move out from under a release. Current names: `cide-2026-08-28`, `cide-salsa-v0.28.2`,
`cide-gopls-v0.23.0`; a second pin against the same base takes a `.1` suffix.

The binaries are stamped `<rev>+cide` — so a report about `rust-analyzer 2026-08-24` is
upstream's and one about `2026-08-24+cide` is ours — except that a rev already naming cide is
left alone rather than stamped twice (`cide_version_stamp`). Both packaging channels use the
same helper: one pin producing two packages that answer `--version` differently is a support
conversation with no bottom.

## Publishing a fork

Fork on GitHub first (the Fork button on the upstream repo), so the network relationship
exists and pushing the `cide` branch uploads only its own objects. Then repoint the remotes —
a checkout made by cloning upstream has them the wrong way round:

```sh
cd ../forks/rust-analyzer
git remote rename origin upstream
git remote add origin git@github.com:<you>/rust-analyzer.git
git push -u origin cide
git tag -a cide-2026-08-28 -m "cide pin: ..." && git push origin cide-2026-08-28
```

Then, in each fork's GitHub settings:

- **Disable Actions.** Upstream's workflows are inherited by the fork and would fire on every
  push to `cide`. rust-analyzer's and golang/tools' CI are neither cheap nor yours.
- Disable Issues, Wiki and Projects. Bug reports belong on cide or upstream, not here.
- Set the default branch to `cide`, so the landing page shows the work and its `CLAUDE.md`
  rather than a stale mirror of upstream.

**All three forks must be public**, whatever cide itself is. `checkout-fork` fetches
anonymously and the release job's `GITHUB_TOKEN` is scoped to the cide repository alone — it
cannot read a private sibling. A private fork means threading a PAT or App token through every
checkout step, in two jobs, for no benefit: the code is upstream's, under upstream's licence,
plus patches whose reasoning is written down in the open anyway.

Note that forks of a public repository cannot themselves be made private, so this is a
constraint on the choice, not a consequence of it.

## Moving a pin

The rule the lock comments state and this file repeats: **a pin moves only after a packaged
build has been exercised against it.** In order:

1. Do the work in the fork, on `cide`. For a version bump this is
   `git fetch upstream --tags && git rebase <new-tag>`, following that fork's own `CLAUDE.md`
   — the rust-analyzer one is a merge-survival guide naming six invariants that break with no
   compile error, and the salsa one is a porting guide whose central claim is that a patch
   with an upstream equivalent should be *retired*, not carried.
2. Push the branch and a new `cide-*` tag. Never move a published tag; a pin that changes
   meaning is worse than one that is out of date, because nothing in the system compares two
   builds of the same rev.
3. Move the lock in this repository, and run `cargo --locked xtask package --write` — the
   Flatpak manifest embeds the pin and `--check` fails on the stale copy.
4. `cargo --locked xtask package --appimage --tarball` (no `--run`) prints the preflight: each
   fork should report *is at the pinned revision*. A mismatch is only a warning, because a
   local build against a work-in-progress fork is a legitimate thing to do; a release always
   checks out the pinned rev, so it never sees it.
5. Build the artefact and run it before the lock lands.

salsa and rust-analyzer move **in one review** or not at all. The `[patch.crates-io]` only
applies while salsa's `version` matches what the rust-analyzer workspace asks for, and a
mismatch does not fail — cargo quietly resolves the registry crate instead, and the disk index
then fails at runtime for reasons that look nothing like a version skew.

## What CI does with all this

- **`ci.yml`'s `fork-pins` job** runs `git ls-remote <url> <rev>` for each lock. Seconds, no
  clone, anonymous exactly as the release fetch is anonymous. It exists because every way a
  pin stops resolving is somebody else's action on somebody else's repository — a tag
  force-moved, a `cide` branch rewritten so the commit it named is orphaned, a fork gone
  private or renamed — so nothing here changes and the lock keeps looking correct until a
  release run is ten minutes in.
- **`release.yml`** checks out all three forks in both the `linux` and `macos` jobs, through
  the `checkout-fork` composite action, at exactly the pinned revisions. rust-analyzer and
  gopls cache their build output; salsa does not, because it is a path dependency compiled
  into the rust-analyzer fork's `target/`, which that fork's own cache entry already carries.
- **Nothing else touches them.** `ci.yml`'s build, test and clippy jobs never see a fork —
  that is the point of keeping them out of the workspace.

## When something goes wrong

| symptom | what it means |
| --- | --- |
| preflight: *the rust-analyzer fork patches salsa to its sibling ../salsa, which is not checked out* | run `./scripts/clone-forks.sh` |
| preflight: *`../forks/salsa` is at `abc123` but `packaging/salsa.lock` pins ...* | a local build will not match a release's; fine while iterating, not fine when the lock lands |
| `fork-pins` fails in CI | the tag the lock names no longer resolves. Push it, or move the lock to a revision that exists |
| the release build dies inside cargo's patch resolution | the salsa checkout is missing or is not the rust-analyzer fork's sibling |
| `cargo` refuses with *the lock file needs to be updated but --locked was passed* in a fork | the fork's `Cargo.lock` has not absorbed a dependency-layout change. Commit the regenerated lockfile to the fork and re-pin |
| a packaged rust-analyzer reports a version with `+cide` twice | `cide_version_stamp` was bypassed by a new build path |
