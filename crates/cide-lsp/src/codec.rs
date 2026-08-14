//! `Content-Length: N\r\n\r\n` + N bytes, in both directions.
//!
//! Generic over [`std::io::BufRead`] and [`std::io::Write`] rather than over a `ChildStdin` /
//! `ChildStdout`, and that is the whole reason this module is separate: it makes every framing
//! test drivable from an in-memory pair with no process, no thread and no timing. The failures
//! below are the ones that only show up against a real server, at which point they look like the
//! server hanging.

use std::io::{BufRead, Write};

use serde_json::Value;

/// The largest frame this client will allocate for.
///
/// Not politeness — a guard. `Content-Length` is a number from another process, and
/// `Vec::with_capacity` on a corrupt or hostile one is an instant out-of-memory abort of the whole
/// application, not of the language server. 64 MiB is far past any real message: rust-analyzer's
/// largest are semantic-token payloads for a huge file, in the low megabytes.
pub const MAX_FRAME_BYTES: usize = 64 << 20;

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("the language server closed its output")]
    Eof,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("a header line was not utf-8")]
    Header,
    #[error("no Content-Length in a message header")]
    MissingLength,
    #[error("Content-Length {0} is beyond the {MAX_FRAME_BYTES}-byte cap")]
    FrameTooLarge(u64),
    #[error("body was not json: {0}")]
    Json(#[from] serde_json::Error),
}

/// Write one message, header and all.
pub fn write_message<W: Write>(out: &mut W, value: &Value) -> Result<(), CodecError> {
    let body = serde_json::to_vec(value)?;
    // One `write_all` for the header and one for the body, then a single flush. Writing them
    // separately without the flush leaves a header in the pipe buffer with no body behind it,
    // which reads at the other end as a server that has gone quiet.
    write!(out, "Content-Length: {}\r\n\r\n", body.len())?;
    out.write_all(&body)?;
    out.flush()?;
    Ok(())
}

/// Read one message, or fail.
///
/// Blocking. Returns [`CodecError::Eof`] when the stream ends cleanly at a message boundary,
/// which is what a server exiting normally looks like and is not an error worth logging as one.
pub fn read_message<R: BufRead>(input: &mut R) -> Result<Value, CodecError> {
    let mut length: Option<u64> = None;
    let mut line = Vec::new();

    loop {
        line.clear();
        // `read_until(b'\n')` rather than `read_line`: a header is ASCII by spec, but a corrupt
        // stream is not, and `read_line` would fail the whole read on invalid UTF-8 in a header
        // we are about to reject anyway.
        let read = input.read_until(b'\n', &mut line)?;
        if read == 0 {
            return Err(CodecError::Eof);
        }
        let text = std::str::from_utf8(&line).map_err(|_| CodecError::Header)?;
        let text = text.trim_end_matches(['\r', '\n']);

        // The blank line that ends the header block.
        if text.is_empty() {
            break;
        }
        // Case-insensitive: the spec says `Content-Length`, and a header name is
        // case-insensitive by HTTP convention. Matching exactly would work against every server
        // that exists and fail against the first one that does not.
        if let Some((name, value)) = text.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse::<u64>().ok();
        }
        // Every other header — `Content-Type` — is read and dropped. Rejecting unknown headers
        // would make this client fail on a future spec revision for no benefit.
    }

    let length = length.ok_or(CodecError::MissingLength)?;
    if length > MAX_FRAME_BYTES as u64 {
        return Err(CodecError::FrameTooLarge(length));
    }

    let mut body = vec![0u8; length as usize];
    // `read_exact`, so a header split across two reads — or a body arriving in pieces, which is
    // the normal case on a pipe — is assembled rather than truncated. This is the single most
    // common way a hand-rolled LSP client goes wrong, and it shows up as intermittent JSON parse
    // errors under load rather than as anything that names the framing.
    input.read_exact(&mut body)?;
    Ok(serde_json::from_slice(&body)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn framed(body: &str) -> Vec<u8> {
        format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
    }

    #[test]
    fn a_message_round_trips() {
        let mut buf = Vec::new();
        let value = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"});
        write_message(&mut buf, &value).expect("write");
        let mut cursor = std::io::Cursor::new(buf);
        assert_eq!(read_message(&mut cursor).expect("read"), value);
    }

    #[test]
    fn two_messages_back_to_back_are_read_separately() {
        // The reader must consume exactly `Content-Length` bytes and leave the rest. Reading "the
        // rest of the buffer" works for one message and silently corrupts every stream after.
        let mut buf = Vec::new();
        write_message(&mut buf, &json!({"id": 1})).expect("write");
        write_message(&mut buf, &json!({"id": 2})).expect("write");
        let mut cursor = std::io::Cursor::new(buf);
        assert_eq!(read_message(&mut cursor).expect("first"), json!({"id": 1}));
        assert_eq!(read_message(&mut cursor).expect("second"), json!({"id": 2}));
    }

    #[test]
    fn a_header_split_across_reads_still_frames() {
        // A pipe delivers whatever the kernel had. `BufReader` over a reader that hands back one
        // byte at a time is the same situation, deterministically.
        struct Dribble(std::io::Cursor<Vec<u8>>);
        impl std::io::Read for Dribble {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if buf.is_empty() {
                    return Ok(0);
                }
                self.0.read(&mut buf[..1])
            }
        }
        let bytes = framed(r#"{"id":7}"#);
        let mut input = std::io::BufReader::new(Dribble(std::io::Cursor::new(bytes)));
        assert_eq!(read_message(&mut input).expect("read"), json!({"id": 7}));
    }

    #[test]
    fn an_extra_header_is_ignored_rather_than_rejected() {
        let body = r#"{"id":1}"#;
        let bytes = format!(
            "Content-Length: {}\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{body}",
            body.len()
        );
        let mut cursor = std::io::Cursor::new(bytes.into_bytes());
        assert_eq!(read_message(&mut cursor).expect("read"), json!({"id": 1}));
    }

    #[test]
    fn the_header_name_is_matched_case_insensitively() {
        let body = r#"{"id":1}"#;
        let bytes = format!("content-length: {}\r\n\r\n{body}", body.len());
        let mut cursor = std::io::Cursor::new(bytes.into_bytes());
        assert_eq!(read_message(&mut cursor).expect("read"), json!({"id": 1}));
    }

    #[test]
    fn a_frame_larger_than_the_cap_is_refused_rather_than_allocated() {
        // Without this the `Vec::with_capacity` below is a 1 TB allocation and an abort of the
        // *application*, from one bad number written by a child process.
        let mut cursor = std::io::Cursor::new(b"Content-Length: 999999999999\r\n\r\n".to_vec());
        assert!(matches!(
            read_message(&mut cursor),
            Err(CodecError::FrameTooLarge(999_999_999_999))
        ));
    }

    #[test]
    fn a_clean_end_of_stream_is_eof_and_not_an_io_error() {
        // A server exiting normally ends here, and it must be distinguishable from a broken pipe
        // so the supervisor does not report a crash for an orderly shutdown.
        let mut cursor = std::io::Cursor::new(Vec::new());
        assert!(matches!(read_message(&mut cursor), Err(CodecError::Eof)));
    }

    #[test]
    fn a_header_block_with_no_content_length_is_rejected() {
        let mut cursor = std::io::Cursor::new(b"Content-Type: text/plain\r\n\r\n".to_vec());
        assert!(matches!(
            read_message(&mut cursor),
            Err(CodecError::MissingLength)
        ));
    }

    #[test]
    fn a_body_that_is_not_json_names_itself_as_such() {
        let mut cursor = std::io::Cursor::new(framed("not json at all"));
        assert!(matches!(
            read_message(&mut cursor),
            Err(CodecError::Json(_))
        ));
    }

    #[test]
    fn a_multibyte_body_is_measured_in_bytes_and_not_characters() {
        // `Content-Length` is bytes. Writing `body.chars().count()` produces a frame that is
        // short by one per non-ASCII character, and every message after it is misaligned — which
        // surfaces as a parse error on a *later*, innocent message.
        let value = json!({"message": "mismatched types — expected `u32`, found `日本`"});
        let mut buf = Vec::new();
        write_message(&mut buf, &value).expect("write");
        let mut cursor = std::io::Cursor::new(buf);
        assert_eq!(read_message(&mut cursor).expect("read"), value);
    }
}
