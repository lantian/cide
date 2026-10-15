//! Native file clipboard transport. GTK requests are asynchronous on the main loop: waiting
//! for a selection on that thread can deadlock when its owner is one of our own webviews.

use cide_core::file_clip::NativeFileClip;
use cide_fs::FsError;
use tokio::sync::oneshot;

fn error(message: impl Into<String>) -> FsError {
    FsError::Io {
        path: "file clipboard".into(),
        message: message.into(),
    }
}

async fn on_main<T: Send + 'static>(
    app: &tauri::AppHandle,
    job: impl FnOnce(oneshot::Sender<Result<T, String>>) + Send + 'static,
) -> Result<T, FsError> {
    let (tx, rx) = oneshot::channel();
    app.run_on_main_thread(move || job(tx))
        .map_err(|e| error(e.to_string()))?;
    tokio::time::timeout(std::time::Duration::from_secs(3), rx)
        .await
        .map_err(|_| error("the clipboard owner did not respond"))?
        .map_err(|_| error("the clipboard request was interrupted"))?
        .map_err(error)
}

pub async fn read(app: &tauri::AppHandle) -> Result<Option<NativeFileClip>, FsError> {
    on_main(app, |tx| {
        platform::read(move |result| {
            let _ = tx.send(result);
        })
    })
    .await
}

pub async fn write(app: &tauri::AppHandle, clip: NativeFileClip) -> Result<(), FsError> {
    let bytes = clip.encode().map_err(error)?;
    let text = clip.text();
    on_main(app, move |tx| {
        let _ = tx.send(platform::write(bytes, text));
    })
    .await
}

/// Never discard a Copy made while a large paste or a collision dialog was in progress.
pub async fn clear_if(app: &tauri::AppHandle, id: String) -> Result<bool, FsError> {
    on_main(app, move |tx| {
        clear_matching(id, move |result| {
            let _ = tx.send(result);
        })
    })
    .await
}

fn clear_matching(id: String, done: impl FnOnce(Result<bool, String>) + 'static) {
    platform::read(move |result| {
        done(result.map(|clip| {
            if clip.as_ref().is_some_and(|clip| clip.clip.id == id) {
                platform::clear();
                true
            } else {
                false
            }
        }));
    });
}

/// Called on the main thread at exit. On macOS the native board survives the process;
/// discard only our own clip, rather than leaving a pending Cut armed across restarts.
pub fn release_owned(id: String) {
    platform::read(move |result| {
        if result.ok().flatten().is_some_and(|c| c.clip.id == id) {
            platform::clear();
        }
    });
}

#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd"
))]
mod platform {
    use super::*;
    use cide_core::file_clip::{FORMAT, GNOME_FILES, KDE_CUT, MAX_BYTES, URI_LIST, kde_mode};
    use gtk::prelude::ObjectExt;

    thread_local! {
        static EPOCH: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    }

    fn epoch() -> String {
        let first = EPOCH.with(|state| {
            let mut state = state.borrow_mut();
            if state.is_some() {
                false
            } else {
                *state = Some(uuid::Uuid::new_v4().to_string());
                true
            }
        });
        if first {
            // gtk-rs omits the typed owner-change binding, because its event argument is not
            // introspectable. The generic signal needs no event fields: each change is a new
            // selection even if a file manager recopies exactly the same paths.
            gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD).connect_local(
                "owner-change",
                false,
                |_| {
                    EPOCH
                        .with(|state| *state.borrow_mut() = Some(uuid::Uuid::new_v4().to_string()));
                    None
                },
            );
        }
        EPOCH.with(|state| {
            state
                .borrow()
                .as_ref()
                .expect("initialized clipboard epoch")
                .clone()
        })
    }

    fn request(target: &str, done: impl FnOnce(Result<Option<Vec<u8>>, String>) + 'static) {
        let clipboard = gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD);
        clipboard.request_contents(&gtk::gdk::Atom::intern(target), move |_, data| {
            done(if data.length() < 0 {
                Ok(None)
            } else if data.length() as usize > MAX_BYTES {
                Err("file clipboard payload is too large".into())
            } else {
                Ok(Some(data.data()))
            });
        });
    }

    pub fn read(done: impl FnOnce(Result<Option<NativeFileClip>, String>) + 'static) {
        epoch();
        request(FORMAT, move |data| match data {
            Ok(Some(bytes)) => done(NativeFileClip::decode(&bytes).map(Some)),
            Ok(None) => read_external(done),
            Err(error) => done(Err(error)),
        });
    }

    fn read_external(done: impl FnOnce(Result<Option<NativeFileClip>, String>) + 'static) {
        let selection = epoch();
        // GNOME supplies mode and URLs together. Dolphin supplies URI_LIST plus KDE_CUT.
        // Never use text/plain: copying a path as text must not arm a file operation.
        request(GNOME_FILES, move |data| match data {
            Ok(Some(bytes)) => finish_external(
                &selection,
                NativeFileClip::from_gnome_files(&bytes, &selection),
                done,
            ),
            Ok(None) => request(URI_LIST, move |data| match data {
                Ok(Some(bytes)) => request(KDE_CUT, move |marker| {
                    let result = marker.and_then(|marker| {
                        let mode = kde_mode(marker.as_deref())?;
                        NativeFileClip::from_uri_list(&bytes, mode, &selection)
                    });
                    finish_external(&selection, result, done);
                }),
                Ok(None) => done(Ok(None)),
                Err(error) => done(Err(error)),
            }),
            Err(error) => done(Err(error)),
        });
    }

    fn finish_external(
        selection: &str,
        result: Result<Option<NativeFileClip>, String>,
        done: impl FnOnce(Result<Option<NativeFileClip>, String>),
    ) {
        if selection != epoch() {
            done(Err(
                "The clipboard changed while reading it. Paste again.".into()
            ));
        } else {
            done(result);
        }
    }

    pub fn write(bytes: Vec<u8>, text: String) -> Result<(), String> {
        let clipboard = gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD);
        let targets = [
            gtk::TargetEntry::new(FORMAT, gtk::TargetFlags::empty(), 0),
            gtk::TargetEntry::new("UTF8_STRING", gtk::TargetFlags::empty(), 1),
            gtk::TargetEntry::new("text/plain;charset=utf-8", gtk::TargetFlags::empty(), 1),
            gtk::TargetEntry::new("text/plain", gtk::TargetFlags::empty(), 1),
            gtk::TargetEntry::new("TEXT", gtk::TargetFlags::empty(), 1),
            gtk::TargetEntry::new("STRING", gtk::TargetFlags::empty(), 1),
        ];
        if clipboard.set_with_data(&targets, move |_, data, info| {
            if info == 0 {
                data.set(&gtk::gdk::Atom::intern(FORMAT), 8, &bytes);
            } else {
                data.set_text(&text);
            }
        }) {
            Ok(())
        } else {
            Err("could not take ownership of the file clipboard".into())
        }
    }

    pub fn clear() {
        // `gtk_clipboard_clear` only releases this process's selection. Acquiring an empty
        // selection also consumes a Cut owned by a *different* cide process.
        gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD).set_text("");
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use cide_core::file_clip::FORMAT;
    use objc2::ClassType;
    use objc2_app_kit::{
        NSPasteboard, NSPasteboardTypeString, NSPasteboardURLReadingFileURLsOnlyKey,
    };
    use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSURL};

    pub fn read(done: impl FnOnce(Result<Option<NativeFileClip>, String>) + 'static) {
        let board = NSPasteboard::generalPasteboard();
        if let Some(value) = board.stringForType(&NSString::from_str(FORMAT)) {
            done(NativeFileClip::decode(value.to_string().as_bytes()).map(Some));
            return;
        }
        let epoch = board.changeCount();
        let classes = NSArray::from_slice(&[NSURL::class()]);
        let options = NSDictionary::from_slices(
            &[unsafe { NSPasteboardURLReadingFileURLsOnlyKey }],
            &[NSNumber::new_bool(true).as_ref()],
        );
        let objects = unsafe { board.readObjectsForClasses_options(&classes, Some(&options)) };
        let mut urls = Vec::new();
        if let Some(objects) = objects {
            for i in 0..objects.count() {
                let object = objects.objectAtIndex(i);
                if let Ok(url) = object.downcast::<NSURL>()
                    && let Some(value) = url.absoluteString()
                {
                    urls.push(value.to_string());
                }
            }
        }
        let result = NativeFileClip::from_uri_list(
            urls.join("\r\n").as_bytes(),
            cide_ipc::PasteMode::Copy,
            &epoch.to_string(),
        );
        if board.changeCount() != epoch {
            done(Err(
                "The clipboard changed while reading it. Paste again.".into()
            ));
        } else {
            done(result);
        }
    }

    pub fn write(bytes: Vec<u8>, text: String) -> Result<(), String> {
        let board = NSPasteboard::generalPasteboard();
        let json = String::from_utf8(bytes).map_err(|e| e.to_string())?;
        board.clearContents();
        let ok = board.setString_forType(&NSString::from_str(&json), &NSString::from_str(FORMAT));
        let text_ok =
            unsafe { board.setString_forType(&NSString::from_str(&text), NSPasteboardTypeString) };
        if ok && text_ok {
            Ok(())
        } else {
            Err("could not write the file clipboard".into())
        }
    }

    pub fn clear() {
        NSPasteboard::generalPasteboard().clearContents();
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "macos"
)))]
mod platform {
    use super::*;
    pub fn read(done: impl FnOnce(Result<Option<NativeFileClip>, String>) + 'static) {
        done(Err(
            "native file clipboard is unsupported on this platform".into()
        ));
    }
    pub fn write(_: Vec<u8>, _: String) -> Result<(), String> {
        Err("native file clipboard is unsupported on this platform".into())
    }
    pub fn clear() {}
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::{
        cell::RefCell,
        path::Path,
        process::{Child, Command},
        rc::Rc,
        time::{Duration, Instant},
    };

    fn pump_until(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready() {
            assert!(Instant::now() < deadline, "native clipboard test timed out");
            while gtk::glib::MainContext::default().iteration(false) {}
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn receive<T: 'static>(start: impl FnOnce(Box<dyn FnOnce(T)>)) -> T {
        let result = Rc::new(RefCell::new(None));
        let answer = result.clone();
        start(Box::new(move |value| {
            *answer.borrow_mut() = Some(value);
        }));
        pump_until(|| result.borrow().is_some());
        result.borrow_mut().take().unwrap()
    }

    fn native() -> Option<NativeFileClip> {
        receive(platform::read).unwrap()
    }

    fn send(clip: &NativeFileClip) {
        platform::write(clip.encode().unwrap(), clip.text()).unwrap();
    }

    struct Process(Child);
    impl Drop for Process {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn spawn(root: &Path, role: &str, test: &str) -> Process {
        Process(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", test, "--ignored", "--nocapture"])
                .env("CIDE_CLIPBOARD_TEST_ROOT", root)
                .env("CIDE_CLIPBOARD_TEST_ROLE", role)
                .spawn()
                .unwrap(),
        )
    }

    /// Run on an isolated X display: this deliberately exercises the real desktop clipboard.
    /// The source and receiver are independent OS processes, with no shared Rust state.
    #[test]
    #[ignore = "requires an isolated X display and CIDE_NATIVE_CLIPBOARD_TEST=1"]
    fn native_clipboard_between_processes() {
        assert_eq!(
            std::env::var("CIDE_NATIVE_CLIPBOARD_TEST").as_deref(),
            Ok("1")
        );
        assert_eq!(
            std::env::var("GDK_BACKEND").as_deref(),
            Ok("x11"),
            "force the isolated X display instead of an inherited Wayland desktop"
        );
        let Ok(root) = std::env::var("CIDE_CLIPBOARD_TEST_ROOT") else {
            let root = std::env::temp_dir()
                .join(format!("cide-native-clipboard-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap();
            let mut owner = spawn(
                &root,
                "owner",
                "file_clipboard::tests::native_clipboard_between_processes",
            );
            pump_until(|| root.join("ready").exists() || owner.0.try_wait().unwrap().is_some());
            assert!(root.join("ready").exists(), "clipboard owner failed");
            let mut receiver = spawn(
                &root,
                "receiver",
                "file_clipboard::tests::native_clipboard_between_processes",
            );
            pump_until(|| receiver.0.try_wait().unwrap().is_some());
            assert!(
                receiver.0.wait().unwrap().success(),
                "clipboard receiver failed"
            );
            pump_until(|| owner.0.try_wait().unwrap().is_some());
            assert!(owner.0.wait().unwrap().success(), "clipboard owner failed");
            std::fs::remove_dir_all(root).unwrap();
            return;
        };
        gtk::init().unwrap();
        let root = std::path::PathBuf::from(root);
        match std::env::var("CIDE_CLIPBOARD_TEST_ROLE").unwrap().as_str() {
            "owner" => {
                let mut clip = NativeFileClip::new(
                    uuid::Uuid::new_v4().to_string(),
                    cide_ipc::FileClip {
                        id: String::new(),
                        project: Some(cide_ipc::ProjectId::new()),
                        mode: cide_ipc::PasteMode::Copy,
                        paths: vec!["/source/a b.txt".into(), "/source/你好\nname".into()],
                    },
                    vec!["/source".into()],
                )
                .unwrap();
                std::fs::write(root.join("copy.json"), clip.encode().unwrap()).unwrap();
                send(&clip);
                std::fs::write(root.join("ready"), []).unwrap();
                pump_until(|| root.join("want-cut").exists());
                clip = NativeFileClip::new(
                    clip.instance,
                    cide_ipc::FileClip {
                        mode: cide_ipc::PasteMode::Cut,
                        ..clip.clip
                    },
                    clip.roots,
                )
                .unwrap();
                std::fs::write(root.join("cut.json"), clip.encode().unwrap()).unwrap();
                send(&clip);
                std::fs::write(root.join("cut-ready"), []).unwrap();
                pump_until(|| root.join("done").exists());
                assert!(
                    native().is_none(),
                    "source instance still sees the consumed Cut"
                );
                std::fs::write(root.join("owner-saw-clear"), []).unwrap();
            }
            "receiver" => {
                let copy = NativeFileClip::decode(&std::fs::read(root.join("copy.json")).unwrap())
                    .unwrap();
                for _ in 0..2 {
                    assert_eq!(native(), Some(copy.clone()));
                }
                let text = receive(|done| {
                    gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD)
                        .request_text(move |_, text| done(text.map(|s| s.to_string())))
                });
                assert_eq!(
                    text,
                    Some(copy.text()),
                    "structured metadata replaced the path text"
                );
                std::fs::write(root.join("want-cut"), []).unwrap();
                pump_until(|| root.join("cut-ready").exists());
                let cut =
                    NativeFileClip::decode(&std::fs::read(root.join("cut.json")).unwrap()).unwrap();
                assert_eq!(native(), Some(cut.clone()));
                let cleared = receive(|done| clear_matching(copy.clip.id, done)).unwrap();
                assert!(!cleared, "a stale gesture cleared a newer Cut");
                assert_eq!(native(), Some(cut.clone()));
                assert!(receive(|done| clear_matching(cut.clip.id, done)).unwrap());
                assert!(native().is_none());
                std::fs::write(root.join("done"), []).unwrap();
                // Keep the empty selection alive while its previous owner reads it.
                pump_until(|| root.join("owner-saw-clear").exists());
            }
            role => panic!("unknown clipboard test role: {role}"),
        }
    }
    // Like Dolphin/GNOME, this owner publishes only OS targets, with no cide metadata.
    fn send_os(kind: &str, uris: &str) {
        use cide_core::file_clip::{GNOME_FILES, KDE_CUT, URI_LIST};
        let formats = match kind {
            "gnome-copy" => vec![(GNOME_FILES, format!("copy\n{uris}").into_bytes())],
            "gnome-cut" => vec![(GNOME_FILES, format!("cut\n{uris}").into_bytes())],
            "kde-cut" => vec![
                (URI_LIST, uris.as_bytes().to_vec()),
                (KDE_CUT, b"1".to_vec()),
            ],
            "text-only" => vec![("UTF8_STRING", uris.as_bytes().to_vec())],
            "remote" => vec![(URI_LIST, b"sftp://server/file".to_vec())],
            "malformed" => vec![(URI_LIST, format!("{uris}not-a-file-url\n").into_bytes())],
            "bad-marker" => vec![
                (URI_LIST, uris.as_bytes().to_vec()),
                (KDE_CUT, b"invalid".to_vec()),
            ],
            _ => vec![(URI_LIST, uris.as_bytes().to_vec())],
        };
        let targets: Vec<_> = formats
            .iter()
            .enumerate()
            .map(|(i, (name, _))| gtk::TargetEntry::new(name, gtk::TargetFlags::empty(), i as u32))
            .collect();
        assert!(
            gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD).set_with_data(
                &targets,
                move |_, data, i| {
                    let (format, bytes) = &formats[i as usize];
                    data.set(&gtk::gdk::Atom::intern(format), 8, bytes);
                }
            )
        );
    }

    #[test]
    #[ignore = "requires an isolated X display and CIDE_NATIVE_CLIPBOARD_TEST=1; run native clipboard tests serially"]
    fn os_clipboard_between_processes() {
        use cide_ipc::PasteMode::{Copy, Cut};
        assert_eq!(
            std::env::var("CIDE_NATIVE_CLIPBOARD_TEST").as_deref(),
            Ok("1")
        );
        assert_eq!(std::env::var("GDK_BACKEND").as_deref(), Ok("x11"));
        let Ok(root) = std::env::var("CIDE_CLIPBOARD_TEST_ROOT") else {
            let root =
                std::env::temp_dir().join(format!("cide-os-clipboard-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap();
            let test = "file_clipboard::tests::os_clipboard_between_processes";
            let mut owner = spawn(&root, "owner", test);
            pump_until(|| root.join("ready").exists() || owner.0.try_wait().unwrap().is_some());
            assert!(root.join("ready").exists(), "OS clipboard owner failed");
            let mut receiver = spawn(&root, "receiver", test);
            pump_until(|| receiver.0.try_wait().unwrap().is_some());
            assert!(
                receiver.0.wait().unwrap().success(),
                "OS clipboard receiver failed"
            );
            pump_until(|| owner.0.try_wait().unwrap().is_some());
            assert!(
                owner.0.wait().unwrap().success(),
                "OS clipboard owner failed"
            );
            std::fs::remove_dir_all(root).unwrap();
            return;
        };
        gtk::init().unwrap();
        let root = std::path::PathBuf::from(root);
        let cases = [
            ("uri-copy", Some(Copy)),
            ("uri-copy-again", Some(Copy)),
            ("kde-cut", Some(Cut)),
            ("gnome-copy", Some(Copy)),
            ("gnome-cut", Some(Cut)),
            ("text-only", None),
            ("remote", None),
            ("malformed", None),
            ("bad-marker", None),
        ];
        let source = root.join("os-source");
        let uris = format!(
            "file://{}/source%20a.txt\r\nfile://{}/folder\r\n",
            source.display(),
            source.display()
        );
        match std::env::var("CIDE_CLIPBOARD_TEST_ROLE").unwrap().as_str() {
            "owner" => {
                for (i, (name, _)) in cases.iter().enumerate() {
                    if i > 0 {
                        pump_until(|| root.join(format!("want-{name}")).exists());
                    }
                    std::fs::create_dir_all(source.join("folder")).unwrap();
                    std::fs::write(source.join("source a.txt"), "OS file contents").unwrap();
                    std::fs::write(source.join("folder/child.txt"), "OS folder contents").unwrap();
                    send_os(name, &uris);
                    std::fs::write(root.join(format!("ready-{name}")), []).unwrap();
                    if i == 0 {
                        std::fs::write(root.join("ready"), []).unwrap();
                    }
                    pump_until(|| root.join(format!("handled-{name}")).exists());
                }
            }
            "receiver" => {
                let mut previous_id = String::new();
                for (i, (name, mode)) in cases.iter().enumerate() {
                    if i > 0 {
                        std::fs::write(root.join(format!("want-{name}")), []).unwrap();
                    }
                    pump_until(|| root.join(format!("ready-{name}")).exists());
                    let read = receive(platform::read);
                    if let Some(mode) = mode {
                        let value = read.unwrap().unwrap();
                        assert_eq!(value.clip.project, None);
                        assert_eq!(value.clip.mode, *mode);
                        assert_eq!(
                            value.clip.paths,
                            vec![source.join("source a.txt"), source.join("folder")]
                        );
                        assert_eq!(
                            native(),
                            Some(value.clone()),
                            "repeated reads changed the OS selection ID"
                        );
                        assert_ne!(
                            value.clip.id, previous_id,
                            "recopying the same files must invalidate an old dialog"
                        );
                        previous_id = value.clip.id.clone();
                        let dest = root.join(format!("pasted-{name}"));
                        std::fs::create_dir(&dest).unwrap();
                        let roots = vec![dest.clone()];
                        assert!(
                            cide_fs::copy::plan_between(
                                &value.roots,
                                &roots,
                                &value.clip.paths,
                                &dest,
                                *mode
                            )
                            .unwrap()
                            .is_empty()
                        );
                        assert!(
                            value.clip.paths.iter().all(|p| p.exists()),
                            "reading/planning moved the source"
                        );
                        cide_fs::copy::paste_between(
                            &value.roots,
                            &roots,
                            &value.clip.paths,
                            &dest,
                            *mode,
                            &[],
                        )
                        .unwrap();
                        assert_eq!(
                            std::fs::read_to_string(dest.join("source a.txt")).unwrap(),
                            "OS file contents"
                        );
                        assert_eq!(
                            std::fs::read_to_string(dest.join("folder/child.txt")).unwrap(),
                            "OS folder contents"
                        );
                        assert_eq!(value.clip.paths.iter().all(|p| p.exists()), *mode == Copy);
                        if *mode == Cut {
                            assert!(receive(|done| clear_matching(value.clip.id, done)).unwrap());
                            assert!(native().is_none());
                        }
                    } else if *name == "text-only" {
                        assert!(
                            read.unwrap().is_none(),
                            "plain text must not become a file selection"
                        );
                    } else {
                        assert!(read.is_err(), "invalid OS selection was accepted: {name}");
                        assert!(source.join("source a.txt").exists());
                    }
                    std::fs::write(root.join(format!("handled-{name}")), []).unwrap();
                }
            }
            role => panic!("unknown OS clipboard test role: {role}"),
        }
    }
}
