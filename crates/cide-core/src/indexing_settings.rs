//! Machine-local project indexing preferences. Absence inherits global exclusions.
use crate::{CoreError, Result};
use cide_ipc::{ProjectId, Workspace};
use std::{collections::BTreeMap, path::Path};

pub type Projects = BTreeMap<String, Vec<String>>;

pub fn overrides(ws: &Workspace, project: ProjectId) -> Result<Option<Vec<String>>> {
    Ok(ws
        .project_indexing
        .get(&crate::harness_settings::project_key(ws, project)?)
        .cloned())
}

pub fn effective(ws: &Workspace, project: ProjectId) -> Vec<String> {
    overrides(ws, project)
        .ok()
        .flatten()
        .unwrap_or_else(|| ws.settings.explorer.index_excluded_folders.clone())
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
    crate::persist::write_atomic_with_mode(path, &bytes, 0o600)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_overrides_replace_inherit_and_survive_reopening() {
        let mut ws = Workspace::default();
        let global = ws.settings.explorer.index_excluded_folders.clone();
        assert_eq!(global, [".cide/worktrees", ".cide/tasks"]);
        let one = crate::workspace::open_project(&mut ws, vec!["/one".into()], None).unwrap();
        let two = crate::workspace::open_project(&mut ws, vec!["/two".into()], None).unwrap();
        let key = crate::harness_settings::project_key(&ws, one).unwrap();
        assert_eq!(overrides(&ws, one).unwrap(), None);
        ws.project_indexing.insert(key.clone(), vec![]);
        assert!(effective(&ws, one).is_empty());
        assert_eq!(effective(&ws, two), global);
        ws.settings.explorer.index_excluded_folders = vec!["**/target".into()];
        assert!(effective(&ws, one).is_empty());
        assert_eq!(effective(&ws, two), ["**/target"]);
        let dir = std::env::temp_dir().join(format!("cide-indexing-{}", ProjectId::new()));
        let file = dir.join("indexing-settings.json");
        assert!(load(&file).unwrap().is_empty());
        save(&file, &ws.project_indexing).unwrap();
        ws.project_indexing = load(&file).unwrap();
        ws.projects.shift_remove(&one);
        let reopened = crate::workspace::open_project(&mut ws, vec!["/one".into()], None).unwrap();
        assert_eq!(overrides(&ws, reopened).unwrap(), Some(vec![]));
        ws.project_indexing
            .insert(key.clone(), vec!["build".into()]);
        assert_eq!(effective(&ws, reopened), ["build"]);
        ws.project_indexing.remove(&key);
        assert_eq!(effective(&ws, reopened), ["**/target"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_fields_receive_defaults_but_explicit_empty_lists_do_not() {
        let old: cide_ipc::ExplorerSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(
            old.index_excluded_folders,
            [".cide/worktrees", ".cide/tasks"]
        );
        let empty: cide_ipc::ExplorerSettings =
            serde_json::from_str(r#"{"indexExcludedFolders":[]}"#).unwrap();
        assert!(empty.index_excluded_folders.is_empty());
        let mut json = serde_json::to_value(Workspace::default()).unwrap();
        json.as_object_mut().unwrap().remove("projectIndexing");
        assert!(
            serde_json::from_value::<Workspace>(json)
                .unwrap()
                .project_indexing
                .is_empty()
        );
    }
}
