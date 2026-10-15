//! Native clipboard metadata. Source roots come from the copying backend, never the webview.
//! The desktop clipboard carries this alongside plain-text paths; it is not a settings file.

use std::{
    collections::HashSet,
    path::{Component, Path, PathBuf},
};

use cide_ipc::{FileClip, PasteMode};
use serde::{Deserialize, Serialize};

pub const FORMAT: &str = "application/x-cide-file-clip+json";
pub const URI_LIST: &str = "text/uri-list";
pub const GNOME_FILES: &str = "x-special/gnome-copied-files";
pub const KDE_CUT: &str = "application/x-kde-cutselection";
pub const MAX_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeFileClip {
    version: u32,
    pub instance: String,
    pub clip: FileClip,
    pub roots: Vec<PathBuf>,
}

impl NativeFileClip {
    /// Import an actual file-list clipboard target, never path-looking plain text. The
    /// desktop's ownership epoch plus the selection contents gives repeated reads one ID.
    pub fn from_uri_list(
        bytes: &[u8],
        mode: PasteMode,
        epoch: &str,
    ) -> Result<Option<Self>, String> {
        let text = bounded_text(bytes)?;
        let mut paths = Vec::new();
        let mut selected = HashSet::new();
        for line in text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
        {
            let url =
                url::Url::parse(line).map_err(|_| "The clipboard contains an invalid file URL.")?;
            let valid_escapes = line.as_bytes().iter().enumerate().all(|(i, &b)| {
                b != b'%'
                    || line
                        .as_bytes()
                        .get(i + 1..i + 3)
                        .is_some_and(|s| s.iter().all(u8::is_ascii_hexdigit))
            });
            if !valid_escapes
                || url.scheme() != "file"
                || url
                    .host_str()
                    .is_some_and(|h| !h.is_empty() && h != "localhost")
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err("The clipboard contains an unsupported or non-local file URL.".into());
            }
            let path = url
                .to_file_path()
                .map_err(|_| "The clipboard contains an invalid local file URL.")?;
            if selected.insert(path.clone()) {
                paths.push(path);
            }
        }
        if paths.is_empty() {
            return Ok(None);
        }
        // Selecting a directory and one of its children should transfer the directory once.
        let paths: Vec<PathBuf> = paths
            .iter()
            .filter(|p| {
                !p.ancestors()
                    .skip(1)
                    .any(|parent| selected.contains(parent))
            })
            .cloned()
            .collect();
        let mut roots: Vec<PathBuf> = paths
            .iter()
            .map(|p| p.parent().unwrap_or(p).to_path_buf())
            .collect();
        roots.sort();
        roots.dedup();
        let identity = serde_json::to_vec(&(epoch, mode, &paths)).map_err(|e| e.to_string())?;
        let hash = blake3::hash(&identity);
        let id = uuid::Uuid::from_bytes(hash.as_bytes()[..16].try_into().expect("16 hash bytes"))
            .to_string();
        let value = Self {
            version: 1,
            instance: uuid::Uuid::nil().to_string(),
            clip: FileClip {
                id,
                mode,
                project: None,
                paths,
            },
            roots,
        };
        value.validate()?;
        Ok(Some(value))
    }

    /// GNOME's native target: `copy` or `cut`, then one file URL per line.
    pub fn from_gnome_files(bytes: &[u8], epoch: &str) -> Result<Option<Self>, String> {
        let text = bounded_text(bytes)?;
        let (verb, uris) = text
            .split_once('\n')
            .ok_or("The file clipboard is missing its file URLs.")?;
        let mode = match verb.trim_end_matches('\r') {
            "copy" => PasteMode::Copy,
            "cut" => PasteMode::Cut,
            _ => return Err("The file clipboard has an invalid Copy/Cut operation.".into()),
        };
        Self::from_uri_list(uris.as_bytes(), mode, epoch)
    }

    pub fn new(instance: String, mut clip: FileClip, roots: Vec<PathBuf>) -> Result<Self, String> {
        clip.id = uuid::Uuid::new_v4().to_string();
        let value = Self {
            version: 1,
            instance,
            clip,
            roots,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_BYTES {
            return Err("too many paths on the file clipboard".into());
        }
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_BYTES {
            return Err("file clipboard payload is too large".into());
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        value.validate()?;
        Ok(value)
    }

    pub fn text(&self) -> String {
        self.clip
            .paths
            .iter()
            .map(|p| p.to_string_lossy())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A dialog must act on the snapshot it displayed, never on a newer Copy or Cut.
    pub fn matches(&self, id: &str, paths: &[PathBuf], mode: PasteMode) -> bool {
        self.clip.id == id && self.clip.paths == paths && self.clip.mode == mode
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err("unsupported file clipboard version".into());
        }
        if uuid::Uuid::parse_str(&self.instance).is_err()
            || uuid::Uuid::parse_str(&self.clip.id).is_err()
        {
            return Err("invalid file clipboard identity".into());
        }
        if self.clip.project.is_none() != (self.instance == uuid::Uuid::nil().to_string()) {
            return Err("invalid file clipboard origin".into());
        }
        let valid_path = |p: &Path| {
            p.is_absolute()
                && !p.as_os_str().as_encoded_bytes().contains(&0)
                && !p.components().any(|c| matches!(c, Component::ParentDir))
        };
        if self.roots.is_empty()
            || self.roots.iter().any(|p| !valid_path(p))
            || self.clip.paths.is_empty()
        {
            return Err("invalid file clipboard paths or source roots".into());
        }
        let roots: HashSet<&Path> = self.roots.iter().map(PathBuf::as_path).collect();
        for path in &self.clip.paths {
            if !valid_path(path) || !path.ancestors().any(|parent| roots.contains(parent)) {
                return Err("a file clipboard path is outside its source roots".into());
            }
            if self.clip.mode == PasteMode::Cut && roots.contains(path.as_path()) {
                return Err("a project root cannot be cut".into());
            }
        }
        Ok(())
    }
}

fn bounded_text(bytes: &[u8]) -> Result<&str, String> {
    if bytes.len() > MAX_BYTES {
        return Err("file clipboard payload is too large".into());
    }
    std::str::from_utf8(bytes).map_err(|_| "The file clipboard is not valid UTF-8.".into())
}

/// KDE's marker accompanies `text/uri-list`; absence is an ordinary Copy.
pub fn kde_mode(bytes: Option<&[u8]>) -> Result<PasteMode, String> {
    match bytes.map(bounded_text).transpose()?.map(str::trim) {
        None | Some("") | Some("0") => Ok(PasteMode::Copy),
        Some("1") => Ok(PasteMode::Cut),
        _ => Err("The file clipboard has an invalid Cut marker.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(mode: PasteMode) -> NativeFileClip {
        NativeFileClip::new(
            uuid::Uuid::new_v4().to_string(),
            FileClip {
                id: String::new(),
                mode,
                project: Some(cide_ipc::ProjectId::new()),
                paths: vec!["/source/a b/你好.txt".into(), "/source/new\nline".into()],
            },
            vec!["/source".into()],
        )
        .unwrap()
    }

    #[test]
    fn os_file_lists_decode_local_urls_and_keep_a_stable_selection_identity() {
        let bytes = b"# file selection\r\nfile:///source/a%20b.txt\r\nfile://localhost/source/%E4%BD%A0%E5%A5%BD%0Aname\r\nfile:///source/a%20b.txt\r\n";
        let value = NativeFileClip::from_uri_list(bytes, PasteMode::Copy, "selection-1")
            .unwrap()
            .unwrap();
        assert_eq!(value.clip.project, None);
        assert_eq!(
            value.clip.paths,
            vec![
                PathBuf::from("/source/a b.txt"),
                PathBuf::from("/source/你好\nname")
            ]
        );
        assert_eq!(value.roots, vec![PathBuf::from("/source")]);
        assert_eq!(
            NativeFileClip::from_uri_list(bytes, PasteMode::Copy, "selection-1")
                .unwrap()
                .unwrap(),
            value
        );
        assert_ne!(
            NativeFileClip::from_uri_list(bytes, PasteMode::Copy, "selection-2")
                .unwrap()
                .unwrap()
                .clip
                .id,
            value.clip.id
        );
        assert_eq!(
            NativeFileClip::decode(&value.encode().unwrap()).unwrap(),
            value
        );
        assert!(
            NativeFileClip::from_uri_list(b"# no files\r\n", PasteMode::Copy, "1")
                .unwrap()
                .is_none()
        );
        let folder = NativeFileClip::from_uri_list(
            b"file:///source/folder\nfile:///source/folder/child\n",
            PasteMode::Cut,
            "1",
        )
        .unwrap()
        .unwrap();
        assert_eq!(folder.clip.paths, vec![PathBuf::from("/source/folder")]);
    }

    #[test]
    fn gnome_and_kde_preserve_copy_and_cut_without_guessing_from_text() {
        for (verb, mode) in [("copy", PasteMode::Copy), ("cut", PasteMode::Cut)] {
            let value = NativeFileClip::from_gnome_files(
                format!("{verb}\nfile:///source/file\n").as_bytes(),
                "1",
            )
            .unwrap()
            .unwrap();
            assert_eq!(value.clip.mode, mode);
            assert_eq!(value.clip.paths, vec![PathBuf::from("/source/file")]);
        }
        assert_eq!(kde_mode(None).unwrap(), PasteMode::Copy);
        assert_eq!(kde_mode(Some(b"0")).unwrap(), PasteMode::Copy);
        assert_eq!(kde_mode(Some(b"1\n")).unwrap(), PasteMode::Cut);
        assert!(kde_mode(Some(b"not a marker")).is_err());
        assert!(NativeFileClip::from_gnome_files(b"move\nfile:///source/file", "1").is_err());
        assert!(NativeFileClip::from_gnome_files(b"cut", "1").is_err());
    }

    #[test]
    fn os_import_rejects_remote_malformed_and_partial_selections() {
        for uri in [
            "/source/file",
            "relative/path",
            "https://example.com/file",
            "sftp://host/path",
            "file://remote-host/source/file",
            "file:///source/a%00b",
            "file:///source/a%ZZ",
            "file:///source/file?query",
            "file:///source/file#fragment",
        ] {
            let bytes = format!("file:///source/valid\n{uri}\n");
            assert!(
                NativeFileClip::from_uri_list(bytes.as_bytes(), PasteMode::Copy, "1").is_err(),
                "accepted {uri}"
            );
        }
        assert!(NativeFileClip::from_uri_list(&[255], PasteMode::Copy, "1").is_err());
        assert!(
            NativeFileClip::from_uri_list(&vec![b'a'; MAX_BYTES + 1], PasteMode::Copy, "1")
                .is_err()
        );
        assert!(NativeFileClip::from_uri_list(b"file:///", PasteMode::Cut, "1").is_err());
    }

    #[test]
    fn native_roundtrip_preserves_paths_mode_roots_and_identity() {
        for mode in [PasteMode::Copy, PasteMode::Cut] {
            let value = clip(mode);
            assert_eq!(
                NativeFileClip::decode(&value.encode().unwrap()).unwrap(),
                value
            );
            assert_eq!(value.text(), "/source/a b/你好.txt\n/source/new\nline");
            assert!(value.matches(&value.clip.id, &value.clip.paths, mode));
            assert!(!value.matches("a-new-copy", &value.clip.paths, mode));
            assert!(!value.matches(&value.clip.id, &[], mode));
        }
    }

    #[test]
    fn native_payload_rejects_invalid_versions_paths_roots_and_cut_roots() {
        let value = clip(PasteMode::Copy);
        for (field, replacement) in [
            ("version", serde_json::json!(2)),
            ("instance", serde_json::json!("")),
            ("roots", serde_json::json!([])),
            ("roots", serde_json::json!(["relative"])),
        ] {
            let mut json = serde_json::to_value(&value).unwrap();
            json[field] = replacement;
            assert!(NativeFileClip::decode(&serde_json::to_vec(&json).unwrap()).is_err());
        }
        for path in ["relative", "/outside/file", "/source/../secret"] {
            let mut bad = value.clone();
            bad.clip.paths = vec![path.into()];
            assert!(bad.encode().is_err());
        }
        let mut root = value;
        root.clip.paths = root.roots.clone();
        assert!(root.encode().is_ok(), "copying a root is allowed");
        root.clip.mode = PasteMode::Cut;
        assert!(root.encode().is_err());
        assert!(NativeFileClip::decode(&vec![b' '; MAX_BYTES + 1]).is_err());
        assert!(NativeFileClip::decode(b"plain text paths").is_err());
    }
}
