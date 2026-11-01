//! Private image copies for the OpenSpec composer. Draft paths and retained run paths
//! are separate so dismissing a dialog can never delete a queued run's inputs.
use super::spec::blocking;
use cide_core::{CoreError, Result};
use cide_ipc::{Harness, ImageDoc, ImageFormat};
use std::path::{Path, PathBuf};
use tauri::Manager;

fn directory(kind: &str) -> PathBuf {
    cide_core::persist::state_dir()
        .join("spec-images")
        .join(kind)
}
fn checked(path: &Path, kind: &str) -> Result<PathBuf> {
    let root = directory(kind).canonicalize()?;
    let path = path.canonicalize()?;
    if path.parent() != Some(root.as_path()) {
        return Err(CoreError::Io("Not an OpenSpec image copy".into()));
    }
    Ok(path)
}
fn validate(path: &Path) -> Result<ImageDoc> {
    let doc = cide_core::image::read(path)?;
    if !matches!(
        doc.format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::Webp
    ) {
        return Err(CoreError::Io(
            "Attach a PNG, JPEG, GIF, or WebP image".into(),
        ));
    }
    Ok(doc)
}
fn save(bytes: &[u8], extension: &str, kind: &str) -> Result<PathBuf> {
    save_in(bytes, extension, &directory(kind))
}
fn save_in(bytes: &[u8], extension: &str, directory: &Path) -> Result<PathBuf> {
    if bytes.len() as u64 > cide_core::image::MAX_IMAGE_BYTES {
        return Err(CoreError::Io("Images must be at most 32 MiB".into()));
    }
    let path = directory.join(format!("{}.{}", uuid::Uuid::new_v4(), extension));
    cide_core::persist::write_atomic_with_mode(&path, bytes, cide_core::persist::PRIVATE_MODE)?;
    if let Err(error) = validate(&path) {
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    Ok(path)
}
fn extension(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Gif => "gif",
        ImageFormat::Webp => "webp",
        _ => "png",
    }
}

#[tauri::command(rename_all = "camelCase")]
pub async fn spec_images_stage(
    app: tauri::AppHandle,
    paths: Option<Vec<PathBuf>>,
) -> Result<Vec<ImageDoc>> {
    let handle = app.clone();
    let docs = blocking(move || {
        let mut saved = Vec::new();
        let result = (|| -> Result<()> {
            if let Some(paths) = paths {
                for path in paths {
                    let doc = validate(&path)?;
                    let bytes = std::fs::read(&path)?;
                    saved.push(save(&bytes, extension(doc.format), "drafts")?);
                }
            } else {
                use tauri_plugin_clipboard_manager::ClipboardExt;
                if let Ok(image) = handle.clipboard().read_image() {
                    let bytes =
                        cide_core::image::encode_png(image.rgba(), image.width(), image.height())?;
                    saved.push(save(&bytes, "png", "drafts")?);
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            for path in saved {
                let _ = std::fs::remove_file(path);
            }
            return Err(error);
        }
        saved
            .iter()
            .map(|path| validate(path))
            .collect::<Result<Vec<_>>>()
    })
    .await?;
    for doc in &docs {
        app.asset_protocol_scope()
            .allow_file(&doc.path)
            .map_err(|e| CoreError::Io(e.to_string()))?;
    }
    Ok(docs)
}
#[tauri::command(rename_all = "camelCase")]
pub async fn spec_images_release(paths: Vec<PathBuf>) -> Result<()> {
    blocking(move || {
        for path in paths {
            if let Ok(path) = checked(&path, "drafts") {
                let _ = std::fs::remove_file(path);
            }
        }
        Ok(())
    })
    .await
}
#[tauri::command]
pub async fn spec_images_pick(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> Result<Vec<PathBuf>> {
    let (tx, rx) = std::sync::mpsc::channel();
    super::project::show_picker(
        &app,
        window,
        super::project::PickerSpec {
            title: "Attach images",
            accept: "Attach",
            folders: false,
            multiple: true,
            filter: Some(("Images", &["png", "jpg", "jpeg", "gif", "webp"])),
        },
        tx,
    )?;
    super::project::receive_picker(rx).await
}

pub(crate) fn retain(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let sources = paths
        .iter()
        .map(|path| {
            let path = checked(path, "drafts")?;
            validate(&path)?;
            Ok(path)
        })
        .collect::<Result<Vec<_>>>()?;
    let mut retained = Vec::new();
    for source in sources {
        let result = (|| {
            let doc = validate(&source)?;
            save(&std::fs::read(&source)?, extension(doc.format), "runs")
        })();
        match result {
            Ok(path) => retained.push(path),
            Err(error) => {
                for path in retained {
                    let _ = std::fs::remove_file(path);
                }
                return Err(error);
            }
        }
    }
    Ok(retained)
}
pub(crate) fn opening_prompt(harness: Harness, text: &str, images: &[PathBuf]) -> String {
    if images.is_empty() || harness == Harness::Codex {
        return text.into();
    }
    let mut prompt = format!("{text} Inspect these attached images before answering:");
    for (index, path) in images.iter().enumerate() {
        prompt.push_str(&format!(
            " Image {}: {}.",
            index + 1,
            serde_json::to_string(&path.to_string_lossy()).unwrap()
        ));
    }
    prompt
}
/// Called with the registry's retained image set after snapshot restoration. A draft
/// abandoned by a crashed window is disposable after a day; run copies follow history.
pub(crate) fn sweep(retained: &std::collections::HashSet<PathBuf>) {
    for kind in ["drafts", "runs"] {
        if let Ok(entries) = std::fs::read_dir(directory(kind)) {
            for entry in entries.flatten() {
                let old = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age.as_secs() > 86400);
                if (kind == "runs" && !retained.contains(&entry.path()))
                    || (kind == "drafts" && old)
                {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
}

pub(crate) fn validate_retained(paths: &[PathBuf]) -> Result<()> {
    for path in paths {
        validate(&checked(path, "runs")?)?;
    }
    Ok(())
}

pub(crate) fn release_retained(paths: &[PathBuf]) {
    for path in paths {
        if let Ok(path) = checked(path, "runs") {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_copies_are_independent_and_validate_content() {
        let root =
            std::env::temp_dir().join(format!("cide-spec-images-test-{}", uuid::Uuid::new_v4()));
        let bytes = cide_core::image::encode_png(&[255, 0, 0, 255], 1, 1).unwrap();
        let draft = save_in(&bytes, "png", &root.join("drafts")).unwrap();
        let run = save_in(&std::fs::read(&draft).unwrap(), "png", &root.join("runs")).unwrap();
        let another = save_in(&bytes, "png", &root.join("drafts")).unwrap();
        assert_ne!(draft, another);
        std::fs::remove_file(draft).unwrap();
        assert_eq!(std::fs::read(run).unwrap(), bytes);
        assert!(save_in(b"not an image", "png", &root).is_err());
        assert!(
            save_in(
                b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>",
                "svg",
                &root
            )
            .is_err()
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
    #[test]
    fn opening_images_preserve_order_and_single_turn() {
        let paths = vec![
            PathBuf::from("/tmp/a b.png"),
            PathBuf::from("/tmp/Привет\n.png"),
        ];
        let text = "/openspec-propose Fix the layout";
        assert_eq!(opening_prompt(Harness::Codex, text, &paths), text);
        assert_eq!(opening_prompt(Harness::Claude, text, &[]), text);
        for harness in [
            Harness::Claude,
            Harness::Qwen,
            Harness::Opencode,
            Harness::Mimo,
        ] {
            let prompt = opening_prompt(harness, text, &paths);
            assert!(prompt.starts_with(text));
            assert!(prompt.contains("Image 1: \"/tmp/a b.png\""));
            assert!(!prompt.contains('\n'));
            assert!(prompt.find("Image 1").unwrap() < prompt.find("Image 2").unwrap());
        }
    }
}
