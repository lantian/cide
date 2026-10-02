//! Materialize Git image bytes off the UI thread and grant only those immutable asset paths.
use cide_core::image::SnapshotCache;
use cide_git::image_diff::{ImageBytes, ImageDiffBytes};
use cide_ipc::image::{ImageDiff, ImageDiffSide};
use parking_lot::Mutex;
use tauri::Manager;

#[derive(Default)]
pub struct ImageSnapshots(Mutex<SnapshotCache>);

impl ImageSnapshots {
    pub fn materialize(&self, app: &tauri::AppHandle, bytes: ImageDiffBytes) -> ImageDiff {
        let mut cache = self.0.lock();
        let mut side = |bytes| match bytes {
            ImageBytes::Absent => ImageDiffSide::Absent,
            ImageBytes::Unavailable(reason) => ImageDiffSide::Unavailable { reason },
            ImageBytes::Ready(bytes) => match cache.store(&bytes) {
                Ok(doc) => match app.asset_protocol_scope().allow_file(&doc.path) {
                    Ok(()) => ImageDiffSide::Ready { doc },
                    Err(e) => ImageDiffSide::Unavailable {
                        reason: format!("The image could not be served: {e}"),
                    },
                },
                Err(e) => ImageDiffSide::Unavailable {
                    reason: e.to_string(),
                },
            },
        };
        ImageDiff {
            old_path: bytes.old_path,
            old: side(bytes.old),
            new: side(bytes.new),
        }
    }

    pub fn clear(&self) {
        self.0.lock().clear();
    }
}
