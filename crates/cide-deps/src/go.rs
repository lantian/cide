//! `go list -m -e -json all`, and why each of those five flags is there.
//!
//! | flag | why |
//! | --- | --- |
//! | `-m` | **modules**, not packages. `go list -deps -json` would enumerate every package in the graph, which is an order of magnitude more rows and more work, and a module is the granularity a dependency browser shows. |
//! | `-e` | keep going past a module that cannot be loaded, and report the reason **on that module**. This is what makes a partial answer possible: 20 modules plus one row saying why the 21st is missing, instead of one group-level failure that hides the 20. |
//! | `-json` | a stream of objects, so the reason above has somewhere to live. |
//! | `all` | the whole module graph, including indirect dependencies. |
//!
//! The output is a **concatenation of JSON objects, not an array** — `{…}{…}{…}` — so it is
//! parsed with a streaming `Deserializer` rather than `from_slice::<Vec<_>>`, which fails on the
//! second object.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::{DepsError, Package, Resolved, Toolchain};

/// One object out of `go list -m -json`, cut down to what this crate reads.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Module {
    path: String,
    /// Absent for the main module — the project itself.
    version: Option<String>,
    /// The main module. Excluded: it is the project, already drawn above this group.
    #[serde(default)]
    main: bool,
    /// The unpacked directory in `GOMODCACHE`. **Absent for a module that is in the graph but
    /// not extracted**, which was 10 of 30 in a real project measured here. Absolute, which is
    /// why `go env GOMODCACHE` never has to be asked.
    dir: Option<PathBuf>,
    /// What `-e` carries instead of failing the whole command.
    error: Option<ModuleError>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ModuleError {
    err: String,
}

/// Every module the project in `dir` depends on.
pub fn resolve(dir: &Path) -> Resolved {
    match run(dir) {
        Ok(packages) => Resolved {
            packages,
            notes: Vec::new(),
        },
        Err(error) => Resolved {
            packages: Vec::new(),
            notes: vec![error.to_string()],
        },
    }
}

fn run(dir: &Path) -> Result<Vec<Package>, DepsError> {
    let binary = crate::locate(Toolchain::Go)?;
    let mut command = Command::new(binary);
    command
        .args(["list", "-m", "-e", "-json", "all"])
        .current_dir(dir)
        // The default since Go 1.16, set explicitly because it is a *guarantee* here and not a
        // preference: a user with `GOFLAGS=-mod=mod` in their environment would otherwise have
        // `go.mod` and `go.sum` rewritten by a click on a twisty in a file tree.
        .env("GOFLAGS", "-mod=readonly")
        // Makes a fetch impossible rather than merely unlikely. An uncached module then reports
        // `missing go.sum entry`, which `-e` turns into one row saying so — the right outcome,
        // and much better than a click that silently downloads a gigabyte of modules.
        .env("GOPROXY", "off");

    let stdout = crate::run(Toolchain::Go, command, "go list -m")?;
    parse(&stdout)
}

/// The parsing half, over bytes rather than a process. See `cargo::parse` for why it is split.
fn parse(stdout: &[u8]) -> Result<Vec<Package>, DepsError> {
    let mut packages = Vec::new();
    // A *stream*, because the output is `{…}{…}{…}` with no enclosing array. `from_slice` reads
    // the first object and then reports trailing characters, which would have looked like "this
    // project has exactly one dependency".
    let stream = serde_json::Deserializer::from_slice(stdout).into_iter::<Module>();
    for module in stream {
        let module = module.map_err(|e| {
            DepsError::Unreadable(format!("`go list -m` produced unreadable JSON: {e}"))
        })?;
        if module.main {
            continue;
        }
        packages.push(Package {
            // The **full module path**, never its last segment: `github.com/go-openapi/
            // jsonpointer` and a bare `jsonpointer` are not the same claim, and the module cache
            // is full of colliding last segments.
            name: module.path,
            version: module.version.unwrap_or_default(),
            dir: module.dir,
            note: module.error.map(|e| e.err),
        });
    }

    // A module with no directory and no error of its own: in the graph, not extracted. The row
    // exists, has no twisty, and says why — never triggering a download to fix it, which is what
    // `go mod download` here would mean and is not something a file tree may decide to do.
    for package in &mut packages {
        if package.dir.is_none() && package.note.is_none() {
            package.note = Some("not downloaded".to_string());
        }
    }
    Ok(packages)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim shape from `go1.25.5` — concatenated objects, no array, no separators.
    const DOC: &str = r#"{
        "Path": "github.com/higress-group/openapi-to-mcpserver",
        "Main": true,
        "Dir": "/home/u/work/x",
        "GoMod": "/home/u/work/x/go.mod",
        "GoVersion": "1.23.0"
}
{
        "Path": "github.com/getkin/kin-openapi",
        "Version": "v0.118.0",
        "Time": "2023-06-07T14:02:03Z",
        "Dir": "/home/u/go/pkg/mod/github.com/getkin/kin-openapi@v0.118.0",
        "GoMod": "/home/u/go/pkg/mod/cache/download/github.com/getkin/kin-openapi/@v/v0.118.0.mod",
        "GoVersion": "1.16"
}
{
        "Path": "github.com/davecgh/go-spew",
        "Version": "v1.1.1",
        "Indirect": true
}
{
        "Path": "example.com/broken",
        "Version": "v0.1.0",
        "Error": {"Err": "missing go.sum entry for go.mod file; to add it: go mod download example.com/broken"}
}"#;

    #[test]
    fn a_stream_of_objects_is_not_an_array() {
        // `serde_json::from_slice::<Vec<Module>>` fails on this input; a parser that used it
        // would report "one dependency" for a project with hundreds.
        let packages = parse(DOC.as_bytes()).expect("parse");
        assert_eq!(packages.len(), 3, "{packages:?}");
    }

    #[test]
    fn the_main_module_is_the_project_and_is_not_a_dependency() {
        let packages = parse(DOC.as_bytes()).expect("parse");
        assert!(
            !packages
                .iter()
                .any(|p| p.name.contains("openapi-to-mcpserver"))
        );
    }

    #[test]
    fn a_module_name_is_its_full_path() {
        let packages = parse(DOC.as_bytes()).expect("parse");
        let kin = &packages[0];
        assert_eq!(kin.name, "github.com/getkin/kin-openapi");
        assert_eq!(kin.version, "v0.118.0");
        assert_eq!(
            kin.dir.as_deref(),
            Some(Path::new(
                "/home/u/go/pkg/mod/github.com/getkin/kin-openapi@v0.118.0"
            ))
        );
    }

    /// 10 of 30 modules in a real project had no `Dir`: in the graph, not extracted.
    #[test]
    fn a_module_that_is_not_downloaded_gets_a_row_that_says_so() {
        let packages = parse(DOC.as_bytes()).expect("parse");
        let spew = packages
            .iter()
            .find(|p| p.name == "github.com/davecgh/go-spew")
            .expect("an unextracted module still has a row");
        assert_eq!(spew.dir, None, "there is nothing to expand into");
        assert_eq!(
            spew.note.as_deref(),
            Some("not downloaded"),
            "a row with no twisty and no explanation is indistinguishable from a broken one"
        );
    }

    /// What `-e` buys: a partial answer plus a per-row reason, instead of one group-level
    /// failure that hides every module that did resolve.
    #[test]
    fn a_module_that_failed_carries_gos_own_reason_and_the_others_still_resolve() {
        let packages = parse(DOC.as_bytes()).expect("parse");
        let broken = packages
            .iter()
            .find(|p| p.name == "example.com/broken")
            .expect("a failed module is still a row");
        assert!(
            broken
                .note
                .as_deref()
                .is_some_and(|n| n.contains("missing go.sum entry")),
            "{:?}",
            broken.note
        );
        assert!(
            packages.iter().any(|p| p.dir.is_some()),
            "one broken module must not cost the modules that resolved"
        );
    }

    #[test]
    fn unreadable_output_is_reported_rather_than_becoming_an_empty_group() {
        assert!(matches!(
            parse(b"{\"Path\": "),
            Err(DepsError::Unreadable(_))
        ));
    }
}
