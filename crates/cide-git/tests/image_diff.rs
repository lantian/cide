mod support;

use cide_git::{
    diff,
    image_diff::{self, ImageBytes, ImageDiffBytes},
    revision,
};
use cide_ipc::{git::DiffSide, history::RevSide};
use support::TempRepo;

fn png(color: u8) -> Vec<u8> {
    cide_core::image::encode_png(&[color, 0, 0, 255], 1, 1).unwrap()
}

fn bytes(side: &ImageBytes) -> &[u8] {
    match side {
        ImageBytes::Ready(bytes) => bytes,
        other => panic!("expected an image, got {other:?}"),
    }
}

fn working(repo: &TempRepo, path: &str, side: DiffSide) -> ImageDiffBytes {
    let handle = cide_git::repo::open(&repo.root).unwrap();
    let (view, images) = image_diff::file_diff(&handle, path, side).unwrap();
    let raw = diff::file_diff(&handle, path, diff::DiffRequest::new(side).renames(true))
        .unwrap()
        .unwrap();
    assert_eq!(
        view.rev,
        raw.rev(),
        "image previews must not change the staging patch"
    );
    images.unwrap()
}

fn at(repo: &TempRepo, spec: &str) -> RevSide {
    RevSide::Commit {
        oid: repo.git(&["rev-parse", spec]).trim().into(),
    }
}

fn historical(repo: &TempRepo, path: &str, new: RevSide, old: RevSide) -> ImageDiffBytes {
    revision::revision_diff_with_images(&repo.root, path, &new, &old)
        .unwrap()
        .1
        .unwrap()
}

#[test]
fn head_index_and_worktree_are_three_distinct_images() {
    let repo = TempRepo::new("image-sides");
    let (a, b, c) = (png(1), png(2), png(3));
    repo.write("pic.png", &a);
    repo.commit_all("first");
    repo.write("pic.png", &b);
    repo.git(&["add", "pic.png"]);
    repo.write("pic.png", &c);
    for (side, old, new) in [
        (DiffSide::Unstaged, &b, &c),
        (DiffSide::Staged, &a, &b),
        (DiffSide::Combined, &a, &c),
    ] {
        let images = working(&repo, "pic.png", side);
        assert_eq!(bytes(&images.old), old);
        assert_eq!(bytes(&images.new), new);
    }
}

#[test]
fn additions_deletions_and_an_unborn_head_have_one_side() {
    let repo = TempRepo::new("image-add-delete");
    let a = png(1);
    repo.write("pic.png", &a);
    for side in [DiffSide::Unstaged, DiffSide::Combined] {
        let images = working(&repo, "pic.png", side);
        assert!(matches!(images.old, ImageBytes::Absent));
        assert_eq!(bytes(&images.new), a);
    }
    repo.git(&["add", "pic.png"]);
    let images = working(&repo, "pic.png", DiffSide::Staged);
    assert!(matches!(images.old, ImageBytes::Absent));
    assert_eq!(bytes(&images.new), a);
    repo.commit_all("root");
    let images = historical(&repo, "pic.png", at(&repo, "HEAD"), RevSide::FirstParent);
    assert!(matches!(images.old, ImageBytes::Absent));
    assert_eq!(bytes(&images.new), a);
    std::fs::remove_file(repo.root.join("pic.png")).unwrap();
    let images = working(&repo, "pic.png", DiffSide::Unstaged);
    assert_eq!(bytes(&images.old), a);
    assert!(matches!(images.new, ImageBytes::Absent));
    repo.git(&["add", "pic.png"]);
    let images = working(&repo, "pic.png", DiffSide::Staged);
    assert_eq!(bytes(&images.old), a);
    assert!(matches!(images.new, ImageBytes::Absent));
    repo.commit_all("delete");
    let images = historical(&repo, "pic.png", at(&repo, "HEAD"), RevSide::FirstParent);
    assert_eq!(bytes(&images.old), a);
    assert!(matches!(images.new, ImageBytes::Absent));
}

#[test]
fn log_uses_selected_commits_and_working_tree_comparisons() {
    let repo = TempRepo::new("image-revisions");
    let (a, b, c) = (png(1), png(2), png(3));
    repo.write("pic.png", &a);
    repo.commit_all("first");
    let first = at(&repo, "HEAD");
    repo.write("pic.png", &b);
    repo.commit_all("second");
    let second = at(&repo, "HEAD");
    repo.write("pic.png", &c);
    for old in [RevSide::FirstParent, first] {
        let images = historical(&repo, "pic.png", second.clone(), old);
        assert_eq!(bytes(&images.old), a);
        assert_eq!(bytes(&images.new), b);
    }
    let images = historical(&repo, "pic.png", RevSide::WorkingTree, second);
    assert_eq!(bytes(&images.old), b);
    assert_eq!(bytes(&images.new), c);
}

#[test]
fn image_renames_keep_the_old_blob_and_path() {
    let repo = TempRepo::new("image-renames");
    let a = png(1);
    repo.write("before.png", &a);
    repo.commit_all("first");
    std::fs::rename(repo.root.join("before.png"), repo.root.join("after.png")).unwrap();
    let images = working(&repo, "after.png", DiffSide::Unstaged);
    assert_eq!(images.old_path.as_deref(), Some("before.png"));
    assert_eq!(bytes(&images.old), a);
    assert_eq!(bytes(&images.new), a);
    repo.git(&["add", "-A"]);
    for side in [DiffSide::Staged, DiffSide::Combined] {
        let images = working(&repo, "after.png", side);
        assert_eq!(images.old_path.as_deref(), Some("before.png"));
        assert_eq!(bytes(&images.old), a);
    }
    repo.commit_all("rename");
    let images = historical(&repo, "after.png", at(&repo, "HEAD"), RevSide::FirstParent);
    assert_eq!(images.old_path.as_deref(), Some("before.png"));
    assert_eq!(bytes(&images.old), a);
    assert_eq!(bytes(&images.new), a);
}

#[test]
fn svg_and_mismatched_extensions_are_sniffed_from_original_bytes() {
    let repo = TempRepo::new("image-svg");
    let a = b"<svg xmlns=\"http://www.w3.org/2000/svg\"><rect width=\"1\"/></svg>";
    let b = b"<svg xmlns=\"http://www.w3.org/2000/svg\"><rect width=\"2\"/></svg>";
    repo.write("pic.svg", a);
    repo.commit_all("first");
    repo.write("pic.svg", b);
    let images = working(&repo, "pic.svg", DiffSide::Combined);
    assert_eq!(bytes(&images.old), a);
    assert_eq!(bytes(&images.new), b);
    repo.commit_all("second");
    let images = historical(&repo, "pic.svg", at(&repo, "HEAD"), RevSide::FirstParent);
    assert_eq!(bytes(&images.old), a);
    assert_eq!(bytes(&images.new), b);
    repo.write("actually-svg.PNG", a);
    assert_eq!(
        bytes(&working(&repo, "actually-svg.PNG", DiffSide::Unstaged).new),
        a
    );
}

#[test]
fn one_bad_or_oversized_side_does_not_hide_the_other() {
    let repo = TempRepo::new("image-refusals");
    let a = png(1);
    repo.write("pic.png", &a);
    repo.commit_all("first");
    repo.write("pic.png", b"not an image");
    let images = working(&repo, "pic.png", DiffSide::Combined);
    assert_eq!(bytes(&images.old), a);
    assert!(matches!(images.new, ImageBytes::Unavailable(_)));
    let f = std::fs::File::create(repo.root.join("pic.png")).unwrap();
    f.set_len(cide_core::image::MAX_IMAGE_BYTES + 1).unwrap();
    let images = working(&repo, "pic.png", DiffSide::Combined);
    assert_eq!(bytes(&images.old), a);
    assert!(matches!(images.new, ImageBytes::Unavailable(ref reason) if reason.contains("32 MiB")));
    repo.write("source.rs", b"fn main() {}\n");
    let handle = cide_git::repo::open(&repo.root).unwrap();
    assert!(
        image_diff::file_diff(&handle, "source.rs", DiffSide::Unstaged)
            .unwrap()
            .1
            .is_none()
    );
}
