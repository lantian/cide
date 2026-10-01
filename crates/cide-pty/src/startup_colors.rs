//! Color queries issued before the first renderer attaches are absent from a screen
//! snapshot. Replay just those queries so xterm can answer with its actual palette.

#[derive(Default)]
pub(crate) struct StartupColors {
    parser: vte::Parser,
    queries: ColorQueries,
}

#[derive(Default)]
struct ColorQueries {
    foreground: bool,
    background: bool,
}

impl vte::Perform for ColorQueries {
    fn osc_dispatch(&mut self, params: &[&[u8]], _: bool) {
        match params {
            [b"10", b"?"] => self.foreground = true,
            [b"11", b"?"] => self.background = true,
            // OSC 10 permits querying foreground and background together.
            [b"10", b"?", b"?"] => {
                self.foreground = true;
                self.background = true;
            }
            _ => {}
        }
    }
}

impl StartupColors {
    pub(crate) fn process(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.queries, bytes);
    }

    pub(crate) fn append_to(&self, snapshot: &mut Vec<u8>) {
        if self.queries.foreground {
            snapshot.extend_from_slice(b"\x1b]10;?\x1b\\");
        }
        if self.queries.background {
            snapshot.extend_from_slice(b"\x1b]11;?\x1b\\");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retains_split_queries_without_replaying_other_osc_effects() {
        let mut parser = StartupColors::default();
        for chunk in [
            &b"\x1b]10;"[..],
            &b"?\x1b"[..],
            &b"\\\x1b]11;?\x07"[..],
            &b"\x1b]52;c;YWJj\x07\x1b]11;#ffffff\x07"[..],
        ] {
            parser.process(chunk);
        }
        let mut snapshot = b"screen".to_vec();
        parser.append_to(&mut snapshot);
        assert_eq!(snapshot, b"screen\x1b]10;?\x1b\\\x1b]11;?\x1b\\");
    }

    #[test]
    fn stacked_and_repeated_queries_have_bounded_storage() {
        let mut parser = StartupColors::default();
        for _ in 0..1000 {
            parser.process(b"\x1b]10;?;?\x07");
        }
        let mut snapshot = Vec::new();
        parser.append_to(&mut snapshot);
        assert_eq!(snapshot, b"\x1b]10;?\x1b\\\x1b]11;?\x1b\\");
    }

    #[test]
    fn ordinary_output_can_wrap_and_scroll_while_waiting_for_an_attachment() {
        let mut parser = StartupColors::default();
        parser.process("long startup output\r\n界\r\n\x1b[1;1rmore output".as_bytes());
        let mut snapshot = Vec::new();
        parser.append_to(&mut snapshot);
        assert!(snapshot.is_empty());
    }
}
