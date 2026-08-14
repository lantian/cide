//! Ordering two version strings the way a person reads them.
//!
//! Pure, and its own module, because the wrong answer is invisible: a plain string compare puts
//! `0.10.0` *before* `0.9.0`, and this repository has 41 crate names present at two versions —
//! `base64 0.21.7` beside `base64 0.22.1`, `bitflags 1.3.2` beside `2.13.1`. A group where two
//! rows with the same name are in the wrong order reads as a sorting bug in the whole tree,
//! which is exactly the kind of thing nobody files.
//!
//! It is deliberately **not** semver. `semver::Version` would reject `v0.118.0` (go's leading
//! `v`), `1.0` (two components), `2023.1.1-beta+build` in the parts that matter and `master`
//! (a git dependency's branch), and a comparator that can fail is a comparator with a fallback
//! path nobody exercises. This one is total: it compares run-by-run, numbers numerically and
//! everything else lexically, and never panics.

use std::cmp::Ordering;

/// Compare two version strings.
///
/// The rules, in the order they apply:
///
/// * A leading `v` is ignored on both sides, so go's `v1.2.3` sorts with cargo's `1.2.3`.
/// * Build metadata (`+…`) is dropped. It is not part of precedence in semver and is not part
///   of any version either toolchain prints for a dependency.
/// * The remainder splits at the first `-` into a **core** and a **pre-release**.
/// * Cores compare run-wise: maximal runs of digits compare as numbers (`9 < 10`, leading zeros
///   ignored), everything else compares as text, a digit run sorts before a non-digit run, and
///   the side that runs out first is the smaller one — `1.2` before `1.2.1`.
/// * A version **with** a pre-release is less than the same core **without** one: `1.0.0-rc.1`
///   before `1.0.0`. This is the one semver rule worth reproducing because it is the one people
///   notice, and it is also what puts a go pseudo-version (`v0.0.0-20200101…-abcdef`) below the
///   tagged release it precedes.
///
/// Digit runs are compared as arbitrary-length decimal strings rather than parsed into an
/// integer. A run long enough to overflow `u64` is not a version anybody wrote, but parsing it
/// would either panic or silently answer `0`, and both are worse than a comparison that simply
/// keeps working.
///
/// The pre-release split is why this is not one flat run-wise walk. The flat version was the
/// first draft and it cannot express both rules at once: `1.2` < `1.2.1` wants the shorter side
/// to win when it runs out, and `1.0.0-alpha` < `1.0.0` wants the *longer* side to lose — and
/// the two cases are indistinguishable to a walk that treats `.` and `-` as the same kind of
/// separator.
pub fn version_cmp(a: &str, b: &str) -> Ordering {
    let (core_a, pre_a) = split(a);
    let (core_b, pre_b) = split(b);
    run_cmp(core_a, core_b).then_with(|| match (pre_a, pre_b) {
        (None, None) => Ordering::Equal,
        // A release outranks its own pre-releases.
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => run_cmp(x, y),
    })
}

/// `v1.2.3-rc.1+build` → `("1.2.3", Some("rc.1"))`.
fn split(version: &str) -> (&str, Option<&str>) {
    let version = strip_v(version);
    let version = version.split('+').next().unwrap_or(version);
    match version.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (version, None),
    }
}

/// Compare two dotted strings run by run. See [`version_cmp`] for the rules.
fn run_cmp(a: &str, b: &str) -> Ordering {
    let mut lhs = runs(a);
    let mut rhs = runs(b);
    loop {
        match (lhs.next(), rhs.next()) {
            (None, None) => return Ordering::Equal,
            // The side that ran out is the shorter one: `1.2` before `1.2.1`.
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let ordering = match (is_digits(x), is_digits(y)) {
                    (true, true) => numeric_cmp(x, y),
                    // A numeric field sorts below an alphanumeric one, which is semver's rule
                    // for pre-release identifiers and a reasonable one for `1.0` vs `master`.
                    (true, false) => Ordering::Less,
                    (false, true) => Ordering::Greater,
                    (false, false) => x.cmp(y),
                };
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
        }
    }
}

fn strip_v(s: &str) -> &str {
    // Only when a digit follows, so a module called `vendor` is not turned into `endor`.
    match s.strip_prefix('v') {
        Some(rest) if rest.starts_with(|c: char| c.is_ascii_digit()) => rest,
        _ => s,
    }
}

fn is_digits(run: &str) -> bool {
    run.starts_with(|c: char| c.is_ascii_digit())
}

/// Two decimal strings, compared as numbers without parsing either.
fn numeric_cmp(a: &str, b: &str) -> Ordering {
    let a = a.trim_start_matches('0');
    let b = b.trim_start_matches('0');
    // More digits is a bigger number, once the leading zeros are gone.
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

/// Split into maximal runs of digits and maximal runs of everything else.
fn runs(s: &str) -> impl Iterator<Item = &str> {
    let mut rest = s;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let digits = rest.starts_with(|c: char| c.is_ascii_digit());
        let end = rest
            .find(|c: char| c.is_ascii_digit() != digits)
            .unwrap_or(rest.len());
        let (run, tail) = rest.split_at(end);
        rest = tail;
        Some(run)
    })
}

/// Compare names the way a file tree does: case-insensitively, so `Cargo` and `build` sort next
/// to each other rather than in two ASCII blocks.
///
/// The same rule `cide_fs::index::name_cmp` applies to filenames, restated here rather than
/// shared because `cide-deps` must not depend on `cide-fs` — that dependency would run the wrong
/// way for a crate the file tree consumes.
pub fn name_cmp(a: &str, b: &str) -> Ordering {
    let mut lhs = a.chars().flat_map(char::to_lowercase);
    let mut rhs = b.chars().flat_map(char::to_lowercase);
    loop {
        match (lhs.next(), rhs.next()) {
            (Some(x), Some(y)) if x == y => continue,
            (Some(x), Some(y)) => return x.cmp(&y),
            (rest_a, rest_b) => return rest_a.is_some().cmp(&rest_b.is_some()),
        }
    }
}

/// How two package rows are ordered: by name, then by version, then by the thing that makes
/// them distinguishable at all.
///
/// The third key is the identity — the directory the row opens — and it is here so the sort is
/// *total*. Two rows that compare equal on name and version are two different unpacked copies of
/// one crate (a `[patch]`, a vendored fork), and leaving their order to the sort's stability
/// would make it depend on the order `cargo metadata` happened to print them in.
pub fn row_cmp(a: (&str, &str, &str), b: (&str, &str, &str)) -> Ordering {
    name_cmp(a.0, b.0)
        .then_with(|| version_cmp(a.1, b.1))
        .then_with(|| a.2.cmp(b.2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering::{Equal, Greater, Less};

    #[test]
    fn ten_is_greater_than_nine() {
        // The whole reason this module exists: `"0.10.0" < "0.9.0"` as strings.
        assert_eq!(version_cmp("0.10.0", "0.9.0"), Greater);
        assert_eq!(version_cmp("0.9.0", "0.10.0"), Less);
        assert!("0.10.0" < "0.9.0", "the string compare really is wrong");
    }

    #[test]
    fn the_two_versions_this_repository_actually_ships_twice() {
        assert_eq!(version_cmp("0.21.7", "0.22.1"), Less);
        assert_eq!(version_cmp("1.3.2", "2.13.1"), Less);
    }

    #[test]
    fn gos_leading_v_does_not_change_the_order() {
        assert_eq!(version_cmp("v1.2.3", "1.2.3"), Equal);
        assert_eq!(version_cmp("v0.118.0", "v0.99.0"), Greater);
    }

    #[test]
    fn a_release_outranks_its_own_prereleases() {
        assert_eq!(version_cmp("1.0.0-alpha", "1.0.0"), Less);
        assert_eq!(version_cmp("1.0.0-alpha", "1.0.0-beta"), Less);
        assert_eq!(version_cmp("1.0.0-rc.2", "1.0.0-rc.10"), Less);
        // Go's pseudo-version for an untagged commit, which must sort below the tag it precedes.
        assert_eq!(
            version_cmp("v0.0.0-20200101120000-abcdef123456", "v0.0.0"),
            Less
        );
    }

    #[test]
    fn build_metadata_is_not_part_of_the_order() {
        assert_eq!(version_cmp("1.0.0+build.7", "1.0.0"), Equal);
    }

    /// The two rules the flat run-wise walk could not hold at once. See [`version_cmp`].
    #[test]
    fn a_shorter_core_wins_while_a_prerelease_suffix_loses() {
        assert_eq!(version_cmp("1.2", "1.2.1"), Less);
        assert_eq!(version_cmp("1.2.1-rc.1", "1.2.1"), Less);
    }

    #[test]
    fn a_shorter_version_comes_first() {
        assert_eq!(version_cmp("1.2", "1.2.1"), Less);
        assert_eq!(version_cmp("1.2.0", "1.2"), Greater);
    }

    #[test]
    fn leading_zeros_are_not_a_different_number() {
        assert_eq!(version_cmp("1.02.3", "1.2.3"), Equal);
        assert_eq!(version_cmp("0.0.0", "0.0.0"), Equal);
    }

    #[test]
    fn a_version_that_is_not_a_version_still_orders_and_never_panics() {
        // git dependencies carry a branch or a rev, and `Version` is `None` for a module go
        // could not resolve — both reach this comparator.
        assert_eq!(version_cmp("master", "main"), Greater);
        assert_eq!(version_cmp("", ""), Equal);
        assert_eq!(version_cmp("", "1.0"), Less);
        assert_eq!(
            version_cmp("99999999999999999999999", "99999999999999999999998"),
            Greater,
            "a run wider than u64 must still compare rather than overflow"
        );
    }

    #[test]
    fn names_sort_case_insensitively_and_rows_break_ties_on_identity() {
        assert_eq!(name_cmp("anyhow", "Base64"), Less);
        assert_eq!(
            row_cmp(("base64", "0.22.1", "/a"), ("base64", "0.21.7", "/b")),
            Greater
        );
        assert_eq!(
            row_cmp(("serde", "1.0.0", "/a"), ("serde", "1.0.0", "/b")),
            Less,
            "two unpacked copies of one version must have a deterministic order"
        );
    }
}
