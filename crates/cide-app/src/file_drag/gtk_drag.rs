//! GTK owns the transfer until drag-end, not merely until the mouse is released.
//! In particular, a destination may request text/uri-list AFTER drop-performed.
use gtk::{gdk, glib, prelude::*};
use std::{cell::RefCell, path::PathBuf, rc::Rc};

type Done = Box<dyn FnOnce(Result<(), String>)>;
#[derive(Default)]
struct Session {
    handlers: Vec<glib::SignalHandlerId>,
    done: Option<Done>,
}

fn finish(widget: &gtk::Widget, session: &RefCell<Session>, result: Result<(), String>) {
    let Session { handlers, done } = std::mem::take(&mut *session.borrow_mut());
    for id in handlers {
        widget.disconnect(id);
    }
    if let Some(done) = done {
        done(result);
    }
}

fn uris(paths: &[PathBuf]) -> Result<Vec<glib::GString>, String> {
    if paths.is_empty() {
        return Err("There are no files to drag.".into());
    }
    paths
        .iter()
        .map(|path| glib::filename_to_uri(path, None).map_err(|e| e.to_string()))
        .collect()
}

pub(super) fn start(
    window: &impl IsA<gtk::Widget>,
    paths: &[PathBuf],
    done: impl FnOnce(Result<(), String>) + 'static,
) -> Result<(), String> {
    let uris = uris(paths)?;
    let widget = window.as_ref();
    let targets = gtk::TargetList::new(&[]);
    targets.add_uri_targets(0);
    let session = Rc::new(RefCell::new(Session {
        handlers: vec![],
        done: Some(Box::new(done)),
    }));
    let data = widget.connect_drag_data_get(move |_, _, data, _, _| {
        data.set_uris(&uris.iter().map(|s| s.as_str()).collect::<Vec<_>>());
    });
    session.borrow_mut().handlers.push(data);
    let end = session.clone();
    let id = widget.connect_drag_end(move |widget, _| finish(widget, &end, Ok(())));
    session.borrow_mut().handlers.push(id);
    let destroyed = session.clone();
    let id = widget.connect_destroy(move |widget| {
        finish(
            widget,
            &destroyed,
            Err("The drag source window was closed.".into()),
        );
    });
    session.borrow_mut().handlers.push(id);

    // This argument is a button number, NOT GDK_BUTTON1_MASK (256).
    // An explicit target list avoids installing an automatic drag source on the window.
    let Some(context) =
        widget.drag_begin_with_coordinates(&targets, gdk::DragAction::COPY, 1, None, -1, -1)
    else {
        let message = "Could not start the file drag.".to_string();
        finish(widget, &session, Err(message.clone()));
        return Err(message);
    };
    let loader = gtk::gdk_pixbuf::PixbufLoader::new();
    if loader
        .write(include_bytes!("../../icons/32x32.png"))
        .is_ok()
        && loader.close().is_ok()
        && let Some(icon) = loader.pixbuf()
    {
        context.drag_set_icon_pixbuf(&icon, 0, 0);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_drag_uris_round_trip() {
        let paths: Vec<PathBuf> = ["/tmp/plain.txt", "/tmp/你好 a%#\n.txt"]
            .into_iter()
            .map(Into::into)
            .collect();
        let encoded = uris(&paths).unwrap();
        assert_eq!(encoded[0], "file:///tmp/plain.txt");
        assert!(encoded[1].contains("%20"));
        assert!(encoded[1].contains("%25%23%0A"));
        for (uri, path) in encoded.iter().zip(paths) {
            assert_eq!(glib::filename_from_uri(uri).unwrap().0, path);
        }
        assert!(uris(&[]).is_err());
        assert!(uris(&["relative.txt".into()]).is_err());
    }
}

#[cfg(test)]
#[path = "gtk_drag_tests.rs"]
mod native_tests;
