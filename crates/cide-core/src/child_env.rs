//! What a child of cide must *not* inherit from the way cide itself was launched.
//!
//! # The failure
//!
//! Started from the AppImage, every MCP server a pane's `claude` spawns over stdio dies before
//! it can speak one byte of protocol:
//!
//! ```text
//! Fatal Python error: Failed to import encodings module
//! ModuleNotFoundError: No module named 'encodings'
//! ```
//!
//! which the CLI reports as `CONNECTION_CLOSED` against a server whose configuration is
//! correct — the same `~/.claude.json` entry works in a terminal. Nothing in the message names
//! cide, and nothing in cide's logs mentions it: the death happens two processes down, in a
//! `uvx`-spawned interpreter that never gets to say why.
//!
//! The cause is one variable. AppImageKit's `AppRun` — the entry point of the bundle, four
//! processes above us — rewrites the environment so the *bundled* binary finds the *bundled*
//! libraries, and everything it sets is inherited by every descendant we ever spawn:
//!
//! ```text
//! PATH=$APPDIR/usr/bin/:…:$PATH          LD_LIBRARY_PATH=$APPDIR/usr/lib/:…:$LD_LIBRARY_PATH
//! PYTHONHOME=$APPDIR/usr/                PYTHONPATH=$APPDIR/usr/share/pyshared/:$PYTHONPATH
//! XDG_DATA_DIRS=$APPDIR/usr/share/:…     GSETTINGS_SCHEMA_DIR=$APPDIR/usr/share/glib-2.0/schemas/:…
//! PERLLIB=$APPDIR/usr/share/perl5/:…     QT_PLUGIN_PATH=$APPDIR/usr/lib/qt4/plugins/:…
//! GST_PLUGIN_SYSTEM_PATH[_1_0]=$APPDIR/usr/lib/gstreamer…
//! ```
//!
//! `PYTHONHOME` is the fatal one: it is absolute, it outranks everything, and it points a
//! Python 3.13 interpreter at a prefix that contains no stdlib at all — cide's bundle has no
//! Python in it. `LD_LIBRARY_PATH` is the same class of hazard for any child that links
//! something the bundle also carries (160 libraries, GTK 3 and WebKitGTK among them), and it
//! fails as a symbol lookup error rather than anything legible.
//!
//! None of this is AppImage being wrong. Those variables are exactly right *for the process
//! inside the bundle* and wrong for every process a terminal emulator launches on the user's
//! behalf, which is what cide is. A shell in a pane is the user's shell, not part of our
//! package.
//!
//! # The rule
//!
//! **A path that lives inside the bundle is removed from a child's environment; everything
//! else is left exactly as it was found.** `AppRun` *prepends*, so filtering the bundle's
//! entries out of a list-shaped variable restores the value the user's session actually had,
//! and a variable left with nothing is unset rather than left empty.
//!
//! This is a rule about values, not a list of variable names, deliberately. The list above is
//! what AppImageKit 13 and the `linuxdeploy` GTK hook write today; a plugin added tomorrow
//! (`QML2_IMPORT_PATH`, another loader cache) writes more of the same shape, and a name list
//! would silently stop covering it. Scanning values costs one pass over an environment we are
//! already about to copy into a child.
//!
//! Emptiness matters as much as the prefix. `PYTHONPATH=$APPDIR/usr/share/pyshared/:` — the
//! literal residue of the prepend when the user had no `PYTHONPATH` — filters down to one
//! *empty* entry, and an empty entry in a path list means the current directory. Handing a
//! child `PYTHONPATH=:` would replace a broken interpreter with an interpreter that imports
//! from whatever directory the pane happens to be sitting in, which is worse.
//!
//! # What this cannot do, and does not try
//!
//! * A variable the bundle **overwrote** rather than prepended to is gone before we run.
//!   `PYTHONHOME` is the only one, and a user who had their own is left with none — correct
//!   far more often than the alternative, since the value we would otherwise pass on names a
//!   prefix that ceases to exist the moment cide exits.
//! * `GDK_BACKEND=x11`, `GTK_THEME=Adwaita:dark` and `PYTHONDONTWRITEBYTECODE=1` are set by
//!   the same launcher and are **left alone**: their values name nothing inside the bundle, so
//!   there is no way to tell them from the same variables set by the user's own profile. What
//!   they cost a child is cosmetic (a GUI app launched from a pane runs on XWayland, in
//!   Adwaita) where the ones here cost it its life.
//! * Nothing is scrubbed when `APPDIR` is unset, which is every development run, every `.deb`
//!   install and every Flatpak. `./run.sh` produces an identical environment before and after
//!   this module existed.

use std::process::Command;

/// One change to make to a child's inherited environment: `Some(value)` sets it, `None`
/// removes it.
pub type EnvChange = (String, Option<String>);

/// Variables that describe the bundle itself rather than a path inside it.
///
/// `APPIMAGE` is the path of the `.AppImage` file, `ARGV0` the name it was invoked as, and
/// `OWD` the directory the user was in when they ran it — none of which is true of a child, and
/// all of which make a program that knows about AppImages (`appimageupdate`, a self-relaunch,
/// anything asking "am I bundled?") answer yes on cide's behalf. `APPDIR` is listed for
/// symmetry; the value rule below would remove it anyway, since its value *is* the bundle root.
const BUNDLE_MARKERS: [&str; 4] = ["APPDIR", "APPIMAGE", "ARGV0", "OWD"];

/// What must change in a child's environment, computed from this process's own.
///
/// Empty — and cheap — when cide was not launched from a bundle.
pub fn bundle_scrub() -> Vec<EnvChange> {
    let Ok(appdir) = std::env::var("APPDIR") else {
        return Vec::new();
    };
    bundle_scrub_from(std::env::vars(), &appdir)
}

/// The rule itself, over an environment handed in rather than read.
///
/// Pure, because the alternative is a test that mutates the process environment: `set_var` is
/// `unsafe` in edition 2024 precisely because it races every other thread reading it, and this
/// crate's tests run in the same process as everything else.
pub fn bundle_scrub_from(
    vars: impl IntoIterator<Item = (String, String)>,
    appdir: &str,
) -> Vec<EnvChange> {
    // Trailing slashes are stripped so the boundary check below has exactly one shape to
    // handle. An `APPDIR` that is relative or empty is not something we can reason about — no
    // launcher produces one, and treating a relative prefix as "inside the bundle" could match
    // an entry that has nothing to do with us.
    let root = appdir.trim_end_matches('/');
    if !root.starts_with('/') {
        return Vec::new();
    }

    let mut changes = Vec::new();
    for (name, value) in vars {
        if BUNDLE_MARKERS.contains(&name.as_str()) {
            changes.push((name, None));
            continue;
        }
        // Nothing from the bundle in here: leave the variable completely untouched rather than
        // rewriting it to an equal value. A child's environment should differ from its
        // parent's only where we can say why.
        if !value.split(':').any(|entry| under(entry, root)) {
            continue;
        }
        let kept: Vec<&str> = value
            .split(':')
            .filter(|entry| !entry.is_empty() && !under(entry, root))
            .collect();
        changes.push((name, (!kept.is_empty()).then(|| kept.join(":"))));
    }
    changes
}

/// Is this path-list entry inside the bundle?
///
/// The boundary is a whole path component, so `/tmp/.mount_cideAAA` does not swallow
/// `/tmp/.mount_cideAAAA` — the mount points AppImage generates are one random suffix apart,
/// and a user running two AppImages at once is ordinary.
fn under(entry: &str, root: &str) -> bool {
    entry
        .strip_prefix(root)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Apply [`bundle_scrub`] to a [`Command`] that is about to be spawned.
///
/// For the children spawned with `std::process` — the `claude` one-shots, `git push`,
/// `claude --version`. PTY children take the same changes through `SpawnSpec`, in
/// `cide_app::cmd::session::base_env`, because this crate cannot see `cide-pty`'s types.
pub fn scrub_command(command: &mut Command) {
    for (name, value) in bundle_scrub() {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mount point of a running AppImage, in the shape the runtime actually produces.
    const APPDIR: &str = "/tmp/.mount_cide_0OOoGFm";

    fn scrub(vars: &[(&str, &str)]) -> Vec<EnvChange> {
        bundle_scrub_from(
            vars.iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string())),
            APPDIR,
        )
    }

    fn change<'a>(changes: &'a [EnvChange], name: &str) -> Option<&'a Option<String>> {
        changes.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    #[test]
    fn the_fatal_one_is_removed() {
        // Verbatim from the session that could not start an MCP server. Absolute, entirely
        // inside the bundle, and nothing survives filtering it — so the variable goes.
        let changes = scrub(&[("PYTHONHOME", "/tmp/.mount_cide_0OOoGFm/usr/")]);
        assert_eq!(change(&changes, "PYTHONHOME"), Some(&None));
    }

    #[test]
    fn a_prepend_onto_nothing_leaves_no_empty_entry() {
        // `PYTHONPATH=$APPDIR/usr/share/pyshared/:$PYTHONPATH` with no inherited `PYTHONPATH`
        // is what produced this trailing colon. Passing on `PYTHONPATH=:` would tell every
        // Python child to import from the pane's current directory.
        let changes = scrub(&[(
            "PYTHONPATH",
            "/tmp/.mount_cide_0OOoGFm/usr/share/pyshared/:",
        )]);
        assert_eq!(change(&changes, "PYTHONPATH"), Some(&None));
    }

    #[test]
    fn a_prepend_onto_something_gives_back_exactly_what_the_user_had() {
        let changes = scrub(&[
            (
                "PATH",
                "/tmp/.mount_cide_0OOoGFm/usr/bin/:/tmp/.mount_cide_0OOoGFm/usr/sbin/:/home/u/bin:/usr/bin",
            ),
            (
                "XDG_DATA_DIRS",
                "/tmp/.mount_cide_0OOoGFm/usr/share:/usr/share:/usr/local/share",
            ),
        ]);
        assert_eq!(
            change(&changes, "PATH"),
            Some(&Some("/home/u/bin:/usr/bin".to_string()))
        );
        assert_eq!(
            change(&changes, "XDG_DATA_DIRS"),
            Some(&Some("/usr/share:/usr/local/share".to_string()))
        );
    }

    #[test]
    fn the_double_slash_the_gtk_hook_writes_is_still_inside_the_bundle() {
        // `linuxdeploy-plugin-gtk.sh` interpolates `$APPDIR//usr/...`. A prefix check that
        // demanded a single separator would keep every one of these.
        let changes = scrub(&[(
            "GDK_PIXBUF_MODULE_FILE",
            "/tmp/.mount_cide_0OOoGFm//usr/lib64/gdk-pixbuf-2.0/2.10.0/loaders.cache",
        )]);
        assert_eq!(change(&changes, "GDK_PIXBUF_MODULE_FILE"), Some(&None));
    }

    #[test]
    fn a_neighbouring_mount_is_not_ours_to_touch() {
        // Two AppImages running at once differ by a random suffix, and one must not scrub the
        // other's paths out of a shell that legitimately has them.
        let changes = scrub(&[("PATH", "/tmp/.mount_cide_0OOoGFmX/usr/bin:/usr/bin")]);
        assert!(
            change(&changes, "PATH").is_none(),
            "a longer mount name is a different mount: {changes:?}"
        );
    }

    #[test]
    fn variables_the_bundle_never_touched_are_left_alone() {
        // Including one whose value contains a colon and one that names a path elsewhere: the
        // rule is about the bundle, not about path-shaped values.
        let changes = scrub(&[
            ("GTK_THEME", "Adwaita:dark"),
            ("HOME", "/home/u"),
            ("PYTHONPATH", "/home/u/lib/python"),
            ("TRACKER_TOKEN", "secret"),
        ]);
        assert!(changes.is_empty(), "{changes:?}");
    }

    #[test]
    fn the_bundle_markers_go_even_though_they_name_no_path_inside_it() {
        let changes = scrub(&[
            ("APPDIR", APPDIR),
            ("APPIMAGE", "/home/u/bin/cide_0.1.0_amd64.AppImage"),
            ("ARGV0", "cide"),
            ("OWD", "/home/u/work/cide"),
        ]);
        for name in BUNDLE_MARKERS {
            assert_eq!(change(&changes, name), Some(&None), "{name}");
        }
        assert_eq!(changes.len(), 4, "each marker exactly once: {changes:?}");
    }

    #[test]
    fn a_development_run_changes_nothing_at_all() {
        // `APPDIR` unset is the path `bundle_scrub` takes for `./run.sh`, the `.deb` and the
        // Flatpak. An empty or relative one cannot be reasoned about and is treated the same.
        let vars = [("PATH".to_string(), "/usr/bin".to_string())];
        assert!(bundle_scrub_from(vars.clone(), "").is_empty());
        assert!(bundle_scrub_from(vars, "usr").is_empty());
    }
}
