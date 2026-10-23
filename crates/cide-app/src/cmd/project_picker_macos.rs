//! Fallible AppKit picker. `rfd 0.16` calls `NSOpenPanel::openPanel`, whose generated
//! return type is `Retained<NSOpenPanel>`. In practice AppKit can return nil when its
//! separate Open/Save panel service fails (for example after replacing a running app's
//! bundle). objc2 then panics on the main thread and our release profile aborts cide.
//!
//! Read that one return as nullable, and send failures through the same channel as the
//! selection. A catch_unwind around rfd cannot help a panic=abort build. The sheet stays
//! asynchronous and parented; no nested runModal loop or change to signing entitlements.

use std::{path::PathBuf, sync::mpsc::Sender};

use block2::RcBlock;
use cide_core::CoreError;
use objc2::{ClassType, MainThreadMarker, msg_send, rc::Retained};
use objc2_app_kit::{
    NSModalResponse, NSModalResponseCancel, NSModalResponseOK, NSOpenPanel, NSWindow,
};
use objc2_foundation::{NSArray, NSString};

use super::{PickerReply, PickerSpec};

fn unavailable() -> CoreError {
    CoreError::Io(
        "macOS could not open the file picker. Restart cide and try again. \
         If cide was updated while running, restart it to finish the update."
            .into(),
    )
}

fn checked_panel<T>(panel: Option<T>) -> Result<T, CoreError> {
    panel.ok_or_else(unavailable)
}

/// SAFETY: called on the main thread, with the marker proving AppKit may be used.
/// The selector's ownership convention is unchanged; only its nullability differs
/// from the generated binding. `Option<Retained<_>>` checks nil before any dereference.
fn create_panel(_mtm: MainThreadMarker) -> Result<Retained<NSOpenPanel>, CoreError> {
    let panel: Option<Retained<NSOpenPanel>> =
        unsafe { msg_send![NSOpenPanel::class(), openPanel] };
    checked_panel(panel)
}

fn selection(response: NSModalResponse, paths: impl FnOnce() -> PickerReply) -> PickerReply {
    if response == NSModalResponseOK {
        paths()
    } else if response == NSModalResponseCancel {
        Ok(Vec::new())
    } else {
        // AppKit also reports NSModalResponseAbort when it cannot display the panel.
        Err(unavailable())
    }
}

pub(crate) fn show_picker(
    app: &tauri::AppHandle,
    window: tauri::WebviewWindow,
    spec: PickerSpec,
    tx: Sender<PickerReply>,
) -> Result<(), CoreError> {
    app.run_on_main_thread(move || {
        let show = || -> Result<(), CoreError> {
            let mtm = MainThreadMarker::new()
                .ok_or_else(|| CoreError::Io("file picker must run on the main thread".into()))?;
            let panel = create_panel(mtm)?;
            let parent = window
                .ns_window()
                .map_err(|error| CoreError::Io(format!("file picker parent: {error}")))?;
            // Tauri owns this NSWindow; retain it on the main thread while starting the sheet.
            let parent = unsafe { Retained::retain(parent.cast::<NSWindow>()) }
                .ok_or_else(|| CoreError::Io("file picker parent window is closed".into()))?;
            panel.setTitle(Some(&NSString::from_str(spec.title)));
            panel.setPrompt(Some(&NSString::from_str(spec.accept)));
            panel.setCanChooseDirectories(spec.folders);
            panel.setCanChooseFiles(!spec.folders);
            panel.setAllowsMultipleSelection(spec.multiple);
            panel.setCanCreateDirectories(true);
            if let Some((_, patterns)) = spec.filter {
                let extensions: Vec<_> = patterns
                    .iter()
                    .map(|pattern| NSString::from_str(pattern.trim_start_matches("*.")))
                    .collect();
                #[allow(deprecated)]
                panel.setAllowedFileTypes(Some(&NSArray::from_retained_slice(&extensions)));
                panel.setAllowsOtherFileTypes(true);
            }

            let held = panel.clone();
            let answer = tx.clone();
            let completion = RcBlock::new(move |response| {
                // AppKit retains the completion until it answers. Holding the panel here
                // keeps URLs valid until read; dropping the completion releases our reference.
                let picked = selection(response, || {
                    held.URLs()
                        .iter()
                        .map(|url| {
                            url.to_file_path().ok_or_else(|| {
                                CoreError::Io(
                                    "macOS returned a selection without a file path".into(),
                                )
                            })
                        })
                        .collect::<Result<Vec<PathBuf>, CoreError>>()
                });
                let _ = answer.send(picked);
            });
            panel.beginSheetModalForWindow_completionHandler(&parent, &completion);
            Ok(())
        };
        if let Err(error) = show() {
            tracing::warn!(%error, "native file picker failed");
            let _ = tx.send(Err(error));
        }
    })
    .map_err(|error| CoreError::Io(format!("file picker: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_app_kit::NSModalResponseAbort;

    #[test]
    fn native_nil_reaches_the_command_as_an_error_not_a_cancellation() {
        // A real Objective-C nil return, without opening a window or requiring the test
        // runner to be the AppKit main thread. This exercises the nullable conversion
        // and the same result channel every picker command awaits.
        let array = NSArray::<NSString>::new();
        let native_nil: Option<Retained<NSString>> = unsafe { msg_send![&*array, firstObject] };
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(checked_panel(native_nil).map(|_| Vec::new()))
            .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let error = runtime
            .block_on(super::super::receive_picker(rx))
            .unwrap_err();
        assert!(error.to_string().contains("Restart cide"));
    }

    #[test]
    fn cancellation_and_service_failure_do_not_read_urls() {
        assert!(
            selection(NSModalResponseCancel, || panic!("cancelled"))
                .unwrap()
                .is_empty()
        );
        assert!(selection(NSModalResponseAbort, || panic!("unavailable")).is_err());
        let paths = vec![PathBuf::from("/tmp/проект с пробелами")];
        assert_eq!(
            selection(NSModalResponseOK, || Ok(paths.clone())).unwrap(),
            paths
        );
    }
}
