//! Reading a custom model's limits off its own server. (M88)
//!
//! # Why this exists
//!
//! opencode takes a custom model's window from its configuration and from nowhere else: it never
//! asks an OpenAI-compatible server what it serves. A model declared without `limit.context`
//! therefore runs with no compaction threshold — the conversation grows until the server refuses
//! a request as too long, mid-task. The Settings row had two number boxes and no way to know what
//! to put in them short of reading the server's launch flags.
//!
//! The server usually knows, and says so in one of three places:
//!
//! | server | where | field |
//! | --- | --- | --- |
//! | vLLM, SGLang | `GET {base}/models` | `max_model_len` |
//! | OpenRouter and several hosted APIs | `GET {base}/models` | `context_length`, `top_provider.*` |
//! | llama.cpp `llama-server` | `GET {root}/props` | `default_generation_settings.n_ctx` |
//! | LM Studio | `GET {root}/api/v0/models` | `loaded_context_length`, `max_context_length` |
//!
//! # What is deliberately not read
//!
//! **A model's *trained* length is not its served length.** llama.cpp's `/v1/models` carries
//! `meta.n_ctx_train`, and Ollama's `/api/show` a `<arch>.context_length`; both describe the
//! weights, while the server may have been started with a quarter of that (Ollama's default
//! `num_ctx` is a few thousand tokens). Filling the row with the trained figure would move the
//! compaction threshold past the point the server starts refusing — worse than no number, since
//! it looks like a measurement. So those are not consulted, and the probe answers "the server
//! does not say" for them.
//!
//! # The output figure is usually an estimate
//!
//! Few servers state a completion ceiling, and opencode refuses a `limit.context` without a
//! `limit.output`. [`estimate_output`] supplies one, and the answer says it did.
//!
//! The parsers are pure and take `serde_json::Value`s, so every server shape is a test fixture;
//! [`probe`] is the only function here that touches the network.

use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::Value;

/// What one endpoint said about one model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Tokens the server will accept in one request.
    pub context: u32,
    /// Its completion ceiling, when it states one.
    pub output: Option<u32>,
    /// Which field the context came from, for the sentence the row shows.
    pub field: &'static str,
}

/// The context fields of an OpenAI-shaped model entry, best first.
///
/// `loaded_context_length` leads because it is LM Studio's figure for the model *as loaded* —
/// its `max_context_length` beside it is the trained length, the thing the module header
/// refuses to use when a served figure is on offer.
const CONTEXT_FIELDS: &[&str] = &[
    "loaded_context_length",
    "max_model_len",
    "context_length",
    "context_window",
    "max_context_length",
    "top_provider.context_length",
];

/// The output fields, best first.
const OUTPUT_FIELDS: &[&str] = &[
    "max_completion_tokens",
    "top_provider.max_completion_tokens",
    "max_output_tokens",
];

/// A dotted path into a JSON object, as a positive `u32`.
fn number_at(entry: &Value, path: &str) -> Option<u32> {
    let mut at = entry;
    for key in path.split('.') {
        at = at.get(key)?;
    }
    at.as_u64()
        .filter(|&n| n > 0)
        .and_then(|n| u32::try_from(n).ok())
}

/// Read one model's limits out of a model list: `{"data": [...]}` (OpenAI, vLLM, SGLang,
/// OpenRouter, LM Studio's `/api/v0`) or a bare array.
///
/// Matched on the exact id. A list of one whose id differs is **not** taken as the answer: a
/// server serving `qwen-36-27b-fp8` under a row that says `qwen-36-27b` is a row opencode will
/// fail to reach, and filling it with that model's numbers would make the typo look configured.
#[must_use]
pub fn from_model_list(body: &Value, model: &str) -> Option<Found> {
    let entries = body
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| body.as_array())?;
    let entry = entries
        .iter()
        .find(|entry| entry.get("id").and_then(Value::as_str) == Some(model))?;
    let (field, context) = CONTEXT_FIELDS
        .iter()
        .find_map(|field| number_at(entry, field).map(|n| (*field, n)))?;
    let output = OUTPUT_FIELDS
        .iter()
        .find_map(|field| number_at(entry, field))
        // A ceiling at or past the whole window is no ceiling: it would leave no room for the
        // prompt, and opencode reserves the output from the window when it decides to compact.
        .filter(|&output| output < context);
    Some(Found {
        context,
        output,
        field,
    })
}

/// Read llama.cpp's `/props`: the context each request slot actually has.
///
/// `default_generation_settings.n_ctx` and not a top-level `n_ctx`, because the server divides
/// its `-c` between `--parallel` slots and the per-slot figure is what one request gets. The
/// top-level one is read only where an older build puts nothing else.
#[must_use]
pub fn from_llama_props(body: &Value) -> Option<Found> {
    number_at(body, "default_generation_settings.n_ctx")
        .map(|context| (context, "default_generation_settings.n_ctx"))
        .or_else(|| number_at(body, "n_ctx").map(|context| (context, "n_ctx")))
        .map(|(context, field)| Found {
            context,
            output: None,
            field,
        })
}

/// An output ceiling for a window whose server named none: a quarter of it, at most 32768.
///
/// A quarter so a small local window still leaves most of itself to the conversation (8192
/// gives 2048), and 32768 as the cap because it is what the hosted models this machine already
/// declares use, and because opencode subtracts the output from the window when it decides to
/// compact — a larger reserve only compacts sooner.
#[must_use]
pub fn estimate_output(context: u32) -> u32 {
    (context / 4).clamp(1, 32_768)
}

/// A blocking client for one probe, on the network path an opencode child would take.
///
/// Built from the proxy settings a run gets (`ProxyScope::claude` — the agents' column), so
/// "the probe reached it" means the same as "a run will reach it". `cide-gitlab`'s builder is
/// the model; the difference is that both schemes are proxied, since a custom endpoint is as
/// often `http://` on a LAN as `https://` on the internet, and `no_proxy` keeps `127.0.0.1`
/// direct when the user said so.
pub fn client(proxy: &cide_ipc::ProxySettings) -> Result<Client, String> {
    use cide_ipc::{ProxyMode, ProxyTarget};
    let mut builder = Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10));
    match (proxy.scope.claude, proxy.mode) {
        (ProxyTarget::Direct, _) => builder = builder.no_proxy(),
        (ProxyTarget::Configured, ProxyMode::Manual) => {
            builder = builder.no_proxy();
            let bypass = || reqwest::NoProxy::from_string(&proxy.no_proxy);
            if let Some(url) = cide_ipc::normalize_proxy_url(&proxy.http)
                .or_else(|| cide_ipc::normalize_proxy_url(&proxy.all))
            {
                builder = builder.proxy(
                    reqwest::Proxy::http(url)
                        .map_err(|_| "the HTTP proxy in Settings is not a valid URL".to_string())?
                        .no_proxy(bypass()),
                );
            }
            if let Some(url) = proxy
                .https_url()
                .or_else(|| cide_ipc::normalize_proxy_url(&proxy.all))
            {
                builder = builder.proxy(
                    reqwest::Proxy::https(url)
                        .map_err(|_| "the HTTPS proxy in Settings is not a valid URL".to_string())?
                        .no_proxy(bypass()),
                );
            }
        }
        // Inherit, or a scope left untouched: the environment's own proxy, which is what the
        // child inherits too (`reqwest`'s `system-proxy` reads the same variables).
        _ => {}
    }
    builder
        .build()
        .map_err(|_| "cide could not start an HTTP client".to_string())
}

/// One GET. `Ok(None)` for an endpoint this server does not have (a 404, a 405, a body that is
/// not JSON) — the ordinary answer from all but one of the places [`probe`] looks; `Err` only
/// for what should stop the search: the server unreachable, or refusing the key.
fn get(client: &Client, url: &str, api_key: &str) -> Result<Option<Value>, String> {
    let mut request = client.get(url);
    if !api_key.is_empty() {
        request = request.bearer_auth(api_key);
    }
    let response = request.send().map_err(|error| {
        if error.is_timeout() {
            format!("{url} did not answer within 10 seconds")
        } else if error.is_connect() {
            format!("could not connect to {url}; check the base URL and that the server is up")
        } else {
            format!("the request to {url} failed")
        }
    })?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(format!(
            "{url} refused the API key (HTTP {}); set the provider's key first",
            status.as_u16()
        ));
    }
    if !status.is_success() {
        return Ok(None);
    }
    Ok(response.json::<Value>().ok())
}

/// Ask a custom provider's server for `model`'s limits.
///
/// `Ok(Some(found, endpoint))` names the URL that answered; `Ok(None)` is a reachable server
/// that states no served window for this model anywhere cide knows to look. The three places are
/// tried in the module header's order, and the first two already cover vLLM, SGLang and
/// llama.cpp, which is nearly every self-hosted endpoint.
pub fn probe(
    client: &Client,
    base_url: &str,
    api_key: &str,
    model: &str,
) -> Result<Option<(Found, String)>, String> {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err("this provider has no base URL yet".to_string());
    }
    // The server's own root, for the two endpoints that are not under the OpenAI prefix.
    let root = base.strip_suffix("/v1").unwrap_or(base);

    let models = format!("{base}/models");
    let listed = get(client, &models, api_key)?;
    if let Some(found) = listed
        .as_ref()
        .and_then(|body| from_model_list(body, model))
    {
        return Ok(Some((found, models)));
    }
    let props = format!("{root}/props");
    if let Some(found) = get(client, &props, api_key)?
        .as_ref()
        .and_then(from_llama_props)
    {
        return Ok(Some((found, props)));
    }
    let lmstudio = format!("{root}/api/v0/models");
    if let Some(found) = get(client, &lmstudio, api_key)?
        .as_ref()
        .and_then(|body| from_model_list(body, model))
    {
        return Ok(Some((found, lmstudio)));
    }
    // Said apart from "states nothing": a list that does not contain the id is the more useful
    // sentence, because it is usually a typo in the row.
    let ids: Vec<&str> = listed
        .as_ref()
        .and_then(|body| body.get("data").and_then(Value::as_array))
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("id").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    if !ids.is_empty() && !ids.contains(&model) {
        return Err(format!(
            "the server does not serve `{model}`; it lists {}",
            ids.iter()
                .take(8)
                .map(|id| format!("`{id}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The k3s endpoint this was written against, verbatim in shape: SGLang's `/v1/models`.
    #[test]
    fn sglang_and_vllm_state_max_model_len() {
        let body = json!({ "object": "list", "data": [
            { "id": "qwen-36-27b-fp8", "object": "model", "owned_by": "sglang",
              "max_model_len": 262_144 }
        ]});
        let found = from_model_list(&body, "qwen-36-27b-fp8").expect("found");
        assert_eq!(found.context, 262_144);
        assert_eq!(found.output, None);
        assert_eq!(found.field, "max_model_len");
    }

    #[test]
    fn a_model_the_list_does_not_have_is_not_answered_by_its_neighbour() {
        let body = json!({ "data": [{ "id": "qwen-36-27b-fp8", "max_model_len": 262_144 }] });
        assert_eq!(from_model_list(&body, "qwen-36-27b"), None);
    }

    #[test]
    fn openrouter_states_both_through_top_provider() {
        let body = json!({ "data": [{
            "id": "minimax/minimax-m3:free",
            "context_length": 1_000_000,
            "top_provider": { "context_length": 1_000_000, "max_completion_tokens": 40_000 }
        }]});
        let found = from_model_list(&body, "minimax/minimax-m3:free").expect("found");
        assert_eq!(found.context, 1_000_000);
        assert_eq!(found.output, Some(40_000));
    }

    /// LM Studio states both the trained and the loaded window; the loaded one is what serves.
    #[test]
    fn lm_studio_prefers_the_loaded_context() {
        let body = json!({ "data": [{
            "id": "qwen3-8b", "max_context_length": 131_072, "loaded_context_length": 16_384
        }]});
        let found = from_model_list(&body, "qwen3-8b").expect("found");
        assert_eq!(found.context, 16_384);
        assert_eq!(found.field, "loaded_context_length");
    }

    /// llama.cpp's `/v1/models` carries only the trained length — deliberately not read.
    #[test]
    fn a_trained_length_is_not_taken_for_a_served_one() {
        let body = json!({ "data": [{ "id": "m", "meta": { "n_ctx_train": 262_144 } }] });
        assert_eq!(from_model_list(&body, "m"), None);
    }

    #[test]
    fn llama_cpp_props_give_the_per_slot_context() {
        let props = json!({
            "default_generation_settings": { "n_ctx": 32_768 },
            "total_slots": 4
        });
        assert_eq!(from_llama_props(&props).map(|f| f.context), Some(32_768));
        assert_eq!(
            from_llama_props(&json!({ "n_ctx": 8192 })).map(|f| f.context),
            Some(8192)
        );
        assert_eq!(from_llama_props(&json!({ "total_slots": 1 })), None);
    }

    /// An output that fills the whole window is not a ceiling anyone can use.
    #[test]
    fn an_output_as_large_as_the_window_is_dropped() {
        let body = json!({ "data": [{
            "id": "m", "context_length": 32_768, "max_completion_tokens": 32_768
        }]});
        assert_eq!(from_model_list(&body, "m").expect("found").output, None);
    }

    #[test]
    fn the_estimate_is_a_quarter_capped_at_32k() {
        assert_eq!(estimate_output(262_144), 32_768);
        assert_eq!(estimate_output(8192), 2048);
        assert_eq!(estimate_output(1), 1);
    }

    /// Against a real server, on purpose: `CIDE_LIMITS_PROBE=<base url>|<model>` and, when the
    /// server wants one, `CIDE_LIMITS_KEY`. Free — one `GET` — but it needs a server, so it is
    /// `#[ignore]`d like every test here that reaches outside the process.
    #[test]
    #[ignore = "needs a live server: CIDE_LIMITS_PROBE=<base url>|<model>"]
    fn a_real_server() {
        let spec = std::env::var("CIDE_LIMITS_PROBE").expect("CIDE_LIMITS_PROBE");
        let (base, model) = spec.split_once('|').expect("<base url>|<model>");
        let key = std::env::var("CIDE_LIMITS_KEY").unwrap_or_default();
        let client = client(&cide_ipc::ProxySettings::default()).expect("client");
        let answer = probe(&client, base, &key, model);
        eprintln!("{answer:?}");
        assert!(matches!(answer, Ok(Some(_))), "{answer:?}");
    }
}
