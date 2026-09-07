//! Which models cide's agents run on: the providers cide configures, and the ordered pools a
//! run falls down. (M45)
//!
//! # Why cide owns provider configuration when it owns so little else
//!
//! [`cide_agents::harness::opencode`]'s header lists three things its configuration document
//! deliberately does **not** carry — `tools`, `permission`, `variant` — and every absence has one
//! reason: cide would be inventing a policy and attributing it to a file somebody else wrote. A
//! provider is the opposite case, which is why it is here. Nobody else wrote it. What this module
//! emits is a transcription of what the user typed into cide's own Settings screen, with no
//! vocabulary being translated and no restriction being invented.
//!
//! # Three kinds of provider, because opencode already knows about 213 of them
//!
//! opencode ships the models.dev catalog (cached at `$XDG_CACHE_HOME/opencode/models.json` — 213
//! providers on the machine this was written on) carrying each provider's npm package, API base
//! URL, credential environment variables and full model list. Asking a user to retype
//! `@openrouter/ai-sdk-provider` and 360 model ids that opencode already has would be a form that
//! is wrong the week after it is filled in. So:
//!
//! * [`LlmProvider::Catalog`] — an id and a key. cide writes the credential and **nothing else**;
//!   npm, base URL and every model come from opencode's own catalog.
//! * [`LlmProvider::Custom`] — an OpenAI-compatible endpoint the catalog has never heard of
//!   (`ollama` is not in it; an `unsloth` GGUF behind a local llama-server is another). Here the
//!   full description is required, models included, because a provider that exists only in a
//!   configuration must declare its models or `--model id/model` resolves to nothing.
//! * [`LlmProvider::External`] — a provider **somebody else installs**, for which cide writes not
//!   one byte. See that variant's own doc: it is the `openspec` arrangement, applied a second
//!   time.
//!
//! # Global, unlike everything about a project
//!
//! [`crate::OrchestrationConfig`]'s doc argues that "this project runs subagents" belongs in the
//! checkout. The mirror image holds here: an API key is a property of the *person*. It must not be
//! committed, must not be reviewable in a pull request, and must follow the user into every
//! project — which is what [`crate::Settings`] is and `.cide/config.json` is not.
//!
//! **Which** pool a role uses is neither: it lives in a per-project *local override* file under
//! the profile's config directory, so it is not committed either. That is the whole reason a pool
//! name never needs to survive somebody else's clone.
//!
//! # Only the opencode harness reads any of this
//!
//! `harness/claude.rs` suppresses `--model` for a subagent entirely, codex has its own catalog,
//! and qwen has no provider vocabulary cide speaks. A pool named for a role on another harness is
//! reported and otherwise ignored — never a silent substitution.

use std::fmt;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Every provider cide knows how to configure, and every pool built out of them.
///
/// Two flat lists rather than maps keyed by id. The user types both keys, and a map would rewrite
/// the key on every keystroke and lose the row the moment two of them were briefly equal — the
/// argument [`crate::ClaudeCli::env`] already makes about `Vec<ClaudeEnvVar>` against `BTreeMap`.
/// For [`ModelPool::entries`] a map would also destroy the order, which is the entire feature.
///
/// This rides every `cide://workspace-changed` to every window, like all of [`crate::Settings`],
/// so it must stay small — tens of rows, not a catalog. That is also why [`LlmProvider::Catalog`]
/// stores no model list: the 360 ids behind `openrouter` belong to opencode's cache, not to
/// cide's settings broadcast.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct LlmSettings {
    /// The providers, in the order the Settings screen draws them.
    pub providers: Vec<LlmProvider>,
    /// The pools. Named from a local agent override, never from a committed file.
    pub pools: Vec<ModelPool>,
}

impl LlmSettings {
    /// The provider with this id.
    ///
    /// Exact match, and no case-insensitive near miss: the id is a JSON key in a document opencode
    /// reads, and the left half of every `provider/model`. A lookup that was cleverer than the
    /// thing it feeds would find a provider the CLI then could not.
    #[must_use]
    pub fn provider(&self, id: &str) -> Option<&LlmProvider> {
        self.providers.iter().find(|provider| provider.id() == id)
    }

    /// The pool with this name, or `None` — which the caller must turn into a **refusal**, never a
    /// fallback. See `cide_agents::pools`.
    #[must_use]
    pub fn pool(&self, name: &str) -> Option<&ModelPool> {
        self.pools.iter().find(|pool| pool.name == name)
    }

    /// Trim what a form cannot. **Drops nothing**, and that is the whole of this function's
    /// interesting behaviour.
    ///
    /// Applied where a patch lands (`cmd::settings::apply_patch`), never on read — the rule
    /// [`crate::SidebarSettings::clamped`] follows, so the stored file converges on a legal value
    /// instead of being repaired forever on every load.
    ///
    /// # Why it must not drop an incomplete row, which is the bug this shape replaces
    ///
    /// It used to drop a provider with no id, a pool with no name and an entry with no model, on
    /// the reasoning that none of them can mean anything. That reasoning was right about the
    /// *values* and wrong about the *screen*, and the result was that **every Add button on the
    /// Models section did nothing at all**.
    ///
    /// ADR 0002 is why: the webview owns no copy of the settings. It sends the whole group to
    /// Rust and redraws from the `cide://workspace-changed` broadcast that comes back. So a row
    /// that Rust refuses to store is a row that can never appear — and a row the user has just
    /// added is, by definition, blank until they type into it. Pressing *Add a known provider*
    /// appended a blank provider, sent it, had it dropped here, and redrew the list without it.
    /// Nothing threw and nothing logged; the button was simply inert.
    ///
    /// Refusing an incomplete row is still right — it just belongs where the value is *used*, not
    /// where it is stored. `cide_agents::harness::opencode::provider_members` already skips a
    /// provider with a blank id and a model with a blank id, so nothing incomplete has ever
    /// reached an opencode document; the drop here was buying nothing and costing the feature.
    ///
    /// `api_key` is deliberately **not** trimmed either. Whitespace in a credential is the user's
    /// to see; silently editing one is how a key that works in a terminal stops working in cide.
    ///
    /// An entry naming a provider that does not exist is likewise kept: the provider may be about
    /// to be created, and the screen strikes the row through and says so rather than deleting what
    /// somebody typed — `ClaudeCliSection`'s "stored verbatim, filtered on the way out".
    #[must_use]
    pub fn cleaned(mut self) -> Self {
        for provider in &mut self.providers {
            provider.clean();
        }
        for pool in &mut self.pools {
            pool.name = pool.name.trim().to_string();
            pool.description = pool.description.trim().to_string();
            for entry in &mut pool.entries {
                entry.provider = entry.provider.trim().to_string();
                entry.model = entry.model.trim().to_string();
                entry.variant = entry.variant.trim().to_string();
            }
        }
        self
    }
}

/// One provider, in the shape that decides what cide writes for it.
///
/// # A tagged union rather than one wide struct
///
/// The repo's precedent for a shape whose arms genuinely differ is [`crate::AgentRoster`],
/// [`crate::AgentSaveOutcome`] and [`crate::RunState`]. A single struct with a `mode` field plus
/// every field present would make three illegal states representable, and all three are silent: a
/// `Catalog` row carrying a stray `npm` that overrides the catalog; a `Custom` row with no
/// `models`, which cannot resolve `--model id/model` at all; and an `External` row carrying a
/// credential cide has promised never to write. As an enum, `provider_members` is an exhaustive
/// `match`, so a fourth kind is a compile error rather than a provider that quietly emits nothing.
///
/// # Why `Debug` is written by hand
///
/// Two variants carry a credential. [`crate::Settings`] derives `Debug`, so a
/// `tracing::debug!(?settings)` anywhere in the app would print it, and there is no way to review
/// every future call site — the exact argument [`crate::ProxySettings`] makes about a proxy
/// password and [`crate::ClaudeCli`] makes about a user-supplied environment. Ids, labels and base
/// URLs survive; the key does not. Which provider a run reached and what its base URL was is most
/// of any answer to "why did this run fail", and none of the risk.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum LlmProvider {
    /// A provider **opencode's own catalog already describes**: openrouter, deepseek, openai,
    /// google, groq, mistral — 213 of them on the machine this was written on.
    ///
    /// cide supplies the credential and nothing else. A configuration declaring only
    /// `provider.<id>.options.apiKey` still loads the provider from the catalog and offers every
    /// catalog model, because the config is deep-merged **over** the catalog entry rather than
    /// replacing it. So `npm`, `baseURL` and the model list are absent here on purpose: cide
    /// storing a copy of them would be cide pinning a package version and a model list that
    /// opencode updates on its own, and getting them wrong the week after.
    Catalog {
        /// The provider id as the catalog spells it, and the left half of `provider/model`.
        ///
        /// Not an enum, and not validated against the catalog. The cache file it would be
        /// validated against is a *cache path* — absent on a fresh install, and moveable by an
        /// opencode release — so a refusal built on it would refuse a provider that works. The
        /// Settings screen suggests ids and accepts anything, which is the same asymmetry
        /// [`crate::AgentModels`]'s doc states: a menu, not a rule.
        #[serde(default)]
        id: String,
        /// What the Settings screen calls it. Blank means the id speaks for itself, and cide
        /// writes no `name` key — the catalog already has one.
        #[serde(default)]
        label: String,
        /// Whether cide writes anything for this provider. See [`LlmProvider::default`] for why
        /// the hand-written `Default` is load-bearing.
        #[serde(default = "yes")]
        enabled: bool,
        /// `provider.<id>.options.apiKey`.
        ///
        /// **Blank is a legitimate, common answer and not an unfinished row.** A cide launched
        /// from a shell that exports `OPENROUTER_API_KEY` already gives every child that variable
        /// — `cide_core::child_env::prepare_command` touches only `$APPDIR` values and `PATH`, so
        /// everything else is inherited wholesale, the same mechanism [`crate::ProxyTarget`]'s doc
        /// explains about an inherited `HTTPS_PROXY` — and opencode reads the catalog's own env
        /// names at highest precedence. So the honest default for this box is empty, the screen
        /// says "leave it blank if it is already in your environment", and the model probe is what
        /// says whether that worked.
        #[serde(default)]
        api_key: String,
    },

    /// An OpenAI-compatible endpoint **the catalog has never heard of**: ollama, a llama.cpp
    /// server, an unsloth GGUF behind LM Studio.
    ///
    /// The full description is required here and that is not a preference: a provider that exists
    /// only in a configuration must declare its `models` map or `--model <id>/<model>` resolves to
    /// nothing at all.
    Custom {
        #[serde(default)]
        id: String,
        #[serde(default)]
        label: String,
        #[serde(default = "yes")]
        enabled: bool,
        /// `provider.<id>.npm`. Defaulted in the form to `@ai-sdk/openai-compatible`, which is
        /// what a local OpenAI-compatible endpoint wants, but stored rather than assumed — an
        /// endpoint wanting a different SDK is exactly the case this variant is for.
        #[serde(default)]
        npm: String,
        /// `provider.<id>.options.baseURL` — `http://localhost:11434/v1` for ollama,
        /// `http://127.0.0.1:1234/v1` for LM Studio.
        ///
        /// Spelled `baseURL` **with that capitalisation** in the document opencode reads, which is
        /// the key its schema declares under `additionalProperties: false` — so `baseUrl` is not a
        /// typo that degrades, it can invalidate the object. The Rust field is `base_url` and
        /// serde renames it to `baseUrl` for *cide's own* wire; opencode's spelling is applied
        /// once, in `provider_members`, and nowhere else.
        #[serde(default)]
        base_url: String,
        /// `provider.<id>.options.apiKey`. Blank is the ordinary state for a local endpoint.
        #[serde(default)]
        api_key: String,
        /// `provider.<id>.models`. Required — see the variant's own doc.
        #[serde(default)]
        models: Vec<LlmModel>,
    },

    /// A provider **somebody else installs**, which cide describes, verifies and never writes.
    ///
    /// # This is the `openspec` arrangement, and it is deliberately the second one
    ///
    /// `CLAUDE.md` calls the `openspec` CLI "the one dependency cide neither ships nor bundles":
    /// `cide_spec::discover` finds it, and a machine without it gets a panel that says so and
    /// names the install command — detected and explained, never silently arranged. This is that
    /// arrangement a second time, and it should read like the first.
    ///
    /// The concrete case is `opencode-openai-codex-auth`, which exposes a ChatGPT/codex
    /// subscription as the opencode provider `openai` with models like `gpt-5.2` and
    /// `gpt-5.1-codex-max`. It is an opencode **plugin**, and cide must not write opencode's
    /// `plugin` array: `~/.config/opencode/package.json` is opencode-managed (it carries
    /// `@opencode-ai/plugin` with a populated `node_modules` beside it), so listing a plugin makes
    /// opencode **install it from npm** — which would turn pressing Dispatch into a package fetch
    /// that stalls the first turn and fails outright with no network. An IDE does not install
    /// third-party packages as a side effect of starting a subagent.
    ///
    /// So this variant exists for exactly two jobs, and neither writes anything: a pool entry may
    /// **name** the provider (which is what makes a subscription-backed model poolable without
    /// cide configuring it), and the screen can **explain the steps** and then say whether they
    /// worked.
    External {
        /// The provider id the plugin registers — `openai` for the codex plugin.
        #[serde(default)]
        id: String,
        #[serde(default)]
        label: String,
        /// What the user has to do, as prose cide shows and does not act on.
        ///
        /// Stored rather than derived from the id, because cide has no table of plugins and must
        /// not grow one — that table would go stale in the direction that tells a user to install
        /// the wrong thing. The screen seeds it from a preset and it is editable prose thereafter.
        #[serde(default)]
        setup: String,
        /// Model ids whose presence in `opencode models` proves the setup worked — e.g.
        /// `openai/gpt-5.2`. Empty means "any id under `<id>/` counts", which is the right answer
        /// for a plugin whose catalog changes with the subscription.
        #[serde(default)]
        expect: Vec<String>,
    },
}

/// `true`, for the `enabled` fields' `#[serde(default)]`.
///
/// Serde's own default for a `bool` is `false`, and a provider that deserialised disabled is the
/// hazard [`LlmProvider::default`] spells out. Every field of every variant carries a
/// `#[serde(default)]` so that a hand-edited or older object *parses* at all — a missing key must
/// fill in, never fail the whole `Settings` load — and this is the one whose fill-in is not the
/// type's own zero.
fn yes() -> bool {
    true
}

impl Default for LlmProvider {
    /// A blank catalogued provider, **enabled**, which is what the Add button produces.
    ///
    /// `enabled: true` is the load-bearing line and the reason this is written out rather than
    /// derived. `bool::default()` is `false`; the container carries `#[serde(default)]`; so a
    /// provider object hand-edited without the key, or written by a build that predates it, would
    /// deserialise **disabled** — every run silently falling to another provider or refusing, from
    /// a screen the user never opened. [`crate::ClaudeInjection`]'s doc calls its own version of
    /// this the most dangerous line in that file. This is the same line.
    fn default() -> Self {
        Self::Catalog {
            id: String::new(),
            label: String::new(),
            enabled: true,
            api_key: String::new(),
        }
    }
}

impl LlmProvider {
    /// The provider id, whichever kind this is.
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::Catalog { id, .. } | Self::Custom { id, .. } | Self::External { id, .. } => id,
        }
    }

    /// What to call it on screen, falling back to the id so no caller has to.
    #[must_use]
    pub fn label(&self) -> &str {
        let label = match self {
            Self::Catalog { label, .. }
            | Self::Custom { label, .. }
            | Self::External { label, .. } => label,
        };
        match label.is_empty() {
            true => self.id(),
            false => label,
        }
    }

    /// Whether cide writes anything for this provider.
    ///
    /// An [`Self::External`] provider is always "on": there is nothing for a switch to gate,
    /// because cide writes nothing for it either way. Answering `true` keeps every caller — the
    /// pool editor's provider list, the entry verdicts — from needing a fourth state.
    #[must_use]
    pub fn enabled(&self) -> bool {
        match self {
            Self::Catalog { enabled, .. } | Self::Custom { enabled, .. } => *enabled,
            Self::External { .. } => true,
        }
    }

    /// The credential, or `""` for a variant that has none. Never printed by [`fmt::Debug`].
    #[must_use]
    pub fn api_key(&self) -> &str {
        match self {
            Self::Catalog { api_key, .. } | Self::Custom { api_key, .. } => api_key,
            Self::External { .. } => "",
        }
    }

    /// Trim every field a form cannot, and drop model rows that name nothing.
    ///
    /// `api_key` is untouched — see [`LlmSettings::cleaned`].
    fn clean(&mut self) {
        match self {
            Self::Catalog { id, label, .. } => {
                *id = id.trim().to_string();
                *label = label.trim().to_string();
            }
            Self::Custom {
                id,
                label,
                npm,
                base_url,
                models,
                ..
            } => {
                *id = id.trim().to_string();
                *label = label.trim().to_string();
                *npm = npm.trim().to_string();
                *base_url = base_url.trim().to_string();
                for model in models.iter_mut() {
                    model.id = model.id.trim().to_string();
                    model.label = model.label.trim().to_string();
                }
                // Not dropped, for `LlmSettings::cleaned`'s reason: "Add model" appends a blank
                // row, and a row Rust refuses to store is a row that can never be typed into.
                // `provider_members` skips a blank model id at emission.
            }
            Self::External {
                id,
                label,
                setup,
                expect,
            } => {
                *id = id.trim().to_string();
                *label = label.trim().to_string();
                *setup = setup.trim().to_string();
                for want in expect.iter_mut() {
                    *want = want.trim().to_string();
                }
                // These come from splitting one comma-separated box rather than from an Add
                // button, so an empty fragment is punctuation the user typed and not a row they
                // created — dropping it is what they meant.
                expect.retain(|want| !want.is_empty());
            }
        }
    }
}

impl fmt::Debug for LlmProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Every field named explicitly per arm, and the hazard `ProxySettings` carries verbatim: a
        // field added above and forgotten here vanishes from every debug print rather than failing
        // to compile. `a_provider_debug_print_names_every_field` is the guard, and it reads the
        // list off the *serialized* value so it cannot fall behind either.
        match self {
            Self::Catalog {
                id,
                label,
                enabled,
                api_key,
            } => f
                .debug_struct("Catalog")
                .field("id", id)
                .field("label", label)
                // Half of any answer to "why did this run go to that model".
                .field("enabled", enabled)
                .field("api_key", &redacted(api_key))
                .finish(),
            Self::Custom {
                id,
                label,
                enabled,
                npm,
                base_url,
                api_key,
                models,
            } => f
                .debug_struct("Custom")
                .field("id", id)
                .field("label", label)
                .field("enabled", enabled)
                .field("npm", npm)
                // A URL, and — unlike a proxy's — one that carries no userinfo in any shape cide
                // writes, because the credential has its own field. Printed as it stands.
                .field("base_url", base_url)
                .field("api_key", &redacted(api_key))
                // Ids and limits, not secrets, and the list a reader needs in order to see whether
                // the model a run asked for was declared at all.
                .field("models", models)
                .finish(),
            Self::External {
                id,
                label,
                setup,
                expect,
            } => f
                .debug_struct("External")
                .field("id", id)
                .field("label", label)
                .field("setup", setup)
                .field("expect", expect)
                .finish(),
        }
    }
}

/// `""` or `<redacted>`, and never the bytes.
///
/// [`crate::ClaudeEnvVar`]'s `Debug` makes the same choice for the same reason: a `Vec<T>`'s
/// `Debug` prints each element with `T`'s, so a redaction has to live at the leaf or it lives
/// nowhere. Distinguishing empty from set is what makes the line useful — "this provider has no
/// key" and "this provider's key is wrong" are different failures.
fn redacted(key: &str) -> &'static str {
    match key.is_empty() {
        true => "\"\"",
        false => "<redacted>",
    }
}

/// One model id under a [`LlmProvider::Custom`] endpoint.
///
/// A named struct rather than a tuple for [`crate::ClaudeEnvVar`]'s stated reason: a tuple arrives
/// in TypeScript as a positional array, one transposition away from writing the label into the id.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct LlmModel {
    /// The model id **as the endpoint serves it**, and the right half of `provider/model`.
    ///
    /// It may itself contain `/` (`openai/gpt-oss-20b` under `lmstudio`) and `:`
    /// (`Qwen3.8-27B-GGUF:UD-Q4_K_M`), which is exactly why [`PoolEntry`] keeps the two halves
    /// apart rather than storing one string it would have to split.
    pub id: String,
    /// `models.<id>.name`. Blank means the id speaks for itself and no key is written.
    pub label: String,
    /// `models.<id>.limit.context`, in tokens. `0` means "do not write one".
    ///
    /// # Why this is here when cide does nothing with the number
    ///
    /// Because opencode does. A catalogued model's limits come from the catalog; a model that
    /// exists **only** in a configuration has whatever that configuration says, and opencode's
    /// compaction is computed against it. A user who moves such a provider out of their
    /// hand-written `opencode.jsonc` into cide, and finds this missing, has silently lost the
    /// tuning their compaction depends on.
    ///
    /// The subtler half, worth knowing before deciding this field is redundant: cide's document
    /// deep-merges, so a user who keeps *both* declarations keeps their limits regardless — cide's
    /// model object only overrides the keys it writes. This field is for the user who stops
    /// keeping both.
    pub context: u32,
    /// `models.<id>.limit.output`. `0` means "do not write one". See [`Self::context`].
    pub output: u32,
}

/// A named, ordered list of models a run falls down. (M45)
///
/// # Ordered, and the order is the whole feature
///
/// A pool is not a set of equivalent models; it is a preference with fallbacks. The first entry is
/// what a run gets, and the rest are what it gets when that one is rate-limited, unreachable or
/// refuses to authenticate. So this is a `Vec` and the Settings screen has explicit move controls:
/// a sorted or set-shaped store would silently destroy the one thing the user is expressing.
///
/// # Selection is sticky for a run
///
/// An opencode run is one process per turn (`harness::opencode`'s header). A run that re-chose on
/// every turn would be a conversation whose turns were answered by different models with nothing
/// recording which. The choice therefore lives on the run as [`PoolChoice`], and only a failover
/// moves it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ModelPool {
    /// The name a local agent override refers to.
    ///
    /// A pool that an override names and that does not exist is a *refused dispatch* with a
    /// sentence — never a silent fallback to the provider default, which would be a run answered
    /// by a model nobody chose and billed to somebody who never agreed to it. That refusal is
    /// affordable precisely because both halves are local: the person who made the inconsistency
    /// is the person looking at the screen.
    pub name: String,
    /// One line for the person choosing — "cheap first, then the subscription". Not prompt text;
    /// nothing a model ever sees.
    pub description: String,
    /// The entries, best first.
    pub entries: Vec<PoolEntry>,
}

/// One `provider/model` in a pool, with the variant it should run at.
///
/// # Three fields and not one `provider/model` string
///
/// A model id **contains slashes**: `lmstudio/openai/gpt-oss-20b` is a real id, and
/// `unsloth/Qwen3.8-27B-GGUF:UD-Q4_K_M` carries a colon as well. A stored flat string would need a
/// split rule to get the provider back out — a parser cide would have to invent, own, and get
/// wrong at the first id that broke the assumption. Stored apart, cide only ever *joins*, which
/// cannot be wrong, and `--variant` (a separate flag on the argv) has somewhere to live.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct PoolEntry {
    /// An [`LlmProvider::id`] — including an [`LlmProvider::External`] one.
    pub provider: String,
    /// A model id under that provider.
    ///
    /// cide refuses nothing here: for a catalogued or external provider it holds no list to refuse
    /// against, and for a custom one the list is a shortlist. [`crate::AgentModels`]'s rule — a
    /// menu, not a rule.
    pub model: String,
    /// `--variant`. Blank means the model's own default.
    ///
    /// **When set it wins over the role's `effort:`**, and that is a decision rather than an
    /// accident. A pool entry names a model *and how hard it should think* as one selectable
    /// target — `gpt-5.1-codex-max` at `xhigh` is a different target from the same model at `low`
    /// — while a role's `effort:` was written for whichever model the role itself named. The role
    /// keeps its `effort:` for every entry that leaves this blank, so the only loser is an entry
    /// that explicitly disagreed.
    ///
    /// The concrete failure this prevents: each model declares its own variant set (`gpt-5.2`
    /// offers none/low/medium/high/xhigh, `gpt-5.1-codex-mini` only medium/high), so a role-level
    /// `effort: xhigh` carried onto the candidate a rate limit fell over to would be refused
    /// outright by the CLI — turning a recoverable failure into a hard one at the worst moment.
    pub variant: String,
}

impl PoolEntry {
    /// The value `--model` takes: `provider/model`.
    ///
    /// The **only** place the two halves are ever joined, so nothing downstream has to know the
    /// shape — and nothing downstream can join them differently.
    #[must_use]
    pub fn model_flag(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }
}

/// The entry a run has settled on, held on the run and reused for every turn. (M45)
///
/// A value rather than an index into [`ModelPool::entries`], because the pool is *global settings*
/// and a user may edit it mid-run: an index would silently re-point at a different model between
/// one turn and the next. Carrying the resolved triple means the only thing that can change a live
/// run's model is a failover.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PoolChoice {
    /// Which pool it came from, for the run's row and for the log line.
    pub pool: String,
    /// How far down the list this run has fallen. `0` is the first choice; anything else is a fact
    /// worth showing, because it means something upstream refused.
    pub index: u16,
    pub entry: PoolEntry,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyed() -> LlmSettings {
        LlmSettings {
            providers: vec![
                LlmProvider::Catalog {
                    id: "openrouter".into(),
                    label: String::new(),
                    enabled: true,
                    api_key: "sk-or-not-a-real-key".into(),
                },
                LlmProvider::Custom {
                    id: "ollama".into(),
                    label: "Ollama".into(),
                    enabled: true,
                    npm: "@ai-sdk/openai-compatible".into(),
                    base_url: "http://127.0.0.1:11434/v1".into(),
                    api_key: "sk-custom-not-a-real-key".into(),
                    models: vec![LlmModel {
                        id: "qwen3:8b".into(),
                        label: String::new(),
                        context: 32768,
                        output: 4096,
                    }],
                },
            ],
            pools: Vec::new(),
        }
    }

    /// The `ProxySettings` guard, per variant: the field list is read off the **serialized** value
    /// so it cannot fall behind a field added to the enum and forgotten in `Debug`.
    #[test]
    fn a_provider_debug_print_names_every_field() {
        for provider in [
            LlmProvider::default(),
            keyed().providers[1].clone(),
            LlmProvider::External {
                id: "openai".into(),
                label: "ChatGPT".into(),
                setup: "add the plugin".into(),
                expect: vec!["openai/gpt-5.2".into()],
            },
        ] {
            let printed = format!("{provider:?}");
            let value = serde_json::to_value(&provider).expect("serializes");
            let fields = value.as_object().expect("a struct");
            assert!(!fields.is_empty());
            for name in fields.keys() {
                // `kind` is serde's tag, not a field of the variant, and has no `Debug` line.
                if name == "kind" {
                    continue;
                }
                // The `Debug` impl uses Rust's own names, serde the camelCase wire ones; comparing
                // on the lower-cased letters alone is what makes `base_url`/`baseUrl` one name.
                let flattened: String = name.chars().filter(char::is_ascii_alphanumeric).collect();
                let printed_flat: String = printed
                    .chars()
                    .filter(char::is_ascii_alphanumeric)
                    .collect::<String>()
                    .to_lowercase();
                assert!(
                    printed_flat.contains(&flattened.to_lowercase()),
                    "`{name}` is missing from the Debug print of {provider:?}"
                );
            }
        }
    }

    /// The assertion that actually protects the user; the one above only protects the impl.
    #[test]
    fn a_debug_print_does_not_carry_the_key() {
        let printed = format!("{:?}", keyed());
        assert!(printed.contains("<redacted>"), "{printed}");
        assert!(!printed.contains("sk-or-not-a-real-key"), "{printed}");
        assert!(!printed.contains("sk-custom-not-a-real-key"), "{printed}");
        // Empty is distinguishable from set: "no key" and "wrong key" are different failures.
        let blank = format!("{:?}", LlmProvider::default());
        assert!(blank.contains("\"\""), "{blank}");
        assert!(!blank.contains("<redacted>"), "{blank}");
    }

    /// A provider object written before the switch existed must not come back **disabled**.
    #[test]
    fn a_provider_written_before_the_enabled_switch_is_still_enabled() {
        let provider: LlmProvider =
            serde_json::from_str(r#"{"kind":"catalog","id":"openrouter"}"#).expect("parses");
        assert!(
            provider.enabled(),
            "a stored provider with no `enabled` key must default to on, or every run silently \
             falls elsewhere from a screen nobody opened"
        );
        // And the reason every field carries `#[serde(default)]`: a partially-written object must
        // *parse*. Without it a hand-edited provider fails the whole `Settings` deserialisation,
        // which is a worse failure than the one above — it loses the user's entire workspace file.
        let bare: LlmProvider =
            serde_json::from_str(r#"{"kind":"custom","id":"ollama"}"#).expect("parses");
        assert_eq!(bare.id(), "ollama");
        assert!(bare.enabled());
        let settings: LlmSettings = serde_json::from_str("{}").expect("parses");
        assert!(settings.providers.is_empty() && settings.pools.is_empty());
    }

    /// The promise is structural, not a habit: there is nowhere on this variant to put a key.
    #[test]
    fn an_external_provider_carries_no_credential_field() {
        let provider = LlmProvider::External {
            id: "openai".into(),
            label: String::new(),
            setup: String::new(),
            expect: Vec::new(),
        };
        let value = serde_json::to_value(&provider).expect("serializes");
        let fields = value.as_object().expect("a struct");
        assert!(!fields.contains_key("apiKey"), "{fields:?}");
        assert_eq!(provider.api_key(), "");
        assert!(provider.enabled(), "there is nothing for a switch to gate");
    }

    /// cide joins the two halves and never splits one, which is the whole reason they are stored
    /// apart: neither `/` nor `:` is a safe delimiter in a real model id.
    #[test]
    fn a_pool_entry_joins_its_two_halves_and_never_splits_one() {
        let awkward = PoolEntry {
            provider: "lmstudio".into(),
            model: "openai/gpt-oss-20b".into(),
            variant: String::new(),
        };
        assert_eq!(awkward.model_flag(), "lmstudio/openai/gpt-oss-20b");
        let colon = PoolEntry {
            provider: "unsloth".into(),
            model: "Qwen3.8-27B-GGUF:UD-Q4_K_M".into(),
            variant: "high".into(),
        };
        assert_eq!(colon.model_flag(), "unsloth/Qwen3.8-27B-GGUF:UD-Q4_K_M");
    }

    /// Whitespace goes; **rows do not**, however incomplete. The credential is left exactly as
    /// typed. See [`LlmSettings::cleaned`] for why dropping an incomplete row broke every Add
    /// button on the Models screen.
    #[test]
    fn cleaning_trims_without_dropping_and_never_edits_a_key() {
        let settings = LlmSettings {
            providers: vec![
                LlmProvider::Catalog {
                    id: "  openrouter  ".into(),
                    label: "  OpenRouter ".into(),
                    enabled: true,
                    // Trailing whitespace in a credential is the user's to see. Trimming it here
                    // is how a key that works in a terminal stops working in cide.
                    api_key: " sk-spaced ".into(),
                },
                LlmProvider::default(),
            ],
            pools: vec![
                ModelPool {
                    name: " cheap-first ".into(),
                    description: String::new(),
                    entries: vec![
                        PoolEntry {
                            provider: " openrouter ".into(),
                            model: " deepseek/deepseek-chat ".into(),
                            variant: String::new(),
                        },
                        PoolEntry::default(),
                    ],
                },
                ModelPool::default(),
            ],
        }
        .cleaned();

        assert_eq!(
            settings.providers.len(),
            2,
            "the blank row a user just added survives"
        );
        assert_eq!(settings.providers[0].id(), "openrouter");
        assert_eq!(settings.providers[0].api_key(), " sk-spaced ");
        assert_eq!(settings.pools.len(), 2, "and so does the blank pool");
        assert_eq!(settings.pools[0].name, "cheap-first");
        assert_eq!(settings.pools[0].entries.len(), 2, "and the blank entry");
        assert_eq!(
            settings.pools[0].entries[0].model_flag(),
            "openrouter/deepseek/deepseek-chat"
        );
    }

    /// An entry naming a provider that does not exist survives cleaning — it is struck through on
    /// screen, not deleted, because the provider may be the next thing the user creates.
    #[test]
    fn an_entry_naming_an_unconfigured_provider_is_kept() {
        let settings = LlmSettings {
            providers: Vec::new(),
            pools: vec![ModelPool {
                name: "p".into(),
                description: String::new(),
                entries: vec![PoolEntry {
                    provider: "not-configured-yet".into(),
                    model: "m".into(),
                    variant: String::new(),
                }],
            }],
        }
        .cleaned();
        assert_eq!(settings.pools[0].entries.len(), 1);
        assert!(settings.provider("not-configured-yet").is_none());
    }
    /// Reproduces the user's gesture: press "Add a known provider", which appends a blank row and
    /// sends the whole group to Rust.
    #[test]
    fn pressing_add_actually_adds_a_row() {
        let after = LlmSettings {
            providers: vec![LlmProvider::default()],
            pools: vec![ModelPool::default()],
        }
        .cleaned();
        assert_eq!(
            after.providers.len(),
            1,
            "a freshly added provider must survive the save"
        );
        assert_eq!(after.pools.len(), 1, "and so must a freshly added pool");
    }
}
