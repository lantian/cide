//! The project symbol index: an authoritative store, and a fuzzy matcher derived from it.
//!
//! # Why there are two of them
//!
//! `cide_search::Matcher` has five methods — `extend`, `query`, `frame`, `clear`, `dirty` — and
//! **no `remove`**. nucleo's injector is append-only. So when a watcher burst says a file changed
//! and it is re-parsed, the symbols it declared a keystroke ago cannot be retracted: the picker
//! would offer `fn old_name` and `fn new_name` for ever.
//!
//! Adding `Matcher::remove` was the alternative and it loses. nucleo has no such operation, so
//! the trait method would have to be implemented as an internal rebuild — the same rebuild this
//! module does, one layer down, with the trait now promising something one implementation fakes.
//!
//! So:
//!
//! * [`SymbolStore`] is **authoritative**. A re-parse replaces a file's slot in place, so slot
//!   indices stay stable and a candidate can name one.
//! * The matcher is a **derived cache that may be stale**. New candidates are pushed at once, so
//!   a just-typed function is findable immediately; the ones they replaced linger.
//! * Staleness is filtered **at frame time**, never tolerated: [`SymbolStore::resolve`] drops any
//!   row the store no longer backs. Nothing stale is ever drawn.
//! * Past a threshold the matcher is rebuilt from the store, which is the only way the lingering
//!   entries ever actually go.
//!
//! `SymbolFrame::matched` is therefore a **ceiling** while a re-index is folding in, and its DTO
//! says so. The same honesty `SearchFrame::total` already practises.
//!
//! # Why a candidate carries a slot index and not a path
//!
//! ~30 bytes instead of ~150. A file's absolute path is 60–100 bytes and would be repeated once
//! per symbol in it; a 100-symbol file would carry a hundred copies. The path is interned once
//! per file in the store and looked up through the slot.

use std::collections::HashMap;
use std::sync::Arc;

use cide_ipc::{SymbolKind, SymbolRow};
use cide_search::Candidate;

/// Separates the fields of a candidate's `value`.
///
/// The ASCII unit separator, because it cannot occur in an identifier or a path — unlike `:`,
/// which is in every Windows path and in a Rust `::`, and unlike `\t`, which a filename may
/// legally contain.
const SEP: char = '\u{1f}';

/// One file's declarations, as the store holds them.
struct FileSymbols {
    path: Arc<str>,
    rel: Arc<str>,
    /// Container strings, interned per file.
    ///
    /// A 40-method `impl` block stores its header once rather than forty times, and impl headers
    /// are the longest strings in the index — `impl<T: Display> Trait<T> for Foo<T> where T:
    /// Clone` is 50 bytes before a single method is counted.
    containers: Vec<Arc<str>>,
    rows: Vec<FlatSymbol>,
}

/// One symbol, flattened.
///
/// The tree is gone by this point on purpose: the project picker shows a flat list, and keeping
/// `children` would carry a `Vec` per row for a shape nothing here reads.
struct FlatSymbol {
    kind: SymbolKind,
    name: Box<str>,
    /// Index into [`FileSymbols::containers`].
    container: Option<u16>,
    line: u32,
    column: u32,
    end_column: u32,
}

/// Every symbol in one project.
#[derive(Default)]
pub struct SymbolStore {
    /// A slot map: `None` is a free slot. The same arena-with-free-list shape `cide_fs::Index`
    /// uses, and for the same reason — an index that stays valid across a re-parse.
    entries: Vec<Option<FileSymbols>>,
    free: Vec<u32>,
    by_path: HashMap<Arc<str>, u32>,
    symbols: u32,
    /// The walk hit its whole-index cap.
    pub truncated: bool,
}

impl SymbolStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn files(&self) -> u32 {
        self.by_path.len() as u32
    }

    pub fn symbols(&self) -> u32 {
        self.symbols
    }

    /// Replace one file's symbols, returning the candidates to push and how many went stale.
    ///
    /// The count is what drives the rebuild threshold — see the module docs. A file seen for the
    /// first time makes none stale, which is the whole of a first walk.
    pub fn insert(&mut self, file: cide_lang::WalkedFile) -> (Vec<Candidate>, u32) {
        let path: Arc<str> = Arc::from(file.path.as_str());
        let mut containers: Vec<Arc<str>> = Vec::new();
        let mut rows: Vec<FlatSymbol> = Vec::new();
        flatten(&file.symbols, &mut containers, &mut rows);

        let stale = match self.by_path.get(&path).copied() {
            Some(slot) => {
                let previous = self.entries[slot as usize]
                    .as_ref()
                    .map_or(0, |f| f.rows.len() as u32);
                self.symbols -= previous;
                previous
            }
            None => 0,
        };

        let slot = match self.by_path.get(&path).copied() {
            Some(slot) => slot,
            None => match self.free.pop() {
                Some(slot) => {
                    self.by_path.insert(Arc::clone(&path), slot);
                    slot
                }
                None => {
                    let slot = self.entries.len() as u32;
                    self.entries.push(None);
                    self.by_path.insert(Arc::clone(&path), slot);
                    slot
                }
            },
        };

        self.symbols += rows.len() as u32;
        let candidates = candidates_for(slot, &rows);
        self.entries[slot as usize] = Some(FileSymbols {
            path,
            rel: Arc::from(file.rel.as_str()),
            containers,
            rows,
        });
        (candidates, stale)
    }

    /// Forget a file — it was deleted, or moved out of the project.
    pub fn remove(&mut self, abs_path: &str) -> u32 {
        let Some(slot) = self.by_path.remove(abs_path) else {
            return 0;
        };
        let gone = self.entries[slot as usize]
            .take()
            .map_or(0, |f| f.rows.len() as u32);
        self.symbols -= gone;
        self.free.push(slot);
        gone
    }

    /// Every candidate in the store, for a rebuild.
    pub fn candidates(&self) -> Vec<Candidate> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(slot, entry)| Some((slot as u32, entry.as_ref()?)))
            .flat_map(|(slot, file)| candidates_for(slot, &file.rows))
            .collect()
    }

    /// Turn one matcher row back into a [`SymbolRow`], or drop it as stale.
    ///
    /// `None` means the store no longer backs it — the file was re-parsed or deleted since the
    /// candidate was pushed. **This is the filter that makes a stale entry unable to reach the
    /// screen**, and it is why the matcher being append-only is survivable at all.
    pub fn resolve(&self, value: &str, name: &str, indices: Vec<u32>) -> Option<SymbolRow> {
        let (slot, index) = decode(value)?;
        let file = self.entries.get(slot as usize)?.as_ref()?;
        let row = file.rows.get(index as usize)?;
        // The name has to match too. A slot reused by a *different* file after a delete would
        // otherwise hand back whatever now sits at that index — a real row, for the wrong file,
        // with the query's highlight offsets on it.
        if &*row.name != name {
            return None;
        }
        Some(SymbolRow {
            kind: row.kind,
            name: row.name.to_string(),
            container: row
                .container
                .and_then(|i| file.containers.get(i as usize))
                .map(|c| c.to_string()),
            path: file.path.to_string(),
            rel: file.rel.to_string(),
            line: row.line,
            column: row.column,
            end_column: row.end_column,
            indices,
        })
    }
}

/// `{slot}\u{1f}{index}` — see the module docs for why this is not a path.
fn encode(slot: u32, index: usize) -> String {
    format!("{slot}{SEP}{index}")
}

fn decode(value: &str) -> Option<(u32, u32)> {
    let (slot, index) = value.split_once(SEP)?;
    Some((slot.parse().ok()?, index.parse().ok()?))
}

fn candidates_for(slot: u32, rows: &[FlatSymbol]) -> Vec<Candidate> {
    rows.iter()
        .enumerate()
        // `text` is the bare identifier, and that is a scoring decision rather than a shortcut.
        // With `text = "Server.Serve"`, typing `serve` also matches `S-e-r-v-e` inside `Server`,
        // and the row the user wants is diluted by six characters they never typed.
        .map(|(i, row)| Candidate::new(row.name.to_string(), encode(slot, i)))
        .collect()
}

/// Depth-first, dropping the tree and the two kinds the project index does not carry.
fn flatten(
    symbols: &[cide_ipc::Symbol],
    containers: &mut Vec<Arc<str>>,
    rows: &mut Vec<FlatSymbol>,
) {
    for symbol in symbols {
        // `Field` and `Variant` are structure-only. Nobody uses go-to-symbol to find a struct
        // field, and they are a large fraction of the count in both languages — withholding them
        // roughly halves the index. They are still in `FileOutline`, which is what the structure
        // popup reads.
        if !matches!(symbol.kind, SymbolKind::Field | SymbolKind::Variant) {
            let container = symbol
                .container
                .as_deref()
                .map(|text| intern(containers, text));
            rows.push(FlatSymbol {
                kind: symbol.kind,
                name: symbol.name.as_str().into(),
                container,
                line: symbol.selection.start_line,
                column: symbol.selection.start_column,
                end_column: symbol.selection.end_column,
            });
        }
        flatten(&symbol.children, containers, rows);
    }
}

/// Intern, or `None` if the table is full.
///
/// `u16`, so 65,535 distinct containers in one file. A file that exceeds that is generated, and
/// losing the container on its symbols is a better failure than widening every row by two bytes
/// for a case nobody has.
fn intern(containers: &mut Vec<Arc<str>>, text: &str) -> u16 {
    if let Some(index) = containers.iter().position(|c| &**c == text) {
        return index as u16;
    }
    if containers.len() >= u16::MAX as usize {
        return 0;
    }
    containers.push(Arc::from(text));
    (containers.len() - 1) as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::{Symbol, SymbolSpan};

    fn span(line: u32) -> SymbolSpan {
        SymbolSpan {
            start_line: line,
            start_column: 5,
            end_line: line,
            end_column: 9,
        }
    }

    fn symbol(name: &str, kind: SymbolKind, container: Option<&str>) -> Symbol {
        Symbol {
            kind,
            name: name.into(),
            detail: None,
            container: container.map(str::to_string),
            range: span(1),
            selection: span(1),
            children: Vec::new(),
        }
    }

    fn file(path: &str, symbols: Vec<Symbol>) -> cide_lang::WalkedFile {
        cide_lang::WalkedFile {
            path: path.into(),
            rel: path.trim_start_matches("/repo/").into(),
            lang: cide_lang::Lang::Rust,
            symbols,
        }
    }

    #[test]
    fn a_symbol_round_trips_through_its_candidate() {
        let mut store = SymbolStore::new();
        let (candidates, stale) = store.insert(file(
            "/repo/src/a.rs",
            vec![symbol("spawn", SymbolKind::Function, Some("net"))],
        ));
        assert_eq!(stale, 0);
        assert_eq!(candidates.len(), 1);

        let row = store
            .resolve(&candidates[0].value, "spawn", vec![0, 1])
            .expect("resolves");
        assert_eq!(row.name, "spawn");
        assert_eq!(row.container.as_deref(), Some("net"));
        assert_eq!(row.path, "/repo/src/a.rs");
        assert_eq!(row.rel, "src/a.rs");
        assert_eq!(row.indices, vec![0, 1]);
    }

    #[test]
    fn a_reparse_replaces_in_place_and_reports_what_went_stale() {
        // The number that drives the rebuild threshold. Without it the matcher grows without
        // bound over a long editing session.
        let mut store = SymbolStore::new();
        store.insert(file(
            "/repo/a.rs",
            vec![
                symbol("old_one", SymbolKind::Function, None),
                symbol("old_two", SymbolKind::Function, None),
            ],
        ));
        let (candidates, stale) = store.insert(file(
            "/repo/a.rs",
            vec![symbol("new_name", SymbolKind::Function, None)],
        ));
        assert_eq!(stale, 2, "the previous two were not counted as stale");
        assert_eq!(store.symbols(), 1);
        assert_eq!(store.files(), 1, "the file was added twice");
        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn a_candidate_for_a_symbol_that_no_longer_exists_resolves_to_nothing() {
        // The filter that keeps a stale row off the screen. The matcher is append-only, so this
        // is the *only* thing between a renamed function and a picker that offers both names.
        let mut store = SymbolStore::new();
        let (before, _) = store.insert(file(
            "/repo/a.rs",
            vec![symbol("old_name", SymbolKind::Function, None)],
        ));
        store.insert(file(
            "/repo/a.rs",
            vec![symbol("new_name", SymbolKind::Function, None)],
        ));

        assert!(
            store
                .resolve(&before[0].value, "old_name", Vec::new())
                .is_none(),
            "a renamed symbol is still reachable through its old candidate"
        );
    }

    #[test]
    fn a_slot_reused_by_another_file_does_not_hand_back_its_rows() {
        // The reason `resolve` checks the name as well as the slot. A free-list reuse would
        // otherwise return a real row from the wrong file, with the query's highlight offsets on
        // it — which reads as a working result and is not one.
        let mut store = SymbolStore::new();
        let (first, _) = store.insert(file(
            "/repo/a.rs",
            vec![symbol("from_a", SymbolKind::Function, None)],
        ));
        store.remove("/repo/a.rs");
        store.insert(file(
            "/repo/b.rs",
            vec![symbol("from_b", SymbolKind::Function, None)],
        ));

        assert!(
            store
                .resolve(&first[0].value, "from_a", Vec::new())
                .is_none()
        );
    }

    #[test]
    fn fields_and_variants_are_not_in_the_project_index() {
        // They *are* in `FileOutline` — the structure popup wants them. Nobody uses go-to-symbol
        // to find a struct field, and withholding them roughly halves the index.
        let mut store = SymbolStore::new();
        let mut parent = symbol("S", SymbolKind::Struct, None);
        parent.children = vec![
            symbol("a", SymbolKind::Field, Some("S")),
            symbol("b", SymbolKind::Field, Some("S")),
        ];
        let (candidates, _) = store.insert(file("/repo/a.rs", vec![parent]));
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].text, "S");
    }

    #[test]
    fn nested_symbols_are_flattened_and_keep_their_containers() {
        let mut store = SymbolStore::new();
        let mut outer = symbol("net", SymbolKind::Module, None);
        let mut inner = symbol("impl Display for Server", SymbolKind::Impl, Some("net"));
        inner.children = vec![symbol(
            "fmt",
            SymbolKind::Method,
            Some("net::impl Display for Server"),
        )];
        outer.children = vec![inner];
        let (candidates, _) = store.insert(file("/repo/a.rs", vec![outer]));

        assert_eq!(candidates.len(), 3);
        let fmt = candidates.iter().find(|c| c.text == "fmt").expect("fmt");
        let row = store
            .resolve(&fmt.value, "fmt", Vec::new())
            .expect("resolves");
        assert_eq!(
            row.container.as_deref(),
            Some("net::impl Display for Server")
        );
    }

    #[test]
    fn one_container_string_is_stored_once_however_many_members_use_it() {
        // The interning that makes an impl block's header cost 50 bytes rather than 50 × its
        // method count.
        let mut store = SymbolStore::new();
        let mut block = symbol("impl Foo", SymbolKind::Impl, None);
        block.children = (0..40)
            .map(|i| symbol(&format!("m{i}"), SymbolKind::Method, Some("impl Foo")))
            .collect();
        store.insert(file("/repo/a.rs", vec![block]));

        let entry = store.entries[0].as_ref().expect("slot 0");
        assert_eq!(
            entry.containers.len(),
            1,
            "the container was stored once per member"
        );
    }

    #[test]
    fn removing_a_file_frees_its_slot_for_reuse() {
        let mut store = SymbolStore::new();
        store.insert(file(
            "/repo/a.rs",
            vec![symbol("a", SymbolKind::Function, None)],
        ));
        assert_eq!(store.remove("/repo/a.rs"), 1);
        assert_eq!(store.symbols(), 0);
        assert_eq!(store.files(), 0);

        store.insert(file(
            "/repo/b.rs",
            vec![symbol("b", SymbolKind::Function, None)],
        ));
        assert_eq!(store.entries.len(), 1, "the freed slot was not reused");
    }

    #[test]
    fn removing_a_file_that_was_never_indexed_is_not_an_error() {
        let mut store = SymbolStore::new();
        assert_eq!(store.remove("/repo/never.rs"), 0);
    }

    #[test]
    fn a_rebuild_reproduces_every_live_candidate_and_no_dead_one() {
        let mut store = SymbolStore::new();
        store.insert(file(
            "/repo/a.rs",
            vec![symbol("a", SymbolKind::Function, None)],
        ));
        store.insert(file(
            "/repo/b.rs",
            vec![symbol("b", SymbolKind::Function, None)],
        ));
        store.remove("/repo/a.rs");

        let candidates = store.candidates();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].text, "b");
    }

    #[test]
    fn a_malformed_candidate_value_is_refused_rather_than_panicking() {
        // `value` crosses no process boundary today, but it is parsed with `split_once` and
        // `parse`, and a panic in a Tauri command takes the whole handler down.
        let store = SymbolStore::new();
        for value in ["", "nonsense", "1", "x\u{1f}y", "999\u{1f}999"] {
            assert!(
                store.resolve(value, "whatever", Vec::new()).is_none(),
                "{value}"
            );
        }
    }
}
