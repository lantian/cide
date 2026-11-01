use super::*;
use std::{
    cell::Cell,
    path::Path,
    process::{Child, Command},
    time::{Duration, Instant},
};

fn pump(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready() {
        assert!(Instant::now() < deadline, "native drag timed out");
        while glib::MainContext::default().iteration(false) {}
        std::thread::sleep(Duration::from_millis(5));
    }
}
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn mouse(args: &[&str]) {
    assert!(
        Command::new("xdotool")
            .args(args)
            .status()
            .unwrap()
            .success()
    );
}
fn paths(root: &Path, round: usize) -> Vec<PathBuf> {
    let mut paths = vec![root.join("plain.txt"), root.join("你好 a%#\n.txt")];
    if round == 2 {
        paths.reverse();
    }
    paths
}

/// A receiver in another process requests the selection only AFTER drop-performed.
/// Run under Xvfb with GDK_BACKEND=x11; never drive the user's desktop.
#[test]
#[ignore = "requires isolated Xvfb, xdotool and CIDE_NATIVE_DRAG_TEST=1"]
fn file_drag_native_delayed_receiver() {
    assert_eq!(std::env::var("CIDE_NATIVE_DRAG_TEST").as_deref(), Ok("1"));
    assert_eq!(std::env::var("GDK_BACKEND").as_deref(), Ok("x11"));
    let test = "file_drag::gtk_drag::native_tests::file_drag_native_delayed_receiver";
    let Ok(root) = std::env::var("CIDE_DRAG_TEST_ROOT") else {
        let root = cide_fs::testing::scratch("native-drag");
        for path in paths(root.path(), 1) {
            std::fs::write(path, b"nonempty attachment\n").unwrap();
        }
        let spawn = |role| {
            Process(
                Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", test, "--ignored", "--nocapture"])
                    .env("CIDE_DRAG_TEST_ROOT", root.path())
                    .env("CIDE_DRAG_TEST_ROLE", role)
                    .spawn()
                    .unwrap(),
            )
        };
        let mut source = spawn("source");
        let mut receiver = spawn("receiver");
        pump(|| {
            root.path().join("source-ready").exists() && root.path().join("receiver-ready").exists()
        });
        for round in 1..=2 {
            mouse(&[
                "mousemove",
                "100",
                "100",
                "mousedown",
                "1",
                "sleep",
                "0.2",
                "mousemove",
                "500",
                "100",
                "sleep",
                "0.2",
                "mouseup",
                "1",
            ]);
            pump(|| root.path().join(format!("end-{round}")).exists());
            assert!(root.path().join(format!("read-{round}")).exists());
        }
        mouse(&[
            "mousemove",
            "100",
            "100",
            "mousedown",
            "1",
            "sleep",
            "0.2",
            "key",
            "Escape",
            "mouseup",
            "1",
        ]);
        pump(|| root.path().join("end-3").exists());
        assert!(!root.path().join("read-3").exists());
        // GTK's default cancellation animation retains the grab briefly after drag-end.
        mouse(&["sleep", "0.7"]);
        mouse(&[
            "mousemove",
            "100",
            "100",
            "mousedown",
            "1",
            "sleep",
            "0.4",
            "mouseup",
            "1",
        ]);
        pump(|| root.path().join("end-4").exists());
        assert!(!root.path().join("read-4").exists());
        std::fs::write(root.path().join("quit"), []).unwrap();
        pump(|| source.0.try_wait().unwrap().is_some() && receiver.0.try_wait().unwrap().is_some());
        assert!(source.0.wait().unwrap().success());
        assert!(receiver.0.wait().unwrap().success());
        return;
    };
    gtk::init().unwrap();
    let root = PathBuf::from(root);
    let role = std::env::var("CIDE_DRAG_TEST_ROLE").unwrap();
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_default_size(200, 200);
    window.move_(if role == "source" { 0 } else { 400 }, 0);
    window.add_events(gdk::EventMask::BUTTON_PRESS_MASK);
    let count = Rc::new(Cell::new(0));
    if role == "source" {
        assert!(
            start(&window, &[], |_| panic!(
                "invalid startup called completion"
            ))
            .is_err()
        );
        assert!(
            start(&window, &["relative.txt".into()], |_| panic!(
                "invalid startup called completion"
            ))
            .is_err()
        );
        let root = root.clone();
        window.connect_button_press_event(move |window, _| {
            let round = count.get() + 1;
            eprintln!("source start {round}");
            count.set(round);
            let done_root = root.clone();
            start(window, &paths(&root, round), move |result| {
                eprintln!("source complete {round}: {result:?}");
                if round == 4 {
                    assert!(result.is_err());
                } else {
                    result.unwrap();
                }
                let file = done_root.join(format!("end-{round}"));
                assert!(!file.exists(), "completion must run once");
                std::fs::write(file, []).unwrap();
            })
            .unwrap();
            if round == 4 {
                let window = window.clone();
                glib::timeout_add_local_once(Duration::from_millis(100), move || {
                    eprintln!("closing source");
                    window.close();
                });
            }
            glib::Propagation::Stop
        });
    } else {
        window.drag_dest_set(
            gtk::DestDefaults::MOTION,
            &[gtk::TargetEntry::new(
                "text/uri-list",
                gtk::TargetFlags::empty(),
                0,
            )],
            gdk::DragAction::COPY,
        );
        window.connect_drag_drop(move |window, context, _, _, time| {
            let window = window.clone();
            let context = context.clone();
            glib::timeout_add_local_once(Duration::from_millis(150), move || {
                window.drag_get_data(&context, &gdk::Atom::intern("text/uri-list"), time);
            });
            true
        });
        let root = root.clone();
        window.connect_drag_data_received(move |_, context, _, _, data, _, time| {
            let round = count.get() + 1;
            count.set(round);
            assert!(
                !root.join(format!("end-{round}")).exists(),
                "source completed before receiver read"
            );
            let received: Vec<_> = data
                .uris()
                .iter()
                .map(|uri| glib::filename_from_uri(uri).unwrap().0)
                .collect();
            assert_eq!(received, paths(&root, round));
            for path in received {
                assert_eq!(std::fs::read(path).unwrap(), b"nonempty attachment\n");
            }
            std::fs::write(root.join(format!("read-{round}")), []).unwrap();
            context.drag_finish(true, false, time);
        });
    }
    window.show_all();
    pump(|| window.is_mapped());
    std::fs::write(root.join(format!("{role}-ready")), []).unwrap();
    pump(|| root.join("quit").exists());
}
