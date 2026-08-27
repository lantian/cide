//! The macOS dock menu: every open project, one click from the front.
//!
//! Right-clicking (or long-pressing) an app's dock icon opens a menu, and macOS fills the
//! bottom of it for us — *Options*, *Show All Windows*, *Quit*. The top is the application's
//! to contribute, through the one hook AppKit offers: `applicationDockMenu:` on the
//! `NSApplicationDelegate`. Every project cide has open goes there, and picking one brings it
//! to the front.
//!
//! # Why this is the dock's job and not the header's
//!
//! It is **not** the `PerProject` window mode wearing a different hat, and the two must not be
//! wired together. In `Stacked` mode there is one OS window and macOS's own *Show All Windows*
//! lists exactly one thing — the projects inside it are invisible to the desktop, because
//! nothing at the AppKit level knows they exist. In `PerProject` mode there is a window each,
//! so the system list is finally useful, and by then the user does not need this menu as badly.
//! The mode that most needs the dock to name projects is the one where the desktop cannot see
//! them at all, which is why [`entries`] reads `ws.projects` and never looks at
//! `ws.settings.window_mode` — the menu says the same thing in both modes, and the mode only
//! decides *which window* a pick raises. That is [`crate::cmd::window::focus_project`]'s
//! problem, and it is answered from the window map rather than from the mode.
//!
//! # The split, and why the interesting half is on this side of a `cfg`
//!
//! [`entries`] is the model — which rows, in which order, saying what — and it is ordinary Rust
//! over a [`Workspace`], unit-tested below on the platform this is developed on. `macos` below
//! is the AppKit glue, and it is deliberately as thin as it can be made: build an `NSMenu` from
//! whatever [`entries`] returns, and turn a click back into a `ProjectId`. Nothing in it decides
//! anything.
//!
//! That split is the same one `cmd::window`'s `title_with` makes for the same reason, and here it
//! is not a preference but the only option: **`cide-app` cannot be type-checked for Darwin from
//! Linux** — `docs/platforms.md`'s *Type-checking for macOS from Linux* excludes this crate, because
//! `git2` is `vendored-openssl` and building OpenSSL for Darwin needs a real cross toolchain.
//!
//! # What has actually been verified, and what has not
//!
//! Stated plainly, because `docs/platforms.md` exists to keep this distinction and
//! the estimate it corrects was wrong in the direction estimates are always wrong.
//!
//! * [`entries`] and its tests **run**, here, in `cargo test --workspace`.
//! * The `macos` module below **type-checks and passes `clippy -D warnings` for
//!   `aarch64-apple-darwin`**, against the real `objc2 0.6` and `objc2-app-kit 0.3`. Not through
//!   this crate — through a throwaway crate carrying a verbatim copy of the module with the four
//!   cide-side types (`AppHandle`, `Manager`, `WorkspaceState`, `ProjectId`) stubbed, the same
//!   `DOCS_RS=1` + `scripts/darwin-cc.sh` recipe `docs/platforms.md` gives, and those three dependencies
//!   at the versions the workspace pins. Two real errors came out of it — a missing
//!   `MainThreadOnly` import and both `transmute` annotations — which is the argument for doing
//!   it rather than a note about diligence.
//! * It has **never been linked and never been run.** Nothing above says the delegate's class is
//!   what tao's source says it is at runtime, that `class_addMethod` returns true against it, or
//!   that macOS asks a `TaoAppDelegateParent` for a dock menu at all. The `macos (advisory)` CI
//!   job is the first thing that will have an opinion, and a Mac is the second.

use cide_ipc::{ProjectId, Workspace};

/// One row of the dock menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockEntry {
    /// What a click on this row brings to the front.
    pub project: ProjectId,
    /// The row's text.
    pub title: String,
    /// The project's primary root, as the header shows it. The row's tooltip.
    ///
    /// Separate from [`Self::title`] even when the title already ends with it, because a
    /// tooltip is the one place a long path costs nothing: the row stays the width of a name.
    pub path: String,
}

/// Every open project, in header order.
///
/// **Header order, not window order.** `ws.projects` is an `IndexMap` whose insertion order is
/// what the project strip draws and what `reorder_project` rewrites, so a user who has arranged
/// their projects finds the same arrangement here. Walking `ws.windows` instead would put the
/// menu in window-map order, which is the same list until the first mode flip and then quietly
/// is not.
///
/// **Names are disambiguated by path, and only when they collide.** A project is named for its
/// directory's basename, so two checkouts of one repository — `~/work/cide` and
/// `~/work/cide-review`, or the same project opened from a worktree — can both be called `cide`,
/// and two identical rows in a menu is a coin toss rather than a choice. Appending the path to
/// *every* row instead was tried on paper and rejected: it makes the common case (three projects
/// with three different names) into three rows of mostly-identical prefix, which is the failure
/// the disambiguation is meant to prevent, applied everywhere.
pub fn entries(ws: &Workspace) -> Vec<DockEntry> {
    // One pass to count, one to build. A project with no duplicate keeps its bare name; the
    // count is over the *name* alone, because that is the string the user would be choosing
    // between.
    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for project in ws.projects.values() {
        *seen.entry(project.name.as_str()).or_default() += 1;
    }

    ws.projects
        .values()
        .map(|project| DockEntry {
            project: project.id,
            title: if seen.get(project.name.as_str()).copied().unwrap_or(0) > 1 {
                format!("{} — {}", project.name, project.display_path)
            } else {
                project.name.clone()
            },
            path: project.display_path.clone(),
        })
        .collect()
}

/// Give this platform's dock, if it has one, a menu of the open projects.
///
/// Called once, from `setup`, and after that the menu keeps itself current: AppKit asks the
/// delegate for a fresh `NSMenu` every time the user opens it, so there is nothing to invalidate
/// when a project opens or closes. That is the whole reason this hook is a *callback* and not a
/// menu handed over once — a cached menu would have to be rebuilt from every mutation site, and
/// the one that got forgotten would leave a row that opens a project that is no longer there.
///
/// A no-op everywhere else. Linux has no equivalent hook that is worth the name: a task bar's
/// context menu belongs to the desktop environment, not to the application, and KDE's is built
/// from the `.desktop` entry's `Actions=` — a static list, which is exactly what this is not.
pub fn install(app: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    macos::install(app);
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

#[cfg(target_os = "macos")]
mod macos {
    //! `applicationDockMenu:`, added to whatever class tao's app delegate turned out to be.
    //!
    //! # Why the method is grafted on at runtime
    //!
    //! There is one dock-menu hook in AppKit and it is a method on the *application delegate*.
    //! tao owns that object — it builds a `TaoAppDelegateParent` in `EventLoop::new` and calls
    //! `setDelegate:` before tauri's `setup` ever runs — and neither tao, wry nor tauri exposes
    //! the dock menu. So the choices are: replace the delegate (which would silently drop
    //! `applicationDidFinishLaunching:`, `applicationWillTerminate:` and `application:openURLs:`,
    //! and `applicationWillTerminate:` is the *only* path a ⌘Q takes to `shutdown` — see the
    //! `RunEvent::Exit` arm in `lib.rs`), proxy it (a forwarding object that has to keep up with
    //! whatever tao adds next), or add the one method the delegate does not implement to the
    //! class it already is. The third costs one `class_addMethod` and changes nothing about the
    //! other methods.
    //!
    //! **It refuses to override.** `applicationDockMenu:` is an *optional* protocol method that
    //! tao does not implement, so today this adds a method to a class that has none — but
    //! `class_addMethod` would happily shadow an inherited implementation, and shadowing a future
    //! tao (or tauri) dock menu is precisely the silent breakage that makes runtime patching a
    //! bad idea. So the delegate is asked first, and a delegate that already answers is left
    //! alone with a line in the log rather than quietly replaced.
    //!
    //! # Threading, and what is safe about it
    //!
    //! Both grafted methods run on the main thread by construction: AppKit builds a dock menu
    //! and sends a menu action from the event loop and nowhere else. That is what makes it legal
    //! for the action to take the workspace lock, create windows and call `set_title` — the same
    //! thread every other window-mutating path in this crate already runs on. The
    //! `MainThreadMarker` each AppKit call wants is *asked for* rather than asserted, so a wrong
    //! assumption becomes a logged refusal instead of undefined behaviour.
    //!
    //! Both are declared `extern "C"` rather than `extern "C-unwind"`, which is the deliberate
    //! choice: an `extern "C"` function aborts if its body panics instead of unwinding into an
    //! Objective-C frame, which is not a stack Rust may unwind through. A release build is
    //! `panic = "abort"` anyway; this makes a debug build behave the same rather than corrupt.

    use std::sync::OnceLock;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, Imp, NSObjectProtocol, Sel};
    use objc2::{MainThreadMarker, MainThreadOnly, sel};
    use objc2_app_kit::{NSApplication, NSMenu, NSMenuItem};
    use objc2_foundation::{NSString, ns_string};
    use tauri::{AppHandle, Manager};

    use crate::workspace_state::WorkspaceState;

    /// The handle the menu is built from and the click is answered with.
    ///
    /// A `OnceLock` rather than managed Tauri state for the reason `windows::awaiting_sessions`
    /// gives about its own: an Objective-C callback is handed a `self` that is tao's delegate
    /// and nothing else, so there is no argument to thread a handle through and the value has
    /// to be reachable from a `static`. Set in [`install`], read from both callbacks, never
    /// written again.
    static APP: OnceLock<AppHandle> = OnceLock::new();

    /// Objective-C type encodings for the two methods below.
    ///
    /// `@` is an object, `:` a selector, `v` void — so `@@:@` is "returns an object; takes self,
    /// _cmd and one object", which is `- (NSMenu *)applicationDockMenu:(NSApplication *)sender`,
    /// and `v@:@` is the same with no return, which is an ordinary menu action. Written out
    /// rather than derived because `ClassBuilder`'s encoding machinery only works while a class
    /// is being *created*, and this one already exists.
    const DOCK_MENU_TYPES: &std::ffi::CStr = c"@@:@";
    const ACTION_TYPES: &std::ffi::CStr = c"v@:@";

    /// The Rust side of each encoding above, named so the cast to `Imp` can spell both types.
    ///
    /// `Imp` is deliberately signature-less — `unsafe extern "C-unwind" fn()` — because the
    /// runtime dispatches by the encoding string rather than by a Rust type, so getting there
    /// needs a `transmute` (an `as` cast cannot change a function pointer's signature). Both
    /// sides of that `transmute` are written out rather than inferred, which is what
    /// `clippy::missing_transmute_annotations` is asking for and is right to ask for: an inferred
    /// source type would follow a changed signature silently, and the encoding beside it would
    /// not — and a mismatch between the two is read as arguments that were never passed.
    type DockMenuImp = extern "C" fn(&AnyObject, Sel, *mut AnyObject) -> *mut NSMenu;
    type ActionImp = extern "C" fn(&AnyObject, Sel, *mut AnyObject);

    pub fn install(app: &AppHandle) {
        // Before the first callback can fire, and unconditionally: a menu built for a handle
        // that was never stored would draw the right rows and do nothing on a click.
        let _ = APP.set(app.clone());

        let Some(mtm) = MainThreadMarker::new() else {
            tracing::error!("the dock menu must be installed from the main thread; skipping");
            return;
        };
        let Some(delegate) = NSApplication::sharedApplication(mtm).delegate() else {
            // tao sets one in `EventLoop::new`, which tauri runs before `setup`, so this is
            // "the runtime changed under us" rather than a state to handle.
            tracing::error!("no NSApplicationDelegate to hang a dock menu on; skipping");
            return;
        };

        // See the module note: refuse to shadow, rather than override.
        if delegate.respondsToSelector(sel!(applicationDockMenu:)) {
            tracing::warn!(
                "the app delegate already answers applicationDockMenu:; leaving it alone"
            );
            return;
        }

        let class: &AnyClass = AsRef::<AnyObject>::as_ref(&*delegate).class();
        // **The action goes on first, and the order is the whole error handling.** The Objective-C
        // runtime has no `class_removeMethod` — a method added is permanent for the life of the
        // process — so a half-installed pair cannot be rolled back and the ordering has to make
        // the surviving half harmless instead.
        //
        // This way round, a failure to add the action leaves `applicationDockMenu:` absent and
        // macOS simply draws its own dock menu, which is the state before this feature. The other
        // way round would leave a dock menu whose every row targets a selector the delegate does
        // not implement, and AppKit *disables* such an item rather than dropping it: a menu of
        // greyed-out project names, which reads as cide having lost them.
        //
        // SAFETY: both implementations match the encodings beside them, take the receiver by
        // reference (the delegate outlives the application), and are `extern "C"`, so neither
        // can unwind into the Objective-C frame that calls it. The selectors are added to a
        // class that does not already implement them, which is what the guard above establishes.
        let added = unsafe {
            add_method(
                class,
                sel!(cideDockOpenProject:),
                ACTION_TYPES,
                std::mem::transmute::<ActionImp, Imp>(open_project_action),
            ) && add_method(
                class,
                sel!(applicationDockMenu:),
                DOCK_MENU_TYPES,
                std::mem::transmute::<DockMenuImp, Imp>(application_dock_menu),
            )
        };
        if !added {
            tracing::error!("could not add the dock menu methods to the app delegate");
            return;
        }
        tracing::info!("dock menu installed");
    }

    /// `class_addMethod`, with the cast the FFI signature wants.
    ///
    /// # Safety
    ///
    /// `imp` must be a function with the ABI `types` describes, whose first two parameters are
    /// the receiver and the selector, and which does not unwind.
    unsafe fn add_method(
        class: &AnyClass,
        selector: Sel,
        types: &std::ffi::CStr,
        imp: Imp,
    ) -> bool {
        unsafe {
            objc2::ffi::class_addMethod(
                class as *const AnyClass as *mut AnyClass,
                selector,
                imp,
                types.as_ptr(),
            )
            .as_bool()
        }
    }

    /// `- (NSMenu *)applicationDockMenu:(NSApplication *)sender`
    ///
    /// Built fresh on every open, from the workspace as it stands at that instant — see
    /// [`super::install`] for why that is the point rather than an inefficiency.
    ///
    /// The returned menu is **autoreleased**: `applicationDockMenu:` is not in the `new`/`alloc`
    /// /`copy` families, so its caller does not own the result and returning a `+1` object here
    /// would leak an `NSMenu` per right-click. `Retained::autorelease_return` is the whole of
    /// that rule.
    extern "C" fn application_dock_menu(
        delegate: &AnyObject,
        _cmd: Sel,
        _sender: *mut AnyObject,
    ) -> *mut NSMenu {
        let Some(mtm) = MainThreadMarker::new() else {
            // AppKit only asks from the main thread, so this is unreachable — and `null` is a
            // documented answer meaning "no custom items", which is the right way to be wrong.
            tracing::error!("applicationDockMenu: off the main thread");
            return std::ptr::null_mut();
        };
        let menu = NSMenu::new(mtm);

        let Some(app) = APP.get() else {
            return Retained::autorelease_return(menu);
        };
        let Some(state) = app.try_state::<WorkspaceState>() else {
            return Retained::autorelease_return(menu);
        };

        // `with` rather than `snapshot`: the whole tree is cloned by a snapshot and all this
        // wants is a name and a path per project. The closure does not mutate, block or
        // re-enter, which is that method's contract.
        let entries = state.with(super::entries);
        for entry in entries {
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(&entry.title),
                    Some(sel!(cideDockOpenProject:)),
                    ns_string!(""),
                )
            };
            // The *delegate* is the target, because the delegate is where the action was added
            // and it is the one object in the process guaranteed to outlive this menu. Leaving
            // the target nil would send the action up the responder chain instead, where nothing
            // answers it and AppKit therefore draws the row disabled.
            unsafe { item.setTarget(Some(delegate)) };
            // The id travels on the item rather than in a side table keyed by index. An index
            // would be a second piece of state to keep in step with a list that can change
            // between the menu opening and the click landing — a project closed from another
            // window in that moment would shift every row below it, and the click would open
            // the wrong project rather than none.
            unsafe {
                item.setRepresentedObject(Some(&NSString::from_str(&entry.project.to_string())))
            };
            item.setToolTip(Some(&NSString::from_str(&entry.path)));
            menu.addItem(&item);
        }

        Retained::autorelease_return(menu)
    }

    /// `- (void)cideDockOpenProject:(id)sender`
    ///
    /// Everything past the id lookup is the ordinary, platform-independent gesture — see
    /// [`crate::cmd::window::focus_project`], which is what the two window modes differ inside.
    extern "C" fn open_project_action(_delegate: &AnyObject, _cmd: Sel, sender: *mut AnyObject) {
        // Every step down to the id is *checked* — `downcast_ref` asks the runtime rather than
        // reinterpreting the pointer. AppKit will only ever send this the `NSMenuItem` built
        // above, so none of these can fail today; a reinterpret that is wrong tomorrow reads
        // whatever memory follows the object, and this is a callback the whole application
        // shares a process with.
        let Some(sender) = (unsafe { sender.as_ref() }) else {
            return;
        };
        let Some(item) = sender.downcast_ref::<NSMenuItem>() else {
            tracing::warn!("cideDockOpenProject: was sent something that is not a menu item");
            return;
        };
        let Some(represented) = item.representedObject() else {
            tracing::warn!("a dock menu item arrived with no project on it");
            return;
        };
        let Some(text) = represented.downcast_ref::<NSString>() else {
            tracing::warn!("a dock menu item carried something that is not a project id");
            return;
        };
        let Ok(project) = text.to_string().parse::<cide_ipc::ProjectId>() else {
            tracing::warn!("a dock menu item named something that is not a project id");
            return;
        };

        let Some(app) = APP.get() else { return };
        let Some(state) = app.try_state::<WorkspaceState>() else {
            return;
        };
        // Logged and dropped. The project closed between the menu opening and the click, which
        // is a race the user cannot act on and the menu cannot prevent — the next right-click
        // shows the list as it now stands.
        if let Err(error) = crate::cmd::window::focus_project(app, &state, project) {
            tracing::warn!(%project, %error, "the dock could not bring a project to the front");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_core::workspace;
    use std::path::PathBuf;

    fn open(ws: &mut Workspace, path: &str) -> ProjectId {
        workspace::open_project(ws, vec![PathBuf::from(path)], None).expect("a root is enough")
    }

    #[test]
    fn lists_every_open_project_in_header_order() {
        let mut ws = Workspace::default();
        let a = open(&mut ws, "/tmp/alpha");
        let b = open(&mut ws, "/tmp/beta");

        let rows = entries(&ws);
        assert_eq!(
            rows.iter().map(|r| r.project).collect::<Vec<_>>(),
            vec![a, b],
            "the menu is the project strip's order, which is the order a user arranged"
        );
        assert_eq!(
            rows.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
            vec!["alpha", "beta"],
            "and a name that is unique among the open projects is the whole row"
        );
    }

    #[test]
    fn says_the_same_thing_in_both_window_modes() {
        let mut ws = Workspace::default();
        open(&mut ws, "/tmp/alpha");
        open(&mut ws, "/tmp/beta");

        let stacked = entries(&ws);
        workspace::set_window_mode(&mut ws, cide_ipc::WindowMode::PerProject).expect("flips");
        assert_eq!(
            entries(&ws),
            stacked,
            "THE POINT OF THIS MENU: the desktop can only see projects as windows, so the mode \
             that hides them from it is the one that needs the dock to name them. The mode \
             decides which window a pick raises and nothing about what the menu says"
        );
    }

    #[test]
    fn disambiguates_two_projects_with_one_name() {
        let mut ws = Workspace::default();
        // Two checkouts of one repository — a worktree, a review copy — are named for their
        // basename and collide. Two identical rows would be a coin toss.
        open(&mut ws, "/tmp/one/cide");
        open(&mut ws, "/tmp/two/cide");
        open(&mut ws, "/tmp/one/notes");

        let rows = entries(&ws);
        assert_eq!(rows[0].title, format!("cide — {}", rows[0].path));
        assert_eq!(rows[1].title, format!("cide — {}", rows[1].path));
        assert_ne!(
            rows[0].path, rows[1].path,
            "and the paths are what tell them apart"
        );
        assert_eq!(
            rows[2].title, "notes",
            "the row that had no collision keeps its bare name — appending a path to every row \
             is the failure this prevents, applied everywhere"
        );
    }

    #[test]
    fn an_empty_workspace_contributes_nothing() {
        // Not a state to guard against: it is first launch, and macOS still draws its own
        // Options / Show All Windows / Quit below whatever we add.
        assert!(entries(&Workspace::default()).is_empty());
    }
}
