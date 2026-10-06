//! Local project launch settings. Root paths, rather than transient project ids, are the keys.
use crate::{CoreError, Result};
use cide_ipc::{ProjectHarnessSettings, ProjectId, Settings, Workspace};
use std::{collections::BTreeMap, path::Path};

pub type Projects = BTreeMap<String, ProjectHarnessSettings>;

pub fn root_key(root: &Path) -> String {
    root.canonicalize()
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

pub fn project_key(ws: &Workspace, id: ProjectId) -> Result<String> {
    let project = crate::workspace::project(ws, id)?;
    Ok(root_key(
        &project.roots.first().ok_or(CoreError::NoRoots)?.path,
    ))
}

pub fn overrides(ws: &Workspace, id: ProjectId) -> Result<ProjectHarnessSettings> {
    Ok(ws
        .project_harness
        .get(&project_key(ws, id)?)
        .cloned()
        .unwrap_or_default())
}

pub fn effective(ws: &Workspace, id: Option<ProjectId>) -> Settings {
    id.and_then(|id| overrides(ws, id).ok())
        .unwrap_or_default()
        .resolve(&ws.settings)
}

pub fn load(path: &Path) -> Result<Projects> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Projects::new()),
        Err(e) => return Err(CoreError::Io(e.to_string())),
    };
    serde_json::from_slice(&bytes).map_err(|e| CoreError::Io(format!("{}: {e}", path.display())))
}

pub fn save(path: &Path, projects: &Projects) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(projects).map_err(|e| CoreError::Io(e.to_string()))?;
    // Launch environments may contain credentials.
    crate::persist::write_atomic_with_mode(path, &bytes, 0o600)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::{ConsoleHarness, ProjectHarnessEdit};

    #[test]
    fn inheritance_and_independent_overrides() {
        let mut global = Settings::default();
        global.codex.cli.binary = "global-wrapper".into();
        let mut overrides = ProjectHarnessSettings::default();
        assert_eq!(overrides.resolve(&global), global);
        overrides.apply(ProjectHarnessEdit::ConsoleHarness(Some(
            ConsoleHarness::Opencode,
        )));
        let mut cli = global.codex.cli.clone();
        cli.binary = "project-wrapper".into();
        cli.inject.git_permissions = false;
        cli.permission_mode = cide_ipc::CodexPermissionMode::ApproveForMe;
        cli.permission_profile = "team".into();
        overrides.apply(ProjectHarnessEdit::Codex(Some(cli)));
        let resolved = overrides.resolve(&global);
        assert_eq!(resolved.console_harness, ConsoleHarness::Opencode);
        assert_eq!(resolved.codex.cli.binary, "project-wrapper");
        assert!(!resolved.codex.cli.inject.git_permissions);
        assert_eq!(
            resolved.codex.cli.permission_mode,
            cide_ipc::CodexPermissionMode::ApproveForMe
        );
        assert_eq!(
            global.codex.cli.permission_mode,
            cide_ipc::CodexPermissionMode::UseConfig
        );
        assert_eq!(resolved.codex.cli.permission_profile, "team");
        assert!(global.codex.cli.inject.git_permissions);
        assert_eq!(global.codex.cli.binary, "global-wrapper");
        overrides.apply(ProjectHarnessEdit::Codex(None));
        assert_eq!(overrides.resolve(&global).codex, global.codex);
    }

    #[test]
    fn project_defaults_are_isolated_and_survive_close_and_reopen() {
        let mut ws = Workspace::default();
        ws.settings.console_harness = ConsoleHarness::Codex;
        let one =
            crate::workspace::open_project(&mut ws, vec!["/project-one".into()], None).unwrap();
        let two =
            crate::workspace::open_project(&mut ws, vec!["/project-two".into()], None).unwrap();
        let cli = cide_ipc::OpencodeCli {
            binary: "project-opencode".into(),
            ..Default::default()
        };
        let key = project_key(&ws, one).unwrap();
        ws.project_harness.insert(
            key,
            ProjectHarnessSettings {
                console_harness: Some(ConsoleHarness::Opencode),
                opencode: Some(cli.clone()),
                ..Default::default()
            },
        );
        assert_eq!(
            effective(&ws, Some(one)).console_harness,
            ConsoleHarness::Opencode
        );
        assert_eq!(effective(&ws, Some(one)).opencode.cli, cli);
        assert_eq!(
            effective(&ws, Some(two)).console_harness,
            ConsoleHarness::Codex
        );
        assert_eq!(effective(&ws, None), ws.settings);
        ws.projects.shift_remove(&one);
        let reopened =
            crate::workspace::open_project(&mut ws, vec!["/project-one".into()], None).unwrap();
        assert_ne!(one, reopened);
        assert_eq!(effective(&ws, Some(reopened)).opencode.cli, cli);
    }

    #[test]
    fn storage_roundtrip_and_legacy_workspace() {
        let dir = std::env::temp_dir().join(format!("cide-harness-{}", cide_ipc::ProjectId::new()));
        let path = dir.join("harness.json");
        assert!(load(&path).unwrap().is_empty());
        let mut projects = Projects::new();
        projects.insert(
            "/one".into(),
            ProjectHarnessSettings {
                console_harness: Some(ConsoleHarness::Opencode),
                ..Default::default()
            },
        );
        save(&path, &projects).unwrap();
        assert_eq!(load(&path).unwrap(), projects);
        let mut json = serde_json::to_value(Workspace::default()).unwrap();
        json.as_object_mut().unwrap().remove("projectHarness");
        assert!(
            serde_json::from_value::<Workspace>(json)
                .unwrap()
                .project_harness
                .is_empty()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
