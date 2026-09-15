//! Bridging an async Docker stream to the blocking reader and writer `cide-pty` wants. (M42)
//!
//! # Why a bridge at all
//!
//! `cide-pty` owns explicit threads rather than tokio, for the reason its own header gives:
//! `portable-pty`'s handles are blocking. Its [`cide_pty::Transport`] seam therefore hands over a
//! `Box<dyn Read + Send>` and a `Box<dyn Write + Send>`, and bollard hands back a `Stream` and an
//! `AsyncWrite`. Something has to sit between them.
//!
//! That something is a channel and a pump task, and the shape is deliberately the dumbest one
//! that works: a tokio task drains the stream into a bounded channel, and [`ChannelReader`]'s
//! `read` blocks on the receiver. **Bounded**, at the same depth `cide-pty` gives its own reader
//! queue, because that is what makes backpressure structural here as well — a full channel stops
//! the pump task, which stops polling the stream, which stops reading the socket. An unbounded
//! channel would buffer a chatty container's whole output in memory while a choked webview
//! caught up.
//!
//! # The one rule
//!
//! **Never send an empty chunk.** `Read::read` returning `Ok(0)` *is* EOF, and `cide-pty`'s
//! reader thread treats it as the child going away — it would end the session while the stream
//! was still live. [`send_chunk`] is the only sender and drops empties for that reason.

use std::io::{self, Read, Write};

use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};

/// How many chunks may be in flight before the pump task stops polling the socket.
///
/// `cide_pty::READ_QUEUE_DEPTH`'s value, and for its reason rather than by coincidence: this
/// channel is the same queue one layer out, and a deeper one here would only move the buffering
/// somewhere `CreditPolicy` cannot see it.
const QUEUE_DEPTH: usize = 16;

/// Make the pair: what the pump task sends into, and what `cide-pty` reads out of.
#[must_use]
pub fn pipe() -> (Sender<Vec<u8>>, ChannelReader) {
    let (tx, rx) = bounded(QUEUE_DEPTH);
    (
        tx,
        ChannelReader {
            rx,
            chunk: Vec::new(),
            pos: 0,
        },
    )
}

/// Send one chunk, dropping an empty one.
///
/// See the module header: an empty chunk would reach [`ChannelReader::read`] as `Ok(0)`, which is
/// EOF, and would end a live session. Returns `false` when the far end has gone, which is the
/// pump task's signal to stop.
pub fn send_chunk(tx: &Sender<Vec<u8>>, chunk: Vec<u8>) -> bool {
    if chunk.is_empty() {
        return true;
    }
    tx.send(chunk).is_ok()
}

/// A blocking [`Read`] over a channel.
pub struct ChannelReader {
    rx: Receiver<Vec<u8>>,
    chunk: Vec<u8>,
    pos: usize,
}

impl Read for ChannelReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        while self.pos >= self.chunk.len() {
            match self.rx.recv() {
                Ok(next) => {
                    self.chunk = next;
                    self.pos = 0;
                }
                // Every sender gone. This is the honest EOF, and it is what makes the session
                // end when the container's stream does.
                Err(_) => return Ok(0),
            }
        }
        let n = buf.len().min(self.chunk.len() - self.pos);
        buf[..n].copy_from_slice(&self.chunk[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// A blocking [`Write`] that hands bytes to a pump task.
///
/// Unbounded, and deliberately so — this is the *input* direction, which is a person typing.
/// Bounding it would mean a keystroke blocking the IPC thread whenever the container was slow to
/// read its stdin, and the volume is a few bytes at a time.
pub struct ChannelWriter {
    tx: Sender<Vec<u8>>,
}

impl ChannelWriter {
    #[must_use]
    pub fn new(tx: Sender<Vec<u8>>) -> Self {
        Self { tx }
    }
}

impl Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        match self.tx.send(buf.to_vec()) {
            Ok(()) => Ok(buf.len()),
            // The pump task is gone, which means the exec is over. `BrokenPipe` rather than a
            // custom error because that is what a pty writer reports for the same situation, and
            // `cide-pty`'s writer thread already knows what to do with it.
            Err(_) => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the exec has ended",
            )),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A [`Write`] that discards, for a stream that has no input direction.
///
/// # Why a log pane still has a writer
///
/// Because `cide-pty` gives every session one, and a pane whose writer *errored* would report a
/// write failure to the user the first time they pressed a key in it — `sessionSink`'s
/// `reportWriteFailure` raises a notice. Discarding is the honest behaviour for a follow: the
/// keystroke has nowhere to go, and saying so once per keypress would be noise about a pane that
/// is working exactly as intended.
pub struct DiscardWriter;

impl Write for DiscardWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Try to send without blocking, reporting whether the far end is gone.
///
/// Used by nothing yet and kept beside [`send_chunk`] because the two are the only ways bytes
/// enter this module; a future non-blocking pump wants this shape.
#[allow(dead_code)]
pub fn try_send_chunk(tx: &Sender<Vec<u8>>, chunk: Vec<u8>) -> Result<(), TrySendError<Vec<u8>>> {
    tx.try_send(chunk)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_chunk_is_dropped_rather_than_becoming_eof() {
        // The whole of this module's one rule. `Read::read` returning `Ok(0)` is EOF, and
        // `cide-pty`'s reader thread treats it as the child going away — so a stream that
        // happened to yield an empty frame would end a live session.
        let (tx, mut reader) = pipe();
        assert!(send_chunk(&tx, Vec::new()));
        assert!(send_chunk(&tx, b"hello".to_vec()));
        drop(tx);

        let mut out = Vec::new();
        reader.read_to_end(&mut out).expect("reads to EOF");
        assert_eq!(out, b"hello");
    }

    #[test]
    fn a_chunk_larger_than_the_buffer_is_handed_over_in_pieces() {
        let (tx, mut reader) = pipe();
        assert!(send_chunk(&tx, b"abcdef".to_vec()));
        drop(tx);

        let mut buf = [0u8; 4];
        assert_eq!(reader.read(&mut buf).unwrap(), 4);
        assert_eq!(&buf, b"abcd");
        assert_eq!(reader.read(&mut buf).unwrap(), 2);
        assert_eq!(&buf[..2], b"ef");
        assert_eq!(reader.read(&mut buf).unwrap(), 0, "and then EOF");
    }

    #[test]
    fn every_sender_gone_is_eof_and_nothing_else_is() {
        let (tx, mut reader) = pipe();
        let second = tx.clone();
        drop(tx);
        assert!(send_chunk(&second, b"x".to_vec()));
        let mut buf = [0u8; 8];
        assert_eq!(
            reader.read(&mut buf).unwrap(),
            1,
            "one sender left is not EOF"
        );
        drop(second);
        assert_eq!(reader.read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn a_writer_whose_far_end_is_gone_reports_a_broken_pipe() {
        // And not a custom error: a pty writer reports exactly this for the same situation, and
        // `cide-pty`'s writer thread already knows what to do with it.
        let (tx, reader) = pipe();
        drop(reader);
        let mut writer = ChannelWriter::new(tx);
        let error = writer.write(b"keystroke").expect_err("nowhere to go");
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn the_discard_writer_accepts_everything_and_never_errors() {
        // A log pane has no input direction, and a writer that errored would raise a notice on
        // every keypress in a pane that is working exactly as intended.
        let mut writer = DiscardWriter;
        assert_eq!(writer.write(b"typed into a log pane").unwrap(), 21);
        assert!(writer.flush().is_ok());
    }
}
