//! A length-prefixed envelope for the two commands that move a file's **bytes**. (M63)
//!
//! `u32 LE n | n bytes of JSON head | payload`. The head is a small DTO this crate already
//! spells ([`crate::FileBytesHead`] on the way out, [`crate::FileBytesWrite`] on the way in);
//! the payload is the file, verbatim.
//!
//! # Why an envelope exists at all
//!
//! [`crate::image`] states the rule for bytes and the IPC: never a JSON array of decimal
//! numbers, never base64 — both put a whole PNG through `serde_json` and the JS engine on the
//! thread that draws every terminal in the window. What Tauri offers instead is a **raw body**
//! in each direction: `tauri::ipc::Response::new(Vec<u8>)` travels as `application/octet-stream`
//! (`cmd::diag::diag_echo_bytes`, `session_attach`), and `invoke(cmd, Uint8Array)` arrives as
//! `tauri::ipc::InvokeBody::Raw`. Neither carries anything *beside* the bytes: a `Response` has
//! no headers, and a request's headers are `HeaderValue`s — visible ASCII only, so a file name
//! with a Cyrillic letter in it would need percent-encoding on one side and a decoder on the
//! other, two more places to be wrong for one string. The drawing pane needs a stamp beside
//! the bytes it reads (the autosave precondition) and a path beside the bytes it writes, and
//! it needs them from the **same** `stat` as the bytes, or a change landing between two calls
//! is a spurious conflict. So the two ride together, in one frame, framed the same way in both
//! directions, and `ui/src/ipc/client.ts` carries the four-line mirror of this module.
//!
//! Little-endian because that is what `DataView.getUint32(offset, true)` reads without a
//! byte-swap, and a wire format that needs one on the JavaScript side is one that gets it
//! wrong silently on the first big-endian host nobody tests on.

use std::fmt;

/// The length prefix, in bytes.
pub const PREFIX: usize = 4;

/// Why a frame could not be read — or, for [`pack`], written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    /// Fewer bytes than the prefix, or than the prefix promised. Carries both numbers so the
    /// sentence a user sees names the shape of the failure rather than "invalid".
    Truncated { have: usize, need: usize },
    /// The head is not UTF-8, so it cannot be JSON.
    HeadNotUtf8,
    /// A head longer than a `u32` can prefix. Unreachable for the DTOs this crate frames,
    /// and stated rather than `expect`ed because the workspace aborts on panic.
    HeadTooLong(usize),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { have, need } => write!(
                f,
                "the frame is truncated: {have} bytes where at least {need} were promised"
            ),
            Self::HeadNotUtf8 => f.write_str("the frame head is not UTF-8"),
            Self::HeadTooLong(len) => write!(f, "the frame head is {len} bytes, over the limit"),
        }
    }
}

impl std::error::Error for FrameError {}

/// Frame a head and a payload.
pub fn pack(head: &str, payload: &[u8]) -> Result<Vec<u8>, FrameError> {
    let len = u32::try_from(head.len()).map_err(|_| FrameError::HeadTooLong(head.len()))?;
    let mut out = Vec::with_capacity(PREFIX + head.len() + payload.len());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(head.as_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

/// Split a frame back into its head and its payload, borrowing both.
pub fn unpack(frame: &[u8]) -> Result<(&str, &[u8]), FrameError> {
    let prefix: [u8; PREFIX] =
        frame
            .get(..PREFIX)
            .and_then(|p| p.try_into().ok())
            .ok_or(FrameError::Truncated {
                have: frame.len(),
                need: PREFIX,
            })?;
    let len = u32::from_le_bytes(prefix) as usize;
    let end = PREFIX + len;
    let head = frame.get(PREFIX..end).ok_or(FrameError::Truncated {
        have: frame.len(),
        need: end,
    })?;
    let head = std::str::from_utf8(head).map_err(|_| FrameError::HeadNotUtf8)?;
    Ok((head, &frame[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_round_trips_a_head_and_a_payload_with_nuls_in_it() {
        let payload = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
        let frame = pack(r#"{"writable":true,"stamp":null}"#, payload).expect("packs");
        let (head, body) = unpack(&frame).expect("unpacks");
        assert_eq!(head, r#"{"writable":true,"stamp":null}"#);
        assert_eq!(body, payload);
    }

    #[test]
    fn the_prefix_is_little_endian_and_four_bytes() {
        // What `DataView.getUint32(0, true)` reads on the other side. Pinned as bytes because
        // the JavaScript mirror in `ui/src/ipc/client.ts` cannot import this module.
        let frame = pack("abc", b"").expect("packs");
        assert_eq!(&frame[..PREFIX], &[3, 0, 0, 0]);
        assert_eq!(&frame[PREFIX..], b"abc");
    }

    #[test]
    fn an_empty_payload_is_a_frame_too() {
        // A new `.excalidraw` from the tree's *New file* is zero bytes, and the read must
        // answer "empty", not "truncated".
        let frame = pack("{}", b"").expect("packs");
        let (head, body) = unpack(&frame).expect("unpacks");
        assert_eq!(head, "{}");
        assert!(body.is_empty());
    }

    #[test]
    fn a_short_frame_is_refused_by_name() {
        assert_eq!(
            unpack(&[1, 0]),
            Err(FrameError::Truncated { have: 2, need: 4 })
        );
        // The prefix promises ten bytes of head and the frame carries three.
        assert_eq!(
            unpack(&[10, 0, 0, 0, b'a', b'b', b'c']),
            Err(FrameError::Truncated { have: 7, need: 14 })
        );
    }

    #[test]
    fn a_head_that_is_not_utf8_is_refused() {
        let mut frame = pack("xx", b"payload").expect("packs");
        frame[PREFIX] = 0xff;
        assert_eq!(unpack(&frame), Err(FrameError::HeadNotUtf8));
    }

    #[test]
    fn a_cyrillic_path_survives_the_head_without_any_encoding() {
        // The whole reason the path is in the head and not in an HTTP header.
        let head = r#"{"path":"/home/иван/схема.excalidraw","ifUnchanged":null}"#;
        let frame = pack(head, b"{}").expect("packs");
        assert_eq!(unpack(&frame).expect("unpacks").0, head);
    }
}
