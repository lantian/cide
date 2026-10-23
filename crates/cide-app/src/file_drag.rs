//! Desktop drag receipts belong to the native window, never to a caller-supplied root list.
use std::{collections::VecDeque, path::PathBuf};

use parking_lot::Mutex;
use tauri::State;

use crate::{files::FsRegistry, workspace_state::WorkspaceState};
use cide_fs::FsError;
use cide_ipc::{PasteCollision, PasteDecision, PasteMode, PastedEntry, ProjectId, TabId};

#[derive(Default)]
pub struct DesktopDrops(Mutex<VecDeque<Receipt>>);

struct Receipt {
    id: String,
    window: String,
    paths: Vec<PathBuf>,
}

impl DesktopDrops {
    pub fn record(&self, window: &str, paths: &[PathBuf]) {
        let mut receipts = self.0.lock();
        receipts.push_back(Receipt {
            id: uuid::Uuid::new_v4().to_string(),
            window: window.into(),
            paths: paths.into(),
        });
        while receipts.len() > 32 {
            receipts.pop_front();
        }
    }

    fn claim(&self, window: &str, paths: &[PathBuf]) -> Result<String, FsError> {
        self.0
            .lock()
            .iter()
            .rev()
            .find(|r| r.window == window && r.paths == paths && !paths.is_empty())
            .map(|r| r.id.clone())
            .ok_or_else(|| {
                FsError::InvalidPath(
                    "These files were not dropped into this window. Drop them again.".into(),
                )
            })
    }

    fn paths(&self, window: &str, id: &str) -> Result<Vec<PathBuf>, FsError> {
        self.0
            .lock()
            .iter()
            .find(|r| r.window == window && r.id == id)
            .map(|r| r.paths.clone())
            .ok_or_else(|| {
                FsError::InvalidPath("The file drop expired. Drop the files again.".into())
            })
    }
}

fn source_roots(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter_map(|p| p.parent().map(PathBuf::from))
        .collect()
}

#[tauri::command(rename_all = "camelCase")]
pub fn fs_drop_claim(
    window: tauri::WebviewWindow,
    drops: State<'_, DesktopDrops>,
    paths: Vec<PathBuf>,
) -> Result<String, FsError> {
    drops.claim(window.label(), &paths)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn fs_drop_plan(
    window: tauri::WebviewWindow,
    drops: State<'_, DesktopDrops>,
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    receipt: String,
    dest_dir: PathBuf,
) -> Result<Vec<PasteCollision>, FsError> {
    let paths = drops.paths(window.label(), &receipt)?;
    let fs = crate::cmd::fs::project_fs(&registry, project)?;
    tauri::async_runtime::spawn_blocking(move || {
        cide_fs::copy::plan_between(
            &source_roots(&paths),
            &fs.writable_paths(),
            &paths,
            &dest_dir,
            PasteMode::Copy,
        )
    })
    .await
    .map_err(|e| FsError::InvalidPath(e.to_string()))?
}

#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::too_many_arguments)]
pub async fn fs_drop_paste(
    window: tauri::WebviewWindow,
    drops: State<'_, DesktopDrops>,
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    receipt: String,
    dest_dir: PathBuf,
    decisions: Vec<PasteDecision>,
) -> Result<Vec<PastedEntry>, FsError> {
    let paths = drops.paths(window.label(), &receipt)?;
    let fs = crate::cmd::fs::project_fs(&registry, project)?;
    tauri::async_runtime::spawn_blocking(move || {
        crate::cmd::fs::paste_with_source_roots(
            &fs,
            None,
            Some(&source_roots(&paths)),
            &paths,
            &dest_dir,
            PasteMode::Copy,
            &decisions,
        )
    })
    .await
    .map_err(|e| FsError::InvalidPath(e.to_string()))?
}

/// Dropping a file is the user's explicit choice to open it, including outside the project.
/// Keep terminal_open_path's regular-file, size and canonical-target checks.
#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::too_many_arguments)]
pub async fn fs_drop_open(
    window: tauri::WebviewWindow,
    drops: State<'_, DesktopDrops>,
    state: State<'_, WorkspaceState>,
    app: tauri::AppHandle,
    project: ProjectId,
    receipt: String,
    path: PathBuf,
) -> Result<TabId, String> {
    let paths = drops
        .paths(window.label(), &receipt)
        .map_err(|e| e.to_string())?;
    if !paths.contains(&path) {
        return Err("This file was not part of the drop.".into());
    }
    let checked = path.clone();
    let real = tauri::async_runtime::spawn_blocking(move || std::fs::canonicalize(checked))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    crate::cmd::file::terminal_open_path(state, app, project, path, Some(real))
        .await
        .map_err(|e| e.to_string())
}

/// Start the desktop's actual file-URL drag. The desktop copies; it never deletes our source.
#[tauri::command(rename_all = "camelCase")]
pub async fn fs_drag_start(
    window: tauri::WebviewWindow,
    registry: State<'_, FsRegistry>,
    project: ProjectId,
    paths: Vec<PathBuf>,
) -> Result<(), String> {
    let fs = crate::cmd::fs::project_fs(&registry, project).map_err(|e| e.to_string())?;
    if paths.is_empty() {
        return Err("There are no files to drag.".into());
    }
    // Export every real tree row, including dependency sources and project roots.
    let roots = fs.reveal_paths();
    for path in &paths {
        cide_fs::ops::check_within(&roots, path).map_err(|e| e.to_string())?;
        std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    let completion = std::sync::Arc::new(Mutex::new(Some(tx)));
    let callback = completion.clone();
    let native_window = window.clone();
    window
        .run_on_main_thread(move || {
            let callback_window = native_window.clone();
            let complete = move |result| {
                if let Some(tx) = callback.lock().take() {
                    // AppKit calls the source's end delegate before Tauri processes the
                    // destination's queued drop. Resolve on the next event-loop turn so JS
                    // retains the source provenance while handling that drop.
                    let callback_window = callback_window.clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = callback_window.run_on_main_thread(move || {
                            let _ = tx.send(result);
                        });
                    });
                }
            };
            // A very fast gesture can release while the IPC is still queued. Starting an
            // AppKit session after that release leaves a phantom drag attached to the mouse.
            #[cfg(target_os = "macos")]
            if objc2_app_kit::NSEvent::pressedMouseButtons() & 1 == 0 {
                complete(Ok(()));
                return;
            }
            #[cfg(any(
                target_os = "linux",
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd"
            ))]
            let native = native_window.gtk_window().map_err(|e| e.to_string());
            #[cfg(not(any(
                target_os = "linux",
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd"
            )))]
            let native: Result<_, String> = Ok(native_window);
            let result = native.and_then(|native| {
                drag::start_drag(
                    &native,
                    drag::DragItem::Files(paths),
                    drag::Image::Raw(include_bytes!("../icons/32x32.png").to_vec()),
                    move |_, _| complete(Ok(())),
                    drag::Options::default(),
                )
                .map_err(|e| e.to_string())
            });
            if let Err(error) = result
                && let Some(tx) = completion.lock().take()
            {
                let _ = tx.send(Err(error));
            }
        })
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Events;
    impl crate::files::FsEvents for Events {
        fn status(&self, _: ProjectId, _: &cide_ipc::FsStatus) {}
        fn changed(&self, _: ProjectId, _: &cide_ipc::FsChange) {}
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn desktop_drop_copies_external_folders_and_plans_collisions_before_writing() {
        use cide_fs::testing::scratch;
        let source = scratch("drop-source");
        let dest = scratch("drop-dest");
        std::fs::create_dir(source.path().join("folder")).unwrap();
        std::fs::write(source.path().join("folder/你好.txt"), "folder contents").unwrap();
        std::fs::write(source.path().join("a b.txt"), "new contents").unwrap();
        std::fs::write(dest.path().join("a b.txt"), "existing contents").unwrap();
        let registry = FsRegistry::default();
        let project = ProjectId::new();
        crate::cmd::fs::index_project(
            std::sync::Arc::new(Events),
            &registry,
            project,
            vec![dest.path().into()],
            cide_fs::Visibility::CONSERVATIVE,
        )
        .await
        .unwrap();
        let fs = registry.get(project).unwrap();
        let dropped = vec![source.path().join("folder"), source.path().join("a b.txt")];
        let drops = DesktopDrops::default();
        drops.record("main", &dropped);
        let id = drops.claim("main", &dropped).unwrap();
        let paths = drops.paths("main", &id).unwrap();
        let roots = source_roots(&paths);
        let collisions = cide_fs::copy::plan_between(
            &roots,
            &fs.writable_paths(),
            &paths,
            dest.path(),
            PasteMode::Copy,
        )
        .unwrap();
        assert_eq!(collisions.len(), 1);
        assert!(
            !dest.path().join("folder").exists(),
            "planning must write nothing"
        );
        assert_eq!(
            std::fs::read_to_string(dest.path().join("a b.txt")).unwrap(),
            "existing contents"
        );
        let pasted = crate::cmd::fs::paste_with_source_roots(
            &fs,
            None,
            Some(&roots),
            &paths,
            dest.path(),
            PasteMode::Copy,
            &[],
        )
        .unwrap();
        assert_eq!(pasted.len(), 2);
        assert!(
            paths.iter().all(|p| p.exists()),
            "desktop imports preserve every source"
        );
        assert_eq!(
            std::fs::read_to_string(dest.path().join("folder/你好.txt")).unwrap(),
            "folder contents"
        );
        assert!(
            pasted
                .iter()
                .all(|p| fs.with_index(|i| i.contains(&p.dest)))
        );
        assert!(pasted.iter().any(|p| p.renamed));
        assert_eq!(
            std::fs::read_to_string(dest.path().join("a b.txt")).unwrap(),
            "existing contents"
        );
        assert!(
            cide_fs::copy::plan_between(
                &roots,
                &fs.writable_paths(),
                &paths,
                source.path(),
                PasteMode::Copy
            )
            .is_err(),
            "native sources do not authorize an external destination"
        );
        drop(registry.remove(project));
    }

    #[test]
    fn receipts_authorize_only_native_paths_in_the_receiving_window() {
        let drops = DesktopDrops::default();
        let paths = vec![PathBuf::from("/outside/a b.txt")];
        assert!(drops.claim("main", &paths).is_err());
        drops.record("main", &paths);
        let id = drops.claim("main", &paths).unwrap();
        assert_eq!(drops.paths("main", &id).unwrap(), paths);
        assert!(drops.paths("other", &id).is_err());
        assert!(
            drops
                .claim("main", &[PathBuf::from("/outside/secret")])
                .is_err()
        );
        for _ in 0..32 {
            drops.record("main", &paths);
        }
        assert!(drops.paths("main", &id).is_err());
    }
}
