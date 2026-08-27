//! The round-trip guarantee, over files shaped like real ones. (M28)
//!
//! `cide_spec::write` replaces exactly one byte range and copies every other byte. This asserts
//! it: for every requirement in every corpus file, address the block, splice it back **with its
//! own text**, and demand the result is byte-identical to the input.
//!
//! That is the property that makes editing a committed file safe, and every way of breaking it is
//! invisible in a diff nobody reads until a pull request: a lost BOM, LF where a Windows file had
//! CRLF, a stripped trailing space, a blank line eaten between two requirements, a fenced example
//! rewritten because the scanner mistook it for the real thing.
//!
//! The files live in `tests/corpus/` rather than in string literals for one specific reason: a
//! Rust literal cannot hold `"###` (it closes every `r#`-family delimiter), and a corpus written
//! around that limitation would be a corpus that avoided the exact bytes under test.

use std::path::PathBuf;

use cide_spec::block;

fn corpus() -> Vec<(String, String)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut files: Vec<(String, String)> = std::fs::read_dir(&dir)
        .expect("the corpus directory is checked in")
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|e| e == "md"))
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let bytes = std::fs::read(entry.path()).expect("readable");
            // Lossless on purpose: the BOM file is valid UTF-8 and the BOM is a character the
            // scanner has to survive, not one the test may quietly strip.
            (name, String::from_utf8(bytes).expect("the corpus is UTF-8"))
        })
        .collect();
    files.sort();
    assert!(files.len() >= 6, "the corpus lost files");
    files
}

const OPERATIONS: [&str; 4] = ["ADDED", "MODIFIED", "REMOVED", "RENAMED"];

#[test]
fn every_requirement_in_the_corpus_splices_back_as_itself() {
    let mut checked = 0usize;
    for (name, text) in corpus() {
        for operation in OPERATIONS {
            for requirement in block::names(&text, operation) {
                let found = block::find(&text, operation, &requirement)
                    .unwrap_or_else(|e| panic!("{name}: {operation}/{requirement}: {e:?}"));
                let spliced = format!(
                    "{}{}{}",
                    &text[..found.start],
                    &text[found.start..found.end],
                    &text[found.end..]
                );
                assert_eq!(
                    spliced, text,
                    "{name}: replacing {operation}/{requirement} with itself changed the file"
                );
                checked += 1;
            }
        }
    }
    assert!(
        checked >= 8,
        "only {checked} requirements were addressed — the corpus or the scanner has gone quiet"
    );
}

#[test]
fn a_fenced_example_is_never_addressed() {
    // OpenSpec's own templates carry fenced examples of what a requirement looks like. If the
    // scanner saw one, an edit aimed at the real requirement would rewrite the documentation.
    let (_, text) = corpus()
        .into_iter()
        .find(|(name, _)| name == "fenced.md")
        .expect("the fenced fixture is checked in");
    assert_eq!(
        block::names(&text, "ADDED"),
        vec!["The real one".to_string()],
        "the fenced examples were counted as requirements"
    );
}

#[test]
fn a_windows_file_keeps_its_bom_and_its_line_endings() {
    let (_, text) = corpus()
        .into_iter()
        .find(|(name, _)| name == "crlf-bom.md")
        .expect("the CRLF fixture is checked in");
    assert!(
        text.starts_with('\u{feff}'),
        "the fixture lost its BOM on disk"
    );
    assert!(text.contains("\r\n"), "the fixture lost its CRLF on disk");

    // The scanner addresses it without normalising anything away.
    let found = block::find(&text, "ADDED", "Windows authored").expect("addressed");
    let block_text = &text[found.start..found.end];
    assert!(
        block_text.contains("\r\n"),
        "the block came back with LF: {block_text:?}"
    );
}

#[test]
fn a_requirement_stops_at_the_next_section_and_not_at_the_end_of_the_file() {
    // `modified.md` has a MODIFIED requirement followed by a REMOVED section. Without the
    // section boundary the first would swallow the second, and an edit would delete it.
    let (_, text) = corpus()
        .into_iter()
        .find(|(name, _)| name == "modified.md")
        .expect("the fixture is checked in");
    let found = block::find(&text, "MODIFIED", "Theme switching").expect("addressed");
    let block_text = &text[found.start..found.end];
    assert!(block_text.contains("per window"));
    assert!(
        !block_text.contains("REMOVED"),
        "the block ran past its section: {block_text:?}"
    );
}
