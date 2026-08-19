//! Deciding whether a file is an image, and which one. (M18)
//!
//! The sibling of [`crate::document`], and split from it for the same reason `cide-fs` is
//! split from both: `document::read` answers *"give me this file as editable text"* and
//! answers it by refusing anything with a NUL byte in its first 8 KiB. That refusal is
//! correct and is exactly what a user opening `logo.png` used to get — a permanent tab
//! reading **"logo.png looks like a binary file"** — because until this module existed cide
//! had one reader and it was the text one. (`a_png_is_refused_by_the_text_reader` below pins
//! that starting point, so the sentence in this paragraph stays a fact rather than a memory.)
//!
//! What this module does *not* do is read the pixels. It reads a header, decides what the
//! file is, and hands back identity — see [`cide_ipc::ImageDoc`], whose own header explains
//! why the bytes travel down Tauri's asset protocol instead of through the IPC.
//!
//! # Detection is by content, never by extension
//!
//! There is no extension table here, on purpose, and there is one in
//! `ui/src/panes/imageKinds.ts`. They answer different questions and putting both on one side
//! would silently merge them:
//!
//! * *Which viewer does this tab open with?* — the frontend, from the name, **before** any
//!   IPC, because a pane has to render something on its first frame.
//! * *What are these bytes?* — here, from the bytes, because that is the only question whose
//!   honest answer catches a `.png` that is really a zip file. A second extension table in
//!   Rust would let those two drift and would make the refusal below unreachable for exactly
//!   the file it exists to catch.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use cide_ipc::{ImageDoc, ImageFormat};

use crate::document;
use crate::error::{CoreError, Result};

/// The largest image cide will put on screen.
///
/// **The transport is not the constraint** — that is the whole point of using the asset
/// protocol, which streams and never marshals — so this number is not about IPC. Two other
/// costs are real and neither is visible in the file size:
///
/// * the **decoded bitmap**, which is `width × height × 4` bytes whatever the file compresses
///   to, held by the compositor for as long as the tab is open;
/// * WebKit's decoder itself, which runs in the same process as every terminal in the window,
///   so an image that makes it thrash takes the user's `claude` sessions down with it.
///
/// 32 MiB of *compressed* image is far past any screenshot, icon or asset that lives in a
/// repository, and a user who genuinely wants to look at a gigapixel scan is better served by
/// an image viewer — the same argument [`document::MAX_FILE_BYTES`] makes about a pager.
///
/// # It must not exceed [`document::MAX_FILE_BYTES`], and that is a hard tie
///
/// `cmd::file::openable` — the ctrl+click-a-path-out-of-terminal-output route — refuses
/// anything over `document::MAX_FILE_BYTES` *before a tab exists*, and it does so without
/// knowing or caring whether the path is an image. Raise this above that and a 40 MB PNG is
/// refused with the **editor's** sentence ("the editor opens files up to 32 MiB") on one route
/// and accepted on another, which is two answers to one question.
/// [`_IMAGE_CAP_FITS_UNDER_THE_EDITORS`] below is the guard, and it is a `const` assertion
/// rather than a `#[test]` on purpose — see its own comment.
pub const MAX_IMAGE_BYTES: u64 = 32 * 1024 * 1024;

/// The tie above, as a **compile-time** assertion rather than a test.
///
/// A test would only fail once somebody ran it; this fails the build, which is the right
/// strength for an invariant whose violation is invisible on screen — the file would simply be
/// refused by `openable` with the editor's wording, on one of the two routes into an image tab,
/// and nobody would connect that to a constant edited in this file.
const _IMAGE_CAP_FITS_UNDER_THE_EDITORS: () = assert!(
    MAX_IMAGE_BYTES <= document::MAX_FILE_BYTES,
    "an image cap above the editor's is unreachable through cmd::file::openable"
);

/// How much of the file is read to decide what it is.
///
/// Every magic number below lives in the first sixteen bytes; the generous window is for SVG,
/// which has no magic number at all and can legitimately open with an XML declaration, a
/// DOCTYPE and a licence comment before the `<svg` element. 8 KiB is the same window
/// [`document::looks_binary`] sniffs, deliberately — two different answers about "the start of
/// a file" measured over two different lengths is a difference nobody would think to look for.
const HEADER_BYTES: usize = 8 * 1024;

/// What a file's leading bytes say it is, or `None` for anything else.
///
/// Pure, and the reason this is a free function over a slice rather than a step inside
/// [`read`]: the interesting cases are all *near misses* — a truncated PNG signature, a RIFF
/// container that is a WAV rather than a WebP, an SVG whose `<svg` is 900 bytes in — and none
/// of them is reachable through a `Path` without writing the fixture to a disk.
pub fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
    // Ordered by how specific the signature is, so a container format cannot be claimed by a
    // shorter prefix. RIFF is the one that matters: `RIFF....WEBP` and `RIFF....WAVE` share
    // four bytes, so the check has to reach the twelfth.
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(ImageFormat::Png);
    }
    // Start of Image plus the first marker's leading byte. Three bytes rather than two,
    // because `FF D8` alone also opens a handful of unrelated formats.
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(ImageFormat::Jpeg);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(ImageFormat::Gif);
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some(ImageFormat::Webp);
    }
    // `BM` is by far the weakest signature in this list — two ASCII letters that plenty of
    // prose starts with — so the whole 14-byte BITMAPFILEHEADER is checked rather than the
    // magic alone. The first draft accepted `declared >= 26` and nothing else, and the test
    // below caught it claiming a text file beginning "BMake is a build tool": the length field
    // read the letters "ake " as 0x206B6561, which is comfortably over any threshold. A number
    // being large is not evidence; a *structure* holding together is.
    //
    // The two reserved `u16`s are zero in every writer's output, and `offset` — where the
    // pixels start — has to be at least the header it follows and no further than the end of
    // the file the same header declares. Three constraints that arbitrary text satisfies by
    // accident with probability near zero.
    if bytes.len() >= 14 && bytes.starts_with(b"BM") {
        let declared = u32::from_le_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]);
        let reserved = &bytes[6..10];
        let offset = u32::from_le_bytes([bytes[10], bytes[11], bytes[12], bytes[13]]);
        if reserved == [0, 0, 0, 0] && (26..=declared).contains(&offset) {
            return Some(ImageFormat::Bmp);
        }
    }
    // ICONDIR: reserved 0, type 1 (icon), and at least one image in the directory. Type 2 is
    // a cursor and is deliberately not accepted — WebKit does decode it, but a `.cur` is not
    // something anyone opens to look at, and every entry in this list is a format the pane
    // claims it can display.
    if bytes.len() >= 6 && bytes[0..4] == [0x00, 0x00, 0x01, 0x00] {
        let count = u16::from_le_bytes([bytes[4], bytes[5]]);
        if count > 0 {
            return Some(ImageFormat::Ico);
        }
    }
    if looks_like_svg(bytes) {
        return Some(ImageFormat::Svg);
    }
    None
}

/// Whether these bytes open an SVG document.
///
/// SVG is the only entry in [`ImageFormat`] with no signature: it is XML, so the test is
/// "text, and an `<svg` element starts somewhere in the header". Two guards keep that from
/// claiming arbitrary text:
///
/// * a NUL anywhere in the window disqualifies it outright, reusing
///   [`document::looks_binary`] so "is this text" has exactly one implementation in this
///   crate;
/// * the tag has to be `<svg` followed by a delimiter, so `<svgAnnotations>` in a Java file
///   is not an image.
///
/// An HTML page containing an inline `<svg>` therefore *does* sniff as SVG. That is accepted
/// rather than worked around: it is reachable only when the user opened a file whose name
/// already said `.svg` (the frontend's extension rule is what routes a tab here at all), so
/// the file is lying about itself in two places at once and rendering it in an `<img>` — where
/// it will simply fail to decode and say so — is the honest outcome.
fn looks_like_svg(bytes: &[u8]) -> bool {
    if bytes.is_empty() || document::looks_binary(bytes) {
        return false;
    }
    let window = &bytes[..bytes.len().min(HEADER_BYTES)];
    window.windows(4).enumerate().any(|(at, w)| {
        w.eq_ignore_ascii_case(b"<svg")
            && window
                .get(at + 4)
                .is_none_or(|c| c.is_ascii_whitespace() || *c == b'>' || *c == b'/')
    })
}

/// Read a file as an image document: identity, size and stamp. Never the pixels.
///
/// Four refusals, each with a sentence the pane shows verbatim, because "a blank pane" is the
/// failure mode this whole feature was written to avoid:
///
/// | refusal | what it catches |
/// | --- | --- |
/// | not a regular file | a directory, a FIFO, a device node — the same liveness guard `openable` makes |
/// | empty | a zero-byte placeholder, which sniffs as nothing and would otherwise read "not an image" |
/// | over [`MAX_IMAGE_BYTES`] | the gigapixel scan |
/// | [`sniff`] answered `None` | **a file that claims to be a PNG and is not** |
///
/// The path is canonicalised first, and the canonical form is what comes back — see
/// [`ImageDoc::path`] for why the asset URL has to be built from it rather than from the
/// symlink the user clicked.
pub fn read(path: &Path) -> Result<ImageDoc> {
    let shown = path.display().to_string();

    // Before the `stat`, so a broken symlink reports "no such file" rather than a metadata
    // error about a path that does resolve textually. Same order as `cmd::file::openable`.
    let real = std::fs::canonicalize(path)
        .map_err(|e| CoreError::Io(format!("{shown} could not be opened: {e}")))?;
    let meta = std::fs::metadata(&real)
        .map_err(|e| CoreError::Io(format!("{shown} could not be opened: {e}")))?;

    if !meta.is_file() {
        return Err(CoreError::Io(format!(
            "{shown} is not a regular file, so there is nothing to display"
        )));
    }
    if meta.len() == 0 {
        return Err(CoreError::Io(format!("{shown} is empty")));
    }
    if meta.len() > MAX_IMAGE_BYTES {
        return Err(CoreError::Io(format!(
            "{shown} is {} MiB; cide displays images up to {} MiB",
            meta.len() / (1024 * 1024),
            MAX_IMAGE_BYTES / (1024 * 1024)
        )));
    }

    // The header only. This is the one place the file's contents are touched at all, and
    // reading 8 KiB of a 30 MB PNG is the difference between this command costing a `stat`
    // and it costing the whole file twice over — once here and once again down the asset
    // protocol when the `<img>` fetches it.
    //
    // `take(…).read_to_end(…)` and **not** a single `Read::read` into a sized buffer, which is
    // what this was first written as. `read` is permitted to return fewer bytes than asked for
    // without being at EOF, and a network filesystem is where that stops being theoretical —
    // this crate already reasons about a stalled NFS mount two functions away. A short read of
    // four bytes leaves a real PNG sniffing as `None`, which surfaces as the *"has a .png name
    // but its contents are not…"* refusal: cide would call a perfectly good file a liar, on a
    // mount, intermittently, which is the worst shape a bug of this kind can take.
    let mut header = Vec::with_capacity(HEADER_BYTES.min(meta.len() as usize));
    File::open(&real)
        .and_then(|f| f.take(HEADER_BYTES as u64).read_to_end(&mut header))
        .map_err(|e| CoreError::Io(format!("{shown} could not be read: {e}")))?;

    let Some(format) = sniff(&header) else {
        return Err(CoreError::Io(not_an_image(path)));
    };

    Ok(ImageDoc {
        path: real,
        format,
        bytes: meta.len(),
        stamp: document::stamp_of(&meta),
    })
}

/// The sentence for a file whose name promised an image and whose bytes did not deliver one.
///
/// Names the extension the file actually carries, taken from the path verbatim — there is no
/// table here and there must not be one, per this module's header. Saying *".png name"* rather
/// than *"is not a PNG"* is deliberate: cide never claimed the file was a PNG, the **name**
/// did, and a message that says so points the user at the file instead of at cide.
///
/// A file with no extension at all still reaches this — a tab can be opened on any path — so
/// the wording falls back rather than printing an empty pair of quotes.
fn not_an_image(path: &Path) -> String {
    let shown = path.display();
    let known = "PNG, JPEG, GIF, WebP, BMP, ICO or SVG";
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!(
            "{shown} has a .{ext} name, but its contents are not {known} data. Nothing was \
             decoded, so there is nothing to show."
        ),
        None => format!("{shown} does not contain {known} data, so there is nothing to show."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cide-image-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// The smallest real PNG: signature, IHDR, one IDAT, IEND. Only the signature matters to
    /// [`sniff`], but a fixture that is a *valid* file keeps the test honest about what it is
    /// standing in for.
    const PNG: &[u8] = &[
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, b'I', b'H', b'D',
        b'R', 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89,
    ];

    /// **The starting point this module exists to change.**
    ///
    /// Before it, cide had exactly one file reader and it was the text one, so opening any
    /// raster image produced a tab reading "… looks like a binary file". If this test ever
    /// starts failing because `document::read` learned to accept a PNG, the split described in
    /// this module's header has been undone somewhere and both readers now have an opinion
    /// about images.
    #[test]
    fn a_png_is_refused_by_the_text_reader() {
        let path = tempdir().join("refused.png");
        fs::write(&path, PNG).expect("write");

        let refusal = document::read(&path).expect_err("the text reader must refuse a PNG");
        assert!(
            refusal.to_string().contains("binary"),
            "the pre-M18 sentence was about binary contents: {refusal}"
        );
    }

    /// Every format in the list, sniffed from its own header, and nothing else claimed.
    #[test]
    fn each_format_is_recognised_by_its_own_bytes() {
        assert_eq!(sniff(PNG), Some(ImageFormat::Png));
        assert_eq!(
            sniff(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F']),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(sniff(b"GIF89a\x01\x00\x01\x00"), Some(ImageFormat::Gif));
        assert_eq!(sniff(b"GIF87a\x01\x00\x01\x00"), Some(ImageFormat::Gif));
        assert_eq!(
            sniff(b"RIFF\x24\x00\x00\x00WEBPVP8 "),
            Some(ImageFormat::Webp)
        );
        // A 70-byte BMP whose pixels start at offset 54: the ordinary 14-byte file header
        // plus a 40-byte BITMAPINFOHEADER.
        assert_eq!(
            sniff(&[
                b'B', b'M', 0x46, 0x00, 0x00, 0x00, 0, 0, 0, 0, 0x36, 0x00, 0x00, 0x00
            ]),
            Some(ImageFormat::Bmp)
        );
        assert_eq!(
            sniff(&[0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x10, 0x10]),
            Some(ImageFormat::Ico)
        );
    }

    /// The near misses. Each of these shares a prefix with something in the list, and every
    /// one of them used to be the reason a stricter test than "starts with four bytes" was
    /// written.
    #[test]
    fn near_misses_are_not_images() {
        // A WAV is a RIFF container too. Four bytes of agreement, twelve of disagreement.
        assert_eq!(sniff(b"RIFF\x24\x00\x00\x00WAVEfmt "), None);
        // A truncated PNG signature.
        assert_eq!(sniff(b"\x89PNG\r\n"), None);
        // A cursor, not an icon: ICONDIR type 2.
        assert_eq!(sniff(&[0x00, 0x00, 0x02, 0x00, 0x01, 0x00]), None);
        // An ICONDIR declaring no images.
        assert_eq!(sniff(&[0x00, 0x00, 0x01, 0x00, 0x00, 0x00]), None);
        // "BM" is two letters, and plenty of text starts with them. This exact string is why
        // the BMP arm checks the whole file header: the four bytes at offset 2 spell "ake ",
        // which reads as a declared length of 0x206B6561 and passes any size threshold.
        assert_eq!(sniff(b"BMake is a build tool\n\n\n\n\n\n"), None);
        // A BMP header with junk in its reserved words, and one whose pixel offset points
        // before the header it follows.
        assert_eq!(
            sniff(&[b'B', b'M', 0x46, 0, 0, 0, 1, 0, 0, 0, 0x36, 0, 0, 0]),
            None
        );
        assert_eq!(
            sniff(&[b'B', b'M', 0x46, 0, 0, 0, 0, 0, 0, 0, 0x04, 0, 0, 0]),
            None
        );
        // Ordinary source. This is the case that must stay `None` or every text file in the
        // project becomes eligible for the image pane.
        assert_eq!(sniff(b"fn main() { println!(\"hi\"); }\n"), None);
        assert_eq!(sniff(b""), None);
    }

    /// SVG has no magic number, so its rule gets its own test.
    #[test]
    fn svg_is_recognised_through_a_preamble_and_not_through_a_prefix() {
        assert_eq!(
            sniff(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
            Some(ImageFormat::Svg)
        );
        assert_eq!(sniff(b"<SVG WIDTH=\"4\"></SVG>"), Some(ImageFormat::Svg));

        // The ordinary real-world shape: declaration, DOCTYPE, a comment, then the element.
        let preamble = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
            <!-- Generated by a drawing program that likes to introduce itself at length -->\n\
            <svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\"></svg>\n";
        assert_eq!(sniff(preamble), Some(ImageFormat::Svg));

        // A Java identifier is not an element.
        assert_eq!(sniff(b"class Foo { List<svgAnnotation> xs; }\n"), None);
        // Binary that happens to contain the letters.
        assert_eq!(sniff(b"\x00\x00<svg />"), None);
    }

    #[test]
    fn a_real_png_reads_as_a_png_document() {
        let path = tempdir().join("logo.png");
        fs::write(&path, PNG).expect("write");

        let doc = read(&path).expect("reads");
        assert_eq!(doc.format, ImageFormat::Png);
        assert_eq!(doc.bytes, PNG.len() as u64);
        // Canonical, because that is what the asset URL is built from.
        assert_eq!(doc.path, fs::canonicalize(&path).expect("canonicalise"));
        assert!(doc.stamp.is_some(), "an ordinary tmpfs file has an mtime");
    }

    /// The headline refusal: the sentence a user gets instead of a blank pane.
    #[test]
    fn a_file_that_claims_to_be_a_png_and_is_not_fails_with_a_sentence() {
        let path = tempdir().join("liar.png");
        fs::write(&path, b"PK\x03\x04this is a zip file wearing a png name").expect("write");

        let refusal = read(&path).expect_err("must refuse").to_string();
        assert!(refusal.contains("liar.png"), "names the file: {refusal}");
        assert!(refusal.contains(".png name"), "names the claim: {refusal}");
        assert!(
            refusal.contains("PNG, JPEG"),
            "names what is accepted: {refusal}"
        );
    }

    /// A real image is bigger than the sniff window, and the read has to cope with that.
    ///
    /// Not a tautology: this is the path where `File::read` into a sized buffer could return a
    /// short count without being at EOF, which would leave a perfectly good PNG sniffing as
    /// `None` and being called a liar. Twenty kibibytes is over [`HEADER_BYTES`], so the read
    /// is truncated by the `take` rather than by the file's own length.
    #[test]
    fn a_file_larger_than_the_sniff_window_still_reads() {
        let path = tempdir().join("big.png");
        let mut bytes = PNG.to_vec();
        bytes.resize(20 * 1024, 0);
        fs::write(&path, &bytes).expect("write");

        let doc = read(&path).expect("reads");
        assert_eq!(doc.format, ImageFormat::Png);
        assert_eq!(doc.bytes, bytes.len() as u64);
    }

    /// The cap's sentence, which is the one a user gets for the gigapixel scan.
    ///
    /// `set_len` rather than writing 32 MiB: the refusal is decided by `metadata().len()` and
    /// happens *before* a single byte is read, which is the whole point of checking it there.
    #[test]
    fn a_file_over_the_cap_is_refused_before_it_is_read() {
        let path = tempdir().join("huge.png");
        let f = fs::File::create(&path).expect("create");
        f.set_len(MAX_IMAGE_BYTES + 1).expect("set_len");
        drop(f);

        let refusal = read(&path).expect_err("must refuse").to_string();
        assert!(refusal.contains("huge.png"), "names the file: {refusal}");
        assert!(
            refusal.contains("32 MiB"),
            "names the limit in the same units the editor's refusal uses: {refusal}"
        );
    }

    #[test]
    fn an_empty_file_and_a_directory_each_get_their_own_sentence() {
        let empty = tempdir().join("empty.png");
        fs::write(&empty, b"").expect("write");
        assert!(
            read(&empty)
                .expect_err("refuses")
                .to_string()
                .contains("is empty"),
            "a zero-byte file is empty, not 'not an image'"
        );

        let dir = tempdir().join("adirectory.png");
        fs::create_dir_all(&dir).expect("mkdir");
        assert!(
            read(&dir)
                .expect_err("refuses")
                .to_string()
                .contains("not a regular file")
        );
    }
}
