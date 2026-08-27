//! Finding one `### Requirement:` block inside a delta file, as a byte range. (M28)
//!
//! # This is not a markdown parser, and the distinction is the whole design
//!
//! The crate header explains why cide does not parse OpenSpec's markdown. This module is the
//! exception that proves it: the CLI has **no content-mutation command** — `new change`
//! scaffolds from templates and `archive` merges, and nothing adds or edits a requirement — so a
//! panel that lets somebody fix a requirement has to write the file itself.
//!
//! What it does is deliberately smaller than parsing. It answers one question — *where in these
//! bytes does the block named `X` under `## <OP> Requirements` start and end* — and the writer
//! then splices a replacement into that range and **copies every other byte unchanged**. Nothing
//! is re-serialised, so front matter, `## Purpose`, comments, blank-line style, trailing
//! whitespace and every other requirement in the file survive byte for byte. That property is
//! the round-trip test in `tests/`, and it is what makes editing a committed file safe.
//!
//! The scan mirrors `dist/core/parsers/requirement-blocks.js` and `code-fence.js` in
//! `@fission-ai/openspec` line for line, and the comments below name the rules it copies. Where
//! it differs it is *stricter*, never looser: an ambiguity is refused rather than resolved.

/// Where a requirement block lives, as a byte range into the text it was found in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub start: usize,
    pub end: usize,
}

/// Why a block could not be addressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Missing {
    /// The `## <OP> Requirements` section is not in this file.
    NoSection,
    /// The section is there and holds no requirement of that name.
    NoRequirement,
    /// **Two requirements share the name.**
    ///
    /// Refused rather than resolved, and this is the one place the scan is deliberately stricter
    /// than the CLI. A duplicate name is a file the user has to fix — `archive` will refuse it
    /// too — and picking one of the two here would land an edit in a block chosen by nothing
    /// better than document order, silently, in a committed file.
    Ambiguous(usize),
}

/// Which lines of `text` are inside a fenced code block.
///
/// One `bool` per line, and **both fence lines are masked**, matching the CLI. Without this a
/// documentation example — a fenced snippet showing what a requirement looks like, which
/// OpenSpec's own templates contain — is indistinguishable from a requirement, and an edit aimed
/// at the real one would rewrite the example.
///
/// A fence opens on three or more backticks or tildes and closes on a run of the *same* marker at
/// least as long, which is CommonMark's rule and upstream's.
pub fn fence_mask(text: &str) -> Vec<bool> {
    let mut mask = Vec::new();
    let mut open: Option<(u8, usize)> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        let marker = trimmed.as_bytes().first().copied();
        let run = match marker {
            Some(b @ (b'`' | b'~')) => {
                let len = trimmed.bytes().take_while(|c| *c == b).count();
                (len >= 3).then_some((b, len))
            }
            _ => None,
        };
        match (open, run) {
            (None, Some((marker, len))) => {
                open = Some((marker, len));
                mask.push(true);
            }
            (Some((marker, len)), Some((closing, closing_len)))
                if marker == closing && closing_len >= len =>
            {
                open = None;
                mask.push(true);
            }
            (Some(_), _) => mask.push(true),
            (None, _) => mask.push(false),
        }
    }
    mask
}

/// One line of the document, with everything the scan needs to decide about it.
struct Line<'a> {
    text: &'a str,
    /// Byte offset of the line's first character in the whole document.
    start: usize,
    fenced: bool,
}

fn lines_of(text: &str) -> Vec<Line<'_>> {
    let mask = fence_mask(text);
    let mut lines = Vec::new();
    let mut start = 0usize;
    for (index, line) in text.split_inclusive('\n').enumerate() {
        let without_newline = line.strip_suffix('\n').unwrap_or(line);
        let without_newline = without_newline
            .strip_suffix('\r')
            .unwrap_or(without_newline);
        lines.push(Line {
            text: without_newline,
            start,
            fenced: mask.get(index).copied().unwrap_or(false),
        });
        start += line.len();
    }
    lines
}

/// The line's text with the things that are not content stripped: a byte-order mark, and the
/// leading whitespace a nested list item carries.
///
/// The BOM is the one that bites and it bites exactly once — on the **first line of the file**,
/// which for a delta is `## ADDED Requirements`. Without this, a file authored on Windows has no
/// sections at all, and the refusal a user sees is "there is no ADDED Requirements section" about
/// a file that visibly has one.
fn content<'a>(line: &Line<'a>) -> &'a str {
    line.text.trim_start_matches('\u{feff}').trim_start()
}

/// Is this an unfenced `## …` heading, and what does it say?
fn section_title<'a>(line: &Line<'a>) -> Option<&'a str> {
    if line.fenced {
        return None;
    }
    let rest = content(line).strip_prefix("##")?;
    // `###` is a requirement, not a section. Checked by what follows the two hashes.
    if rest.starts_with('#') {
        return None;
    }
    Some(rest.trim())
}

/// Is this an unfenced `### Requirement: <name>` header, and what is the name?
///
/// The header word is matched case-insensitively and the name is **not** — `normalizeRequirementName`
/// upstream trims and compares exactly, because two requirements differing only in case are two
/// requirements as far as the archive is concerned, and folding them here would let an edit aimed
/// at one land on the other.
fn requirement_name(line: &Line<'_>) -> Option<String> {
    if line.fenced {
        return None;
    }
    let rest = content(line).strip_prefix("###")?;
    if rest.starts_with('#') {
        return None;
    }
    let rest = rest.trim_start();
    let lower = rest.to_ascii_lowercase();
    let name = lower
        .strip_prefix("requirement:")
        .map(|_| &rest["requirement:".len()..])?;
    Some(name.trim().to_string())
}

/// Find the block for `requirement` under `## <operation> Requirements`.
///
/// `operation` is the file's own uppercase word — `ADDED`, `MODIFIED`, `REMOVED`, `RENAMED` —
/// which is what [`cide_ipc::DeltaOperation::header`] exists to supply.
///
/// The block runs from its `###` header to **whichever comes first**: the next requirement header,
/// or the next `##` section. The second half is the one that is easy to forget and it is why the
/// last requirement in a section does not swallow the section after it.
pub fn find(text: &str, operation: &str, requirement: &str) -> Result<Block, Missing> {
    let lines = lines_of(text);
    let wanted_section = format!("{} requirements", operation.to_ascii_lowercase());

    let mut in_section = false;
    let mut hits: Vec<usize> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if let Some(title) = section_title(line) {
            in_section = title.to_ascii_lowercase() == wanted_section;
            continue;
        }
        if !in_section {
            continue;
        }
        if requirement_name(line).as_deref() == Some(requirement.trim()) {
            hits.push(index);
        }
    }

    match hits.len() {
        0 => {
            // Told apart so the refusal can say which mistake it was: a missing section usually
            // means the delta operation is wrong, a missing requirement usually means the name is.
            let has_section = lines
                .iter()
                .filter_map(section_title)
                .any(|title| title.to_ascii_lowercase() == wanted_section);
            Err(if has_section {
                Missing::NoRequirement
            } else {
                Missing::NoSection
            })
        }
        1 => {
            let start_line = hits[0];
            let start = lines[start_line].start;
            let end = lines
                .iter()
                .skip(start_line + 1)
                .find(|line| requirement_name(line).is_some() || section_title(line).is_some())
                .map(|line| line.start)
                .unwrap_or_else(|| text.len());
            Ok(Block { start, end })
        }
        n => Err(Missing::Ambiguous(n)),
    }
}

/// Every requirement name in a section, in document order. Used by the round-trip test and by the
/// writer's `MODIFIED` scenario-loss check.
pub fn names(text: &str, operation: &str) -> Vec<String> {
    let lines = lines_of(text);
    let wanted = format!("{} requirements", operation.to_ascii_lowercase());
    let mut in_section = false;
    let mut found = Vec::new();
    for line in &lines {
        if let Some(title) = section_title(line) {
            in_section = title.to_ascii_lowercase() == wanted;
            continue;
        }
        if in_section && let Some(name) = requirement_name(line) {
            found.push(name);
        }
    }
    found
}

/// One requirement block, taken apart into the pieces an editor shows. (M28)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    /// The name off the `### Requirement: <name>` header — what `archive` matches on.
    pub name: String,
    /// The prose between the header and the first scenario, as written.
    pub text: String,
    pub scenarios: Vec<Scenario>,
}

/// One `#### Scenario:` section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scenario {
    /// The title off its header.
    pub title: String,
    /// Everything under the header, as written — the `- **WHEN**` lines and whatever else.
    pub body: String,
}

/// Take one requirement block apart.
///
/// # Why this exists at all, when the CLI already returns requirements
///
/// **Because its JSON drops two things cide needs.** `openspec show --json` gives a requirement's
/// `text` *without* its `### Requirement:` header, and each scenario's `rawText` *without* its
/// `#### Scenario:` header — so the requirement's **name** and every scenario's **title** are
/// simply absent from the wire. The name is what `archive` matches on and what the write path
/// addresses a block by; the titles are what a reviewer reads. Rebuilding a block from that JSON
/// would silently rename every requirement to nothing and delete every scenario title in the
/// file.
///
/// So the panel's requirements are read from the delta file with this, addressed by the *same*
/// scanner the writer uses — which is the property that matters: the name shown on a card is by
/// construction the name an edit to that card will address.
pub fn parse(block: &str) -> Parsed {
    let lines = lines_of(block);
    let name = lines.first().and_then(requirement_name).unwrap_or_default();

    // Where each scenario header sits, so the prose is everything before the first one.
    let heads: Vec<(usize, String)> = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| scenario_title(line).map(|title| (index, title)))
        .collect();

    let prose_end = heads
        .first()
        .map_or(block.len(), |(index, _)| lines[*index].start);
    let prose_start = lines.first().map_or(0, |line| {
        // Everything after the header line, which is the block's first line.
        lines
            .get(1)
            .map_or(block.len(), |next| next.start)
            .max(line.start)
    });
    let text = block
        .get(prose_start..prose_end)
        .unwrap_or_default()
        .trim()
        .to_string();

    let mut scenarios = Vec::new();
    for (position, (index, title)) in heads.iter().enumerate() {
        let start = lines.get(index + 1).map_or(block.len(), |line| line.start);
        let end = heads
            .get(position + 1)
            .map_or(block.len(), |(next, _)| lines[*next].start);
        scenarios.push(Scenario {
            title: title.clone(),
            body: block.get(start..end).unwrap_or_default().trim().to_string(),
        });
    }

    Parsed {
        name,
        text,
        scenarios,
    }
}

/// Is this an unfenced `#### Scenario: <title>` header, and what is the title?
fn scenario_title(line: &Line<'_>) -> Option<String> {
    if line.fenced {
        return None;
    }
    let rest = content(line).strip_prefix("####")?;
    if rest.starts_with('#') {
        return None;
    }
    let rest = rest.trim_start();
    let lower = rest.to_ascii_lowercase();
    lower
        .strip_prefix("scenario:")
        .map(|_| rest["scenario:".len()..].trim().to_string())
}

/// The `#### Scenario:` names inside one block of requirement text.
///
/// Used to refuse the silent scenario drop that `archive` refuses at the far end: a `MODIFIED`
/// requirement replaces its whole block, so a replacement missing a scenario the main spec has
/// deletes it, and finding that out at archive time is finding it out far too late.
pub fn scenario_names(block: &str) -> Vec<String> {
    lines_of(block).iter().filter_map(scenario_title).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "\
# Delta

## ADDED Requirements

### Requirement: Theme switching
The app SHALL switch themes.

#### Scenario: toggled
- **WHEN** the user picks dark
- **THEN** the editor repaints

### Requirement: Remembering
The app SHALL remember the choice.

#### Scenario: relaunch
- **WHEN** cide restarts
- **THEN** the theme is the one chosen

## MODIFIED Requirements

### Requirement: Theme switching
The app SHALL switch themes per window.

#### Scenario: two windows
- **WHEN** two windows are open
- **THEN** each keeps its own
";

    fn slice<'a>(text: &'a str, block: &Block) -> &'a str {
        &text[block.start..block.end]
    }

    #[test]
    fn a_block_is_found_in_the_section_the_operation_names() {
        // The same requirement name under two operations is the ordinary case in a delta file,
        // and picking the wrong section would edit the wrong statement of the same requirement.
        let added = find(DOC, "ADDED", "Theme switching").expect("found");
        assert!(slice(DOC, &added).contains("SHALL switch themes."));
        assert!(!slice(DOC, &added).contains("per window"));

        let modified = find(DOC, "MODIFIED", "Theme switching").expect("found");
        assert!(slice(DOC, &modified).contains("per window"));
    }

    #[test]
    fn a_block_ends_at_the_next_requirement_and_also_at_the_next_section() {
        // The second half is the one that is easy to forget: without it the last requirement of a
        // section swallows every section after it, and an edit would delete them.
        let first = find(DOC, "ADDED", "Theme switching").expect("found");
        assert!(!slice(DOC, &first).contains("Remembering"));

        let last = find(DOC, "ADDED", "Remembering").expect("found");
        let text = slice(DOC, &last);
        assert!(text.contains("relaunch"));
        assert!(
            !text.contains("## MODIFIED"),
            "the last requirement in a section stops at the section boundary: {text:?}"
        );
    }

    #[test]
    fn a_fenced_requirement_header_is_not_a_requirement() {
        // OpenSpec's own templates carry fenced examples of what a requirement looks like. An
        // edit aimed at the real one must not rewrite the documentation.
        let doc = "\
## ADDED Requirements

Write them like this:

```markdown
### Requirement: Example
The app SHALL do the thing.
```

### Requirement: Real
The app SHALL really do it.
";
        assert_eq!(names(doc, "ADDED"), vec!["Real".to_string()]);
        let block = find(doc, "ADDED", "Real").expect("found");
        assert!(slice(doc, &block).contains("really"));
        assert_eq!(find(doc, "ADDED", "Example"), Err(Missing::NoRequirement));
    }

    #[test]
    fn a_tilde_fence_and_a_longer_closing_run_are_honoured() {
        let doc = "\
## ADDED Requirements

~~~
### Requirement: Fenced
~~~~

### Requirement: Real
Body.
";
        assert_eq!(names(doc, "ADDED"), vec!["Real".to_string()]);
    }

    #[test]
    fn a_duplicate_name_is_refused_rather_than_guessed() {
        // Picking one of the two would land an edit in a block chosen by document order, in a
        // committed file, with nothing on screen saying so. `archive` refuses this file too.
        let doc = "\
## ADDED Requirements

### Requirement: Twice
One.

### Requirement: Twice
Two.
";
        assert_eq!(find(doc, "ADDED", "Twice"), Err(Missing::Ambiguous(2)));
    }

    #[test]
    fn a_missing_section_and_a_missing_requirement_are_told_apart() {
        // Different mistakes: the first is usually the wrong operation, the second the wrong name.
        assert_eq!(
            find(DOC, "REMOVED", "Theme switching"),
            Err(Missing::NoSection)
        );
        assert_eq!(
            find(DOC, "ADDED", "Nonexistent"),
            Err(Missing::NoRequirement)
        );
    }

    #[test]
    fn a_name_differing_only_in_case_is_a_different_requirement() {
        // `normalizeRequirementName` compares exactly. Folding case here would let an edit aimed
        // at one land on the other.
        assert_eq!(
            find(DOC, "ADDED", "theme switching"),
            Err(Missing::NoRequirement)
        );
        // …but the header word itself is matched loosely, because that is markup, not a name.
        let doc = "## ADDED Requirements\n\n### requirement: Loose\nBody.\n";
        assert_eq!(names(doc, "ADDED"), vec!["Loose".to_string()]);
    }

    #[test]
    fn a_byte_order_mark_does_not_hide_the_first_section() {
        // The BOM bites exactly once — on the first line of the file, which for a delta is the
        // `## ADDED Requirements` heading. Without tolerating it, a file authored on Windows has
        // no sections at all and the refusal reads "there is no ADDED Requirements section" about
        // a file that visibly has one. Found by the corpus round-trip, not by review.
        let doc = format!("{}{DOC}", '\u{feff}');
        assert_eq!(
            names(&doc, "ADDED"),
            vec!["Theme switching".to_string(), "Remembering".to_string()]
        );
        assert!(find(&doc, "ADDED", "Remembering").is_ok());
    }

    #[test]
    fn crlf_lines_are_addressed_the_same_as_lf_ones() {
        let doc = DOC.replace('\n', "\r\n");
        let block = find(&doc, "ADDED", "Remembering").expect("found");
        let text = &doc[block.start..block.end];
        assert!(text.contains("relaunch"));
        assert!(!text.contains("MODIFIED"));
    }

    #[test]
    fn scenarios_are_named_so_a_modified_block_cannot_drop_one_silently() {
        let block = slice(DOC, &find(DOC, "ADDED", "Theme switching").expect("found"));
        assert_eq!(scenario_names(block), vec!["toggled".to_string()]);
    }

    #[test]
    fn a_block_gives_back_its_name_its_prose_and_its_scenarios_with_their_titles() {
        // The two things `openspec show --json` drops: the requirement's name (its `text` has no
        // header) and every scenario's title (its `rawText` is the body only). Rebuilding a block
        // from that JSON would rename the requirement to nothing and delete every title.
        let block = slice(DOC, &find(DOC, "ADDED", "Theme switching").expect("found"));
        let parsed = parse(block);
        assert_eq!(parsed.name, "Theme switching");
        assert_eq!(parsed.text, "The app SHALL switch themes.");
        assert_eq!(parsed.scenarios.len(), 1);
        assert_eq!(parsed.scenarios[0].title, "toggled");
        assert_eq!(
            parsed.scenarios[0].body,
            "- **WHEN** the user picks dark\n- **THEN** the editor repaints"
        );
    }

    #[test]
    fn a_requirement_with_several_scenarios_keeps_each_ones_boundary() {
        let doc = "\
## ADDED Requirements

### Requirement: R
The app SHALL do it.
And it SHALL keep doing it.

#### Scenario: one
- **WHEN** a
- **THEN** b

#### Scenario: two
- **WHEN** c
- **THEN** d
";
        let parsed = parse(slice(doc, &find(doc, "ADDED", "R").expect("found")));
        assert_eq!(
            parsed.text,
            "The app SHALL do it.\nAnd it SHALL keep doing it."
        );
        assert_eq!(
            parsed
                .scenarios
                .iter()
                .map(|s| s.title.as_str())
                .collect::<Vec<_>>(),
            vec!["one", "two"]
        );
        assert_eq!(parsed.scenarios[0].body, "- **WHEN** a\n- **THEN** b");
        assert_eq!(parsed.scenarios[1].body, "- **WHEN** c\n- **THEN** d");
    }

    #[test]
    fn a_requirement_with_no_scenarios_is_all_prose() {
        // What a REMOVED delta looks like: a reason and a migration, no scenarios at all.
        let doc = "## REMOVED Requirements\n\n### Requirement: Gone\n**Reason**: superseded.\n";
        let parsed = parse(slice(doc, &find(doc, "REMOVED", "Gone").expect("found")));
        assert_eq!(parsed.name, "Gone");
        assert_eq!(parsed.text, "**Reason**: superseded.");
        assert!(parsed.scenarios.is_empty());
    }

    #[test]
    fn every_block_splices_back_as_itself() {
        // The round-trip guarantee in miniature; `tests/corpus.rs` runs it over real files.
        for operation in ["ADDED", "MODIFIED"] {
            for name in names(DOC, operation) {
                let block = find(DOC, operation, &name).expect("found");
                let spliced = format!(
                    "{}{}{}",
                    &DOC[..block.start],
                    &DOC[block.start..block.end],
                    &DOC[block.end..]
                );
                assert_eq!(spliced, DOC, "{operation}/{name} did not round-trip");
            }
        }
    }
}
