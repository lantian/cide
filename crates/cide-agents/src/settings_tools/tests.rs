//! The settings vocabulary against a sink with no app behind it. (M134)
//!
//! The keymap half runs over the **real** `cide_core::keymap::resolve` and `apply_edit`, so a
//! rebind here goes through the same layering, removal lines and default-collapse a write to
//! `keymap.json` does — a fake keymap would be a test of the fake.

use std::path::PathBuf;
use std::sync::Mutex;

use super::HIDDEN;
use serde_json::{Value, json};

use cide_ipc::settings::Settings;
use cide_ipc::settings_ops::{KeymapConflict, KeymapReport, SettingsPatch};
use cide_ipc::{
    AgentDef, AgentId, AgentRun, Binding, KeymapEdit, LlmSettings, OrchestrationConfig,
    OrchestrationPatch, ProjectOverrides, RunId, TaskId,
    agents::{AgentDraft, AgentScope},
};

use crate::tools::{AgentSink, CallerKind, Integrated, Notify, Stopped, ToolResult, tool};

struct Fake {
    caller: CallerKind,
    settings: Mutex<Settings>,
    project_harness: Mutex<cide_ipc::ProjectHarnessSettings>,
    window_mode: Mutex<Option<cide_ipc::WindowMode>>,
    user_keymap: Mutex<Vec<Binding>>,
    edits: Mutex<Vec<Vec<KeymapEdit>>>,
    extensions: Mutex<Vec<cide_ipc::ext::InstalledExtension>>,
    spec: Mutex<cide_ipc::spec::SpecSettings>,
    gitlab: Mutex<cide_ipc::gitlab::GitLabPreferences>,
    ran: Mutex<Vec<(String, Option<Value>)>>,
}

impl Fake {
    fn new() -> Self {
        Self::as_caller(CallerKind::Console)
    }

    fn as_caller(caller: CallerKind) -> Self {
        Self {
            caller,
            settings: Mutex::new(Settings::default()),
            project_harness: Mutex::new(Default::default()),
            window_mode: Mutex::new(None),
            user_keymap: Mutex::new(Vec::new()),
            edits: Mutex::new(Vec::new()),
            extensions: Mutex::new(Vec::new()),
            spec: Mutex::new(cide_ipc::spec::SpecSettings::default()),
            gitlab: Mutex::new(cide_ipc::gitlab::GitLabPreferences::default()),
            ran: Mutex::new(Vec::new()),
        }
    }

    fn report(&self) -> KeymapReport {
        let user = self.user_keymap.lock().unwrap().clone();
        let (bindings, _) = cide_core::keymap::resolve_with_diagnostics(&user);
        let conflicts = cide_core::keymap::conflicts(&bindings)
            .into_iter()
            .map(|c| KeymapConflict {
                key: c.key,
                commands: c.commands,
                when: c.when,
            })
            .collect();
        KeymapReport {
            bindings,
            conflicts,
            problems: Vec::new(),
            overrides: user,
            readable: true,
            path: "keymap.json".into(),
        }
    }
}

impl AgentSink for Fake {
    fn agents(&self) -> Result<Vec<AgentDef>, String> {
        Ok(Vec::new())
    }
    fn definition(&self, _: AgentScope, _: &AgentId) -> Result<Option<AgentDraft>, String> {
        Ok(None)
    }
    fn write_definition(&self, _: &AgentDraft) -> Result<PathBuf, String> {
        Err("unused".into())
    }
    fn runs(&self) -> Result<Vec<AgentRun>, String> {
        Ok(Vec::new())
    }
    fn dispatch(
        &self,
        _: &AgentId,
        _: Option<&TaskId>,
        _: Option<&str>,
        _: Notify,
    ) -> Result<RunId, String> {
        Err("unused".into())
    }
    fn stop(&self, _: RunId, _: Option<&str>, _: bool) -> Result<Stopped, String> {
        Err("unused".into())
    }
    fn integrate(&self, _: &AgentId, _: Option<&TaskId>) -> Result<Integrated, String> {
        Err("unused".into())
    }
    fn now_unix_ms(&self) -> u64 {
        0
    }
    fn isolated(&self) -> Result<bool, String> {
        Ok(true)
    }
    fn dispatching(&self) -> Result<bool, String> {
        Ok(true)
    }
    fn config(&self) -> Result<OrchestrationConfig, String> {
        Err("unused".into())
    }
    fn set_config(&self, _: OrchestrationPatch) -> Result<OrchestrationConfig, String> {
        Err("unused".into())
    }
    fn overrides(&self) -> Result<ProjectOverrides, String> {
        Ok(ProjectOverrides::default())
    }
    fn set_overrides(&self, _: ProjectOverrides) -> Result<(), String> {
        Err("unused".into())
    }
    fn llm(&self) -> Result<LlmSettings, String> {
        Ok(LlmSettings::default())
    }
    fn set_llm(&self, _: LlmSettings) -> Result<(), String> {
        Err("unused".into())
    }
    fn resolutions(&self) -> Result<Vec<(AgentId, crate::overrides::Resolved)>, String> {
        Ok(Vec::new())
    }

    fn caller(&self) -> CallerKind {
        self.caller
    }
    fn settings(&self) -> Result<Settings, String> {
        Ok(self.settings.lock().unwrap().clone())
    }
    fn project_harness(&self) -> Result<cide_ipc::ProjectHarnessSettings, String> {
        Ok(self.project_harness.lock().unwrap().clone())
    }
    fn set_project_harness(
        &self,
        edit: cide_ipc::ProjectHarnessEdit,
    ) -> Result<cide_ipc::ProjectHarnessSettings, String> {
        let mut value = self.project_harness.lock().unwrap();
        value.apply(edit);
        Ok(value.clone())
    }
    /// Group-wise, as `apply_patch` is: every group the patch names replaces the stored one.
    /// The accent is stored unfitted — fitting is `cide_core::accent`'s, tested there.
    fn set_settings(&self, patch: SettingsPatch) -> Result<Settings, String> {
        let mut stored = self.settings.lock().unwrap();
        let mut whole = serde_json::to_value(&*stored).unwrap();
        let patch = serde_json::to_value(patch).unwrap();
        for (key, value) in patch.as_object().unwrap() {
            if value.is_null() {
                continue;
            }
            let value = match (key.as_str(), value) {
                ("accent", Value::String(reset)) if reset == "reset" => Value::Null,
                ("accent", set) => {
                    let base = set["set"]["base"].clone();
                    json!({ "base": base, "light": base, "dark": base })
                }
                _ => value.clone(),
            };
            whole[key] = value;
        }
        *stored = serde_json::from_value(whole).map_err(|e| e.to_string())?;
        Ok(stored.clone())
    }
    fn set_window_mode(&self, mode: cide_ipc::WindowMode) -> Result<(), String> {
        *self.window_mode.lock().unwrap() = Some(mode);
        Ok(())
    }
    fn keymap(&self) -> Result<KeymapReport, String> {
        Ok(self.report())
    }
    fn keymap_edit(&self, edits: Vec<KeymapEdit>) -> Result<KeymapReport, String> {
        {
            let mut user = self.user_keymap.lock().unwrap();
            for edit in &edits {
                cide_core::keymap::apply_edit(&mut user, edit);
            }
        }
        self.edits.lock().unwrap().push(edits);
        Ok(self.report())
    }
    fn extensions(&self) -> Result<Vec<cide_ipc::ext::InstalledExtension>, String> {
        Ok(self.extensions.lock().unwrap().clone())
    }
    fn set_extension_setting(
        &self,
        extension: &cide_ipc::ext::ExtensionRef,
        key: &str,
        value: Value,
    ) -> Result<(), String> {
        let mut all = self.extensions.lock().unwrap();
        let ext = all.iter_mut().find(|e| e.id == *extension).unwrap();
        ext.settings.insert(key.into(), value);
        Ok(())
    }
    fn spec_settings(&self) -> Result<cide_ipc::spec::SpecSettings, String> {
        Ok(*self.spec.lock().unwrap())
    }
    fn set_spec_settings(&self, settings: cide_ipc::spec::SpecSettings) -> Result<(), String> {
        *self.spec.lock().unwrap() = settings;
        Ok(())
    }
    fn gitlab_preferences(&self) -> Result<cide_ipc::gitlab::GitLabPreferences, String> {
        Ok(self.gitlab.lock().unwrap().clone())
    }
    fn set_gitlab_preferences(
        &self,
        preferences: cide_ipc::gitlab::GitLabPreferences,
    ) -> Result<(), String> {
        *self.gitlab.lock().unwrap() = preferences;
        Ok(())
    }
    fn run_command(&self, command: &str, args: Option<Value>) -> Result<(), String> {
        self.ran.lock().unwrap().push((command.into(), args));
        Ok(())
    }
}

fn ask(name: &str, arguments: Value, sink: &Fake) -> ToolResult {
    crate::tools::dispatch_orchestration(name, &arguments, sink).expect("one of ours")
}

fn text(result: &ToolResult) -> String {
    result.to_json()["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn ok(result: ToolResult) -> String {
    let body = text(&result);
    assert!(!result.is_error, "expected success, got: {body}");
    body
}

fn refused(result: ToolResult) -> String {
    let body = text(&result);
    assert!(result.is_error, "expected a refusal, got: {body}");
    body
}

// --- the vocabulary ---------------------------------------------------------------------------

#[test]
fn every_new_tool_is_advertised_with_a_description_and_a_schema() {
    for name in [
        tool::SETTINGS,
        tool::KEYMAP,
        tool::EXTENSION_SETTINGS,
        tool::PROJECT_SETTINGS,
        tool::COMMAND_RUN,
    ] {
        assert!(
            tool::ORCHESTRATION.contains(&name),
            "{name} not in ORCHESTRATION"
        );
        assert!(
            tool::ORCHESTRATION_NO_TRACKER.contains(&name),
            "{name} not in ORCHESTRATION_NO_TRACKER"
        );
        assert!(!tool::ALL.contains(&name), "{name} must never reach a run");
        assert!(
            !crate::tools::description(name).is_empty(),
            "{name} has no description"
        );
        assert_eq!(
            crate::tools::input_schema(name)["type"],
            "object",
            "{name} has no schema"
        );
    }
}

// --- cide_settings ----------------------------------------------------------------------------

#[test]
fn setting_one_leaf_leaves_every_sibling_alone() {
    let sink = Fake::new();
    {
        let mut settings = sink.settings.lock().unwrap();
        settings.editor.tab_size = 2;
        settings.editor.show_minimap = false;
    }
    ok(ask(
        tool::SETTINGS,
        json!({"action": "set", "path": "editor.wordWrap", "value": true}),
        &sink,
    ));
    let after = sink.settings.lock().unwrap().clone();
    assert!(after.editor.word_wrap);
    assert_eq!(after.editor.tab_size, 2);
    assert!(!after.editor.show_minimap);
}

#[test]
fn a_wrong_type_or_variant_is_refused_naming_the_path_and_nothing_is_stored() {
    let sink = Fake::new();
    let before = sink.settings.lock().unwrap().clone();
    let why = refused(ask(
        tool::SETTINGS,
        json!({"action": "set", "path": "git.pullStrategy", "value": "yolo"}),
        &sink,
    ));
    assert!(why.contains("git.pullStrategy"), "{why}");
    assert!(
        why.contains("rebase"),
        "serde's variant list should reach the model: {why}"
    );
    refused(ask(
        tool::SETTINGS,
        json!({"action": "set", "path": "editor.wordWrap", "value": "yes"}),
        &sink,
    ));
    assert_eq!(*sink.settings.lock().unwrap(), before);
}

#[test]
fn an_unknown_path_names_its_neighbours() {
    let sink = Fake::new();
    let why = refused(ask(
        tool::SETTINGS,
        json!({"action": "set", "path": "editor.wrap", "value": true}),
        &sink,
    ));
    assert!(why.contains("editor.wordWrap"), "{why}");
}

#[test]
fn a_group_is_reset_whole_but_never_set_whole() {
    let sink = Fake::new();
    sink.settings.lock().unwrap().editor.tab_size = 8;
    refused(ask(
        tool::SETTINGS,
        json!({"action": "set", "path": "editor", "value": {"wordWrap": true}}),
        &sink,
    ));
    assert_eq!(sink.settings.lock().unwrap().editor.tab_size, 8);
    ok(ask(
        tool::SETTINGS,
        json!({"action": "reset", "path": "editor"}),
        &sink,
    ));
    assert_eq!(
        sink.settings.lock().unwrap().editor,
        Settings::default().editor
    );
}

#[test]
fn reset_restores_one_default() {
    let sink = Fake::new();
    sink.settings.lock().unwrap().terminal.scrollback = 1;
    ok(ask(
        tool::SETTINGS,
        json!({"action": "reset", "path": "terminal.scrollback"}),
        &sink,
    ));
    assert_eq!(
        sink.settings.lock().unwrap().terminal.scrollback,
        Settings::default().terminal.scrollback
    );
}

#[test]
fn a_data_map_takes_a_new_key_and_null_removes_it() {
    let sink = Fake::new();
    ok(ask(
        tool::SETTINGS,
        json!({"action": "set", "path": "inspections.sources.clippy", "value": false}),
        &sink,
    ));
    assert_eq!(
        sink.settings
            .lock()
            .unwrap()
            .inspections
            .sources
            .get("clippy"),
        Some(&false)
    );
    ok(ask(
        tool::SETTINGS,
        json!({"action": "reset", "path": "inspections.sources.clippy"}),
        &sink,
    ));
    assert!(
        !sink
            .settings
            .lock()
            .unwrap()
            .inspections
            .sources
            .contains_key("clippy")
    );
}

#[test]
fn llm_is_refused_and_pointed_at_its_own_tools() {
    let sink = Fake::new();
    let why = refused(ask(
        tool::SETTINGS,
        json!({"action": "set", "path": "llm.providers", "value": []}),
        &sink,
    ));
    assert!(why.contains("cide_llm_provider"), "{why}");
}

#[test]
fn no_answer_prints_a_proxy_password_or_an_env_value() {
    let sink = Fake::new();
    {
        let mut settings = sink.settings.lock().unwrap();
        settings.proxy.http = "http://alice:hunter2@proxy.corp:3128".into();
        settings.claude.cli.env = vec![cide_ipc::settings::ClaudeEnvVar {
            name: "GITHUB_TOKEN".into(),
            value: "ghp_secret".into(),
        }];
    }
    let everything = ok(ask(tool::SETTINGS, json!({"action": "get"}), &sink));
    for secret in ["hunter2", "alice", "ghp_secret"] {
        assert!(
            !everything.contains(secret),
            "{secret} leaked: {everything}"
        );
    }
    assert!(
        everything.contains("proxy.corp:3128"),
        "the host is not a secret"
    );
    assert!(
        everything.contains("GITHUB_TOKEN"),
        "the name is not a secret"
    );
    let after_write = ok(ask(
        tool::SETTINGS,
        json!({"action": "set", "path": "proxy.mode", "value": "manual"}),
        &sink,
    ));
    assert!(!after_write.contains("hunter2"));
}

#[test]
fn the_hidden_placeholder_is_never_written_back() {
    let sink = Fake::new();
    let why = refused(ask(
        tool::SETTINGS,
        json!({"action": "set", "path": "claude.cli.env", "value": [{"name": "T", "value": super::HIDDEN}]}),
        &sink,
    ));
    assert!(why.contains("secret"), "{why}");
}

#[test]
fn accent_and_window_mode_take_their_own_roads() {
    let sink = Fake::new();
    ok(ask(
        tool::SETTINGS,
        json!({"action": "set", "path": "accent", "value": "#4f46e5"}),
        &sink,
    ));
    assert!(sink.settings.lock().unwrap().accent.is_some());
    ok(ask(
        tool::SETTINGS,
        json!({"action": "reset", "path": "accent"}),
        &sink,
    ));
    assert!(sink.settings.lock().unwrap().accent.is_none());
    ok(ask(
        tool::SETTINGS,
        json!({"action": "set", "path": "windowMode", "value": "perProject"}),
        &sink,
    ));
    assert_eq!(
        *sink.window_mode.lock().unwrap(),
        Some(cide_ipc::WindowMode::PerProject)
    );
}

#[test]
fn get_shows_a_changed_value_beside_its_default_and_describe_explains_it() {
    let sink = Fake::new();
    sink.settings.lock().unwrap().editor.word_wrap = true;
    let got = ok(ask(
        tool::SETTINGS,
        json!({"action": "get", "path": "editor.wordWrap"}),
        &sink,
    ));
    assert_eq!(got, "editor.wordWrap = true   (default false)");
    let described = ok(ask(
        tool::SETTINGS,
        json!({"action": "describe", "path": "git"}),
        &sink,
    ));
    assert!(described.contains("git.pullStrategy — enum ask | fastForward | merge | rebase"));
}

#[test]
fn a_tab_cide_opened_may_read_but_never_write() {
    for caller in [CallerKind::CideTab, CallerKind::Worker, CallerKind::Run] {
        let sink = Fake::as_caller(caller);
        ok(ask(
            tool::SETTINGS,
            json!({"action": "get", "path": "theme"}),
            &sink,
        ));
        ok(ask(
            tool::KEYMAP,
            json!({"action": "find", "query": "palette"}),
            &sink,
        ));
        for (name, arguments) in [
            (
                tool::SETTINGS,
                json!({"action": "set", "path": "theme", "value": "light"}),
            ),
            (tool::SETTINGS, json!({"action": "reset", "path": "theme"})),
            (
                tool::KEYMAP,
                json!({"action": "rebind", "command": "palette.commands", "key": "ctrl+alt+p"}),
            ),
            (tool::KEYMAP, json!({"action": "resetAll", "confirm": true})),
            (tool::COMMAND_RUN, json!({"command": "palette.commands"})),
            (
                tool::PROJECT_SETTINGS,
                json!({"action": "set", "section": "openspec", "field": "applyInWorktree", "value": true}),
            ),
        ] {
            let why = refused(ask(name, arguments, &sink));
            assert!(why.contains("console"), "{name}: {why}");
        }
        assert_eq!(*sink.settings.lock().unwrap(), Settings::default());
        assert!(sink.edits.lock().unwrap().is_empty());
        assert!(sink.ran.lock().unwrap().is_empty());
    }
}

// --- cide_keymap ------------------------------------------------------------------------------

/// The palette's default chord as `cide_core::keymap::platform_defaults` spells it on this
/// machine: the macOS layer moves `ctrl` to `meta`, so hard-coding `ctrl+shift+p` passed on Linux
/// and failed only on the macOS CI runner ("ctrl+shift+p is not bound to anything").
const PALETTE_KEY: &str = if cfg!(target_os = "macos") {
    "shift+meta+p"
} else {
    "ctrl+shift+p"
};

#[test]
fn find_answers_how_to_open_the_command_palette() {
    let sink = Fake::new();
    let found = ok(ask(
        tool::KEYMAP,
        json!({"action": "find", "query": "command palette"}),
        &sink,
    ));
    let first = found.lines().next().unwrap();
    assert!(first.starts_with("palette.commands"), "{found}");
    assert!(first.contains(PALETTE_KEY), "{found}");
}

#[test]
fn lookup_names_the_command_a_default_key_runs() {
    let sink = Fake::new();
    let found = ok(ask(
        tool::KEYMAP,
        json!({"action": "lookup", "key": PALETTE_KEY.to_uppercase()}),
        &sink,
    ));
    assert!(found.contains("palette.commands"), "{found}");
    refused(ask(
        tool::KEYMAP,
        json!({"action": "lookup", "key": "ctrl+"}),
        &sink,
    ));
}

#[test]
fn rebind_to_a_free_chord_moves_the_command() {
    let sink = Fake::new();
    let answer = ok(ask(
        tool::KEYMAP,
        json!({"action": "rebind", "command": "palette.commands", "key": "ctrl+alt+f12"}),
        &sink,
    ));
    assert!(answer.contains("ctrl+alt+f12"), "{answer}");
    let report = sink.report();
    let keys: Vec<&str> = report
        .bindings
        .iter()
        .filter(|b| b.command == "palette.commands")
        .map(|b| b.key.as_str())
        .collect();
    assert_eq!(keys, ["ctrl+alt+f12"]);
}

/// A key that some default already holds, found through the real table rather than named here,
/// so the test does not break the day a default moves.
fn a_taken_key(sink: &Fake, not: &str) -> (String, String) {
    let report = sink.report();
    let binding = report
        .bindings
        .iter()
        .find(|b| b.command != not && b.when.is_none() && !b.key.contains(' '))
        .expect("some unscoped default");
    (binding.key.clone(), binding.command.clone())
}

#[test]
fn rebind_onto_a_taken_key_is_refused_until_the_conflict_is_decided() {
    let sink = Fake::new();
    let (key, holder) = a_taken_key(&sink, "palette.commands");
    let why = refused(ask(
        tool::KEYMAP,
        json!({"action": "rebind", "command": "palette.commands", "key": key}),
        &sink,
    ));
    assert!(why.contains(&holder), "{why}");
    assert!(sink.edits.lock().unwrap().is_empty());

    ok(ask(
        tool::KEYMAP,
        json!({"action": "rebind", "command": "palette.commands", "key": key, "onConflict": "unbindOther"}),
        &sink,
    ));
    let edits = sink.edits.lock().unwrap().clone();
    assert_eq!(edits.len(), 1, "one write, however many edits");
    assert_eq!(edits[0].len(), 2);
    assert!(matches!(&edits[0][1], KeymapEdit::Unbind { command, .. } if *command == holder));
}

#[test]
fn a_bare_typing_key_needs_the_user_to_insist() {
    let sink = Fake::new();
    refused(ask(
        tool::KEYMAP,
        json!({"action": "rebind", "command": "palette.commands", "key": "shift+q"}),
        &sink,
    ));
    assert!(sink.edits.lock().unwrap().is_empty());
}

#[test]
fn a_command_bound_in_several_contexts_asks_which() {
    let sink = Fake::new();
    let report = sink.report();
    let mut seen: std::collections::BTreeMap<&str, Vec<Option<&str>>> = Default::default();
    for binding in &report.bindings {
        let contexts = seen.entry(binding.command.as_str()).or_default();
        if !contexts.contains(&binding.when.as_deref()) {
            contexts.push(binding.when.as_deref());
        }
    }
    let Some((command, _)) = seen.iter().find(|(_, contexts)| contexts.len() > 1) else {
        return; // no default is bound in two contexts today; nothing to ask about
    };
    let why = refused(ask(
        tool::KEYMAP,
        json!({"action": "rebind", "command": command, "key": "ctrl+alt+f11"}),
        &sink,
    ));
    assert!(why.contains("contexts"), "{why}");
}

#[test]
fn unbind_then_reset_round_trips() {
    let sink = Fake::new();
    ok(ask(
        tool::KEYMAP,
        json!({"action": "unbind", "command": "palette.commands"}),
        &sink,
    ));
    assert!(
        !sink
            .report()
            .bindings
            .iter()
            .any(|b| b.command == "palette.commands")
    );
    ok(ask(
        tool::KEYMAP,
        json!({"action": "reset", "command": "palette.commands"}),
        &sink,
    ));
    assert!(sink.user_keymap.lock().unwrap().is_empty());
    assert!(
        sink.report()
            .bindings
            .iter()
            .any(|b| b.command == "palette.commands")
    );
}

#[test]
fn reset_all_needs_confirmation_and_unknown_ids_are_refused() {
    let sink = Fake::new();
    refused(ask(tool::KEYMAP, json!({"action": "resetAll"}), &sink));
    assert!(sink.edits.lock().unwrap().is_empty());
    ok(ask(
        tool::KEYMAP,
        json!({"action": "resetAll", "confirm": true}),
        &sink,
    ));
    refused(ask(
        tool::KEYMAP,
        json!({"action": "rebind", "command": "no.such.command", "key": "ctrl+alt+f10"}),
        &sink,
    ));
}

// --- cide_command_run -------------------------------------------------------------------------

#[test]
fn command_run_asks_for_a_known_command_and_refuses_the_rest() {
    let sink = Fake::new();
    ok(ask(
        tool::COMMAND_RUN,
        json!({"command": "palette.commands"}),
        &sink,
    ));
    assert_eq!(
        *sink.ran.lock().unwrap(),
        vec![("palette.commands".to_string(), None)]
    );
    refused(ask(
        tool::COMMAND_RUN,
        json!({"command": "no.such.command"}),
        &sink,
    ));
    if let Some(unavailable) = cide_core::commands::registry()
        .iter()
        .find(|c| c.unavailable.is_some())
    {
        refused(ask(
            tool::COMMAND_RUN,
            json!({"command": unavailable.id}),
            &sink,
        ));
    }
    assert_eq!(sink.ran.lock().unwrap().len(), 1);
}

// --- cide_project_settings / cide_extension_settings ------------------------------------------

#[test]
fn project_settings_change_one_field() {
    let sink = Fake::new();
    ok(ask(
        tool::PROJECT_SETTINGS,
        json!({"action": "set", "section": "openspec", "field": "applyInWorktree", "value": true}),
        &sink,
    ));
    assert!(sink.spec.lock().unwrap().apply_in_worktree);
    ok(ask(
        tool::PROJECT_SETTINGS,
        json!({"action": "set", "section": "gitlab", "field": "excludeEnabled", "value": false}),
        &sink,
    ));
    assert!(!sink.gitlab.lock().unwrap().exclude_enabled);
    refused(ask(
        tool::PROJECT_SETTINGS,
        json!({"action": "set", "section": "gitlab", "field": "token", "value": "x"}),
        &sink,
    ));
}

#[test]
fn extension_settings_are_listed_and_a_bad_choice_is_refused() {
    let sink = Fake::new();
    let ext: cide_ipc::ext::InstalledExtension = serde_json::from_value(json!({
        "marketplace": "local",
        "extension": "demo",
        "name": "Demo",
        "version": "1.0.0",
        "description": "",
        "enabled": true,
        "railIcon": false,
        "commit": "",
        "capabilities": [],
        "contributes": {
            "languages": [], "languageServers": [], "panels": [], "commands": [],
            "settings": [{
                "id": "mode", "label": "Mode",
                "kind": {"type": "choice", "default": "fast", "choices": [
                    {"value": "fast", "label": "Fast"}, {"value": "slow", "label": "Slow"}
                ]}
            }]
        },
        "settings": {},
        "path": "/tmp/demo",
        "problems": []
    }))
    .expect("a test extension");
    sink.extensions.lock().unwrap().push(ext);
    let listed = ok(ask(
        tool::EXTENSION_SETTINGS,
        json!({"action": "get"}),
        &sink,
    ));
    assert!(listed.contains("mode = \"fast\""), "{listed}");
    refused(ask(
        tool::EXTENSION_SETTINGS,
        json!({"action": "set", "extension": "local.demo", "key": "mode", "value": "warp"}),
        &sink,
    ));
    ok(ask(
        tool::EXTENSION_SETTINGS,
        json!({"action": "set", "extension": "local.demo", "key": "mode", "value": "slow"}),
        &sink,
    ));
    assert_eq!(
        sink.extensions.lock().unwrap()[0].settings.get("mode"),
        Some(&json!("slow"))
    );
}

#[test]
fn local_harness_overrides_reset_and_redact_launch_secrets() {
    let sink = Fake::new();
    ok(ask(
        tool::PROJECT_SETTINGS,
        json!({"action":"set", "section":"harness", "field":"consoleHarness", "value":"opencode"}),
        &sink,
    ));
    assert_eq!(
        sink.project_harness.lock().unwrap().console_harness,
        Some(cide_ipc::ConsoleHarness::Opencode)
    );
    assert_eq!(
        sink.settings.lock().unwrap().console_harness,
        cide_ipc::ConsoleHarness::Claude
    );
    let mut cli = cide_ipc::OpencodeCli::default();
    cli.env.push(cide_ipc::ClaudeEnvVar {
        name: "TOKEN".into(),
        value: "launch-secret".into(),
    });
    let reply = ok(ask(
        tool::PROJECT_SETTINGS,
        json!({"action":"set", "section":"harness", "field":"opencode", "value":cli}),
        &sink,
    ));
    assert!(!format!("{reply:?}").contains("launch-secret"));
    assert!(reply.contains(HIDDEN));
    let hidden: Value = serde_json::from_str(&reply).unwrap();
    refused(ask(
        tool::PROJECT_SETTINGS,
        json!({"action":"set", "section":"harness", "field":"opencode", "value":hidden["opencode"]}),
        &sink,
    ));
    assert_eq!(
        sink.project_harness
            .lock()
            .unwrap()
            .opencode
            .as_ref()
            .unwrap()
            .env[0]
            .value,
        "launch-secret"
    );
    ok(ask(
        tool::PROJECT_SETTINGS,
        json!({"action":"reset", "section":"harness", "field":"consoleHarness"}),
        &sink,
    ));
    assert!(
        sink.project_harness
            .lock()
            .unwrap()
            .console_harness
            .is_none()
    );
    assert!(sink.project_harness.lock().unwrap().opencode.is_some());
    refused(ask(
        tool::PROJECT_SETTINGS,
        json!({"action":"set", "section":"harness", "field":"consoleHarness", "value":"opencode"}),
        &Fake::as_caller(CallerKind::Worker),
    ));
}
