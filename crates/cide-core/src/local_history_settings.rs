//! Whether one project keeps a Local History. Machine-local, absence inherits the global switch.
//!
//! A file of its own, beside `indexing-settings.json` and for its reason: `.cide/config.json` is
//! committed and shared, and whether *this machine* keeps copies of a project's files is not a
//! decision to make for a teammate.
use crate::{CoreError, Result};
use cide_ipc::{ProjectId, Workspace};
use std::{collections::BTreeMap, path::Path};

pub type Projects = BTreeMap<String, bool>;

pub fn overrides(ws: &Workspace, project: ProjectId) -> Result<Option<bool>> {
    Ok(ws
        .project_local_history
        .get(&crate::harness_settings::project_key(ws, project)?)
        .copied())
}

/// The one answer to "does this project capture". Everything that decides asks this, so the
/// override and the global switch cannot be read in two orders.
pub fn effective(ws: &Workspace, project: ProjectId) -> bool {
    overrides(ws, project)
        .ok()
        .flatten()
        .unwrap_or(ws.settings.local_history.enabled)
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
    fn a_project_override_beats_the_global_switch_in_both_directions() {
        let mut ws = Workspace::default();
        let project = crate::workspace::open_project(&mut ws, vec!["/one".into()], None).unwrap();
        let key = crate::harness_settings::project_key(&ws, project).unwrap();

        ws.settings.local_history.enabled = true;
        assert!(effective(&ws, project), "no entry inherits");
        ws.project_local_history.insert(key.clone(), false);
        assert!(!effective(&ws, project), "off here while on everywhere");

        ws.settings.local_history.enabled = false;
        ws.project_local_history.insert(key.clone(), true);
        assert!(effective(&ws, project), "on here while off everywhere");
        ws.project_local_history.remove(&key);
        assert!(!effective(&ws, project), "back to inheriting");
    }

    #[test]
    fn overrides_survive_a_round_trip_and_a_missing_file_is_empty() {
        let dir = std::env::temp_dir().join(format!("cide-lh-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("local-history-settings.json");
        let _ = std::fs::remove_file(&file);
        assert!(load(&file).unwrap().is_empty());
        let projects = Projects::from([("/one".to_owned(), false), ("/two".to_owned(), true)]);
        save(&file, &projects).unwrap();
        assert_eq!(load(&file).unwrap(), projects);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
