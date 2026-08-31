//! Structured (JSON) log lines, rendered for a person.
//!
//! # The stream this exists for
//!
//! A shell pane that runs a service prints its log as one JSON object per line — logrus, zap,
//! pino, bunyan, structlog, Serilog, ECS, every one of them different in its key names and
//! identical in being unreadable at a glance. What a person wants out of such a line is four
//! things in a fixed place: when, how bad, what happened, and the fields. This module is that
//! rewrite, and nothing else: it is pure, it allocates a `String` per rendered line, and it
//! runs on `cide-pty`'s coalescer thread through `cide_pty::LineRender`, so every byte of
//! every shell session pays for whatever is written here.
//!
//! # Why the detector is the careful half
//!
//! A false positive is not a cosmetic bug — the rendering happens *upstream of the vt100
//! mirror*, so a line this module claims is destroyed: it is not in the scrollback, not in
//! the reattach snapshot, and not recoverable by scrolling back. `jq -c` output, a minified
//! `package.json`, a Go `map[…]` printed with braces and an API response echoed by `curl` all
//! look like a JSON object and are *data a person is reading*, not a log. So the gate is
//! three-fold and deliberately conservative: shaped like an object, parses as one, and
//! carries at least **two** of the three roles a log line has (a time, a level, a message).
//! One role is not enough — `{"message":"ok"}` is a REST response as often as it is a log.
//!
//! Everything else returns [`None`] and is kept byte for byte. See `cide_pty::Rendered::Keep`,
//! which exists for this: a line passed back as its own text would be re-terminated `\r\n`,
//! which corrupts a raw-mode TUI.

use serde_json::{Map, Value};

/// SGR only, and only from the eight-colour + dim/bold set.
///
/// `ui/src/settings/theme.ts`'s `TERMINAL_SLOTS` maps exactly these to theme tokens, so they
/// follow light and dark without this module knowing a thing about either. A 24-bit literal
/// would not: xterm is constructed with `minimumContrastRatio: 3`, which silently repaints
/// any foreground that lands too close to its cell background — the colour would be *almost*
/// what was asked for, differently on each theme, with nothing to grep for.
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const BLUE: &str = "\x1b[34m";
const CYAN: &str = "\x1b[36m";
const RESET: &str = "\x1b[0m";

/// Past this a line is not a log line anybody is reading — it is a payload that happens to be
/// JSON, and parsing it per line on the coalescer thread is a cost with no reader.
const MAX_LINE: usize = 64 * 1024;

/// Keys that carry the event's time, most specific first.
const TIME_KEYS: &[&str] = &["ts", "time", "timestamp", "@timestamp", "@t", "at", "t"];

/// Keys that carry its severity. `log.level` is ECS, which spells nesting with a dot in the
/// key itself rather than with an object.
const LEVEL_KEYS: &[&str] = &["level", "lvl", "severity", "levelname", "log.level", "@l"];

/// Keys that carry the human sentence. `event` is structlog's, `@mt` Serilog's message
/// template, `short_message` GELF's.
const MSG_KEYS: &[&str] = &["msg", "message", "event", "@mt", "short_message"];

/// Keys whose value is a stack or a multi-line error — the one thing worth spending several
/// lines on, because a stack folded into `key=value` is a stack nobody can read.
const ERROR_KEYS: &[&str] = &["error", "err", "exception", "stack", "stacktrace", "trace"];

/// Fields that are true of every line of a run and so tell a reader nothing: bunyan's schema
/// version, the pid and host every line shares.
const NOISE_KEYS: &[&str] = &["v", "pid", "hostname"];

/// Could these bytes still become a line [`render`] would rewrite?
///
/// `cide_pty::LineRender::holding_only`'s predicate, and the reason a shell prompt is not
/// swallowed: the splitter holds an unterminated tail only while this says yes, and a prompt
/// does not open with a brace. Deliberately cheaper and looser than [`render`]'s gate — it
/// answers about a fragment, so it may only test what a fragment already shows.
pub fn could_start(partial: &str) -> bool {
    partial.len() <= MAX_LINE && partial.trim_start().starts_with('{')
}

/// Render one line, or [`None`] if it is not a structured log line.
///
/// [`None`] means *keep the original bytes*, never *drop the line*: this stream is also where
/// a program's own prose, its errors and its prompts arrive.
pub fn render(line: &str) -> Option<String> {
    render_with(line, None)
}

/// As [`render`], with the time-and-level prefix marked as an OSC 8 hyperlink to `uri`.
///
/// # Why only the prefix, and why a hyperlink at all
///
/// The rendering is a summary — a nested object arrives as compact JSON and a wide event
/// wraps — so there has to be a way back to the whole thing, and the whole thing is *gone*
/// downstream: the rewrite happens above the screen mirror. So the app keeps the raw line and
/// this marks the rendering with the handle that finds it again. OSC 8 rather than anything
/// cide invented, because xterm already parses it, the marker occupies no cells, it survives
/// wrapping, and a copied line does not contain it.
///
/// **The prefix only, never the whole line.** xterm resolves overlapping links by discarding
/// the later provider's (`_removeIntersectingLinks`), and its OSC-link provider is registered
/// before ours — so a hyperlink over the whole line would silently delete every file-path link
/// inside it, and `caller=srv/main.go:42` is one of the most useful things in a log line. The
/// timestamp and the level are the one span no path is ever found in.
pub fn render_linked(line: &str, uri: &str) -> Option<String> {
    render_with(line, Some(uri))
}

/// Open an OSC 8 hyperlink. Terminated with ST (`ESC \`) rather than BEL, which is the form
/// that survives being read back out of a terminal's own state.
fn osc8(uri: &str) -> String {
    format!("\x1b]8;;{uri}\x1b\\")
}

/// Close one. The empty URI is what ends a hyperlink; there is no separate terminator.
const OSC8_END: &str = "\x1b]8;;\x1b\\";

fn render_with(line: &str, link: Option<&str>) -> Option<String> {
    let trimmed = line.trim();
    // An escape sequence means the writer is already painting — a coloured logger, a progress
    // bar, a TUI frame. Re-rendering it would fight whatever it is doing, and the braces are
    // more likely to be part of a drawn picture than a document.
    if trimmed.len() > MAX_LINE
        || line.contains('\x1b')
        || !trimmed.starts_with('{')
        || !trimmed.ends_with('}')
    {
        return None;
    }
    let fields: Map<String, Value> = serde_json::from_str(trimmed).ok()?;

    let time = take(&fields, TIME_KEYS);
    let level = take(&fields, LEVEL_KEYS);
    let message = take(&fields, MSG_KEYS);
    // Two of the three. One alone is as likely to be somebody's data as somebody's log, and
    // being wrong here deletes a line that was never ours. See the module header.
    let roles = [&time, &level, &message]
        .iter()
        .filter(|f| f.is_some())
        .count();
    if roles < 2 {
        return None;
    }

    let mut out = String::with_capacity(trimmed.len());
    if let Some((_, value)) = &time
        && let Some(clock) = clock(value)
    {
        out.push_str(DIM);
        out.push_str(&clock);
        out.push_str(RESET);
        out.push(' ');
    }
    if let Some((_, value)) = &level {
        let (text, colour) = severity(value);
        out.push_str(colour);
        // Padded so the messages of consecutive lines start in one column — the whole reason
        // a log is skimmable at all.
        out.push_str(&format!("{text:<5}"));
        out.push_str(RESET);
        out.push(' ');
    }
    // The prefix ends here, and an empty one gets no link: a timestamp this module could not
    // read and no level at all leaves nothing to click, and a zero-width hyperlink is a link
    // the user can never find.
    if let Some(uri) = link
        && !out.is_empty()
    {
        // Closed *inside* the separating space rather than around it, so the underline xterm
        // draws on hover ends with the level word instead of trailing a blank column into the
        // message.
        let trailing = out.ends_with(' ');
        if trailing {
            out.pop();
        }
        out.insert_str(0, &osc8(uri));
        out.push_str(OSC8_END);
        if trailing {
            out.push(' ');
        }
    }
    if let Some((_, value)) = &message {
        out.push_str(&plain(value));
    }

    // Whatever is left, in the order the writer put it in: a logger that puts the request id
    // first meant it to be read first, and sorting would throw that away.
    let claimed: Vec<&str> = [&time, &level, &message]
        .iter()
        .filter_map(|f| f.as_ref().map(|(key, _)| *key))
        .collect();
    let mut errors: Vec<(&str, &Value)> = Vec::new();
    for (key, value) in &fields {
        if claimed.contains(&key.as_str()) || NOISE_KEYS.contains(&key.as_str()) {
            continue;
        }
        if ERROR_KEYS.contains(&key.as_str()) && multiline(value) {
            errors.push((key, value));
            continue;
        }
        out.push_str("  ");
        out.push_str(DIM);
        out.push_str(key);
        out.push('=');
        out.push_str(RESET);
        out.push_str(&value_text(value));
    }

    // A stack last and on its own lines. `Rendered::Replace` may span lines, and this is what
    // that is for: the alternative is a 40-frame trace as one `error=…` cell off the right
    // edge of the pane, which is the same as not printing it.
    for (key, value) in errors {
        out.push('\n');
        out.push_str(RED);
        out.push_str(key);
        out.push(':');
        out.push_str(RESET);
        for frame in plain(value).lines() {
            out.push_str("\n    ");
            out.push_str(frame.trim_end());
        }
    }
    Some(out)
}

/// The first of `keys` this object has, with the key as it was written.
///
/// A `null` counts as **absent**, which is not pedantry: writers emit `"level":null` for an
/// event they had no level for, and reading it as a value rendered the word `NULL` in the
/// level column. It also decides a role for the two-of-three gate, where counting a null would
/// let `{"msg":"hello","level":null}` — one real role — through as a log line.
fn take<'a>(fields: &'a Map<String, Value>, keys: &[&'a str]) -> Option<(&'a str, &'a Value)> {
    keys.iter().find_map(|key| match fields.get(*key) {
        Some(Value::Null) | None => None,
        Some(value) => Some((*key, value)),
    })
}

/// `HH:MM:SS` out of whatever the writer called a timestamp.
///
/// **A numeric timestamp is rendered in UTC**, not local time. Converting an epoch to a wall
/// clock in the reader's zone needs the tz database, and this workspace has no date crate;
/// pulling one in for six characters on a log line is not the trade. An RFC 3339 *string* is
/// sliced rather than parsed, so it keeps whatever offset the writer already applied — which
/// is the common case and the one people actually read.
fn clock(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        // `2026-08-28T19:14:59.123Z` -> `19:14:59`. Anything else is shown as the writer wrote
        // it: a made-up format rendered as if it were understood is worse than a raw one.
        let after_t = text.split_once('T').map(|(_, rest)| rest).unwrap_or(text);
        let clock = &after_t[..after_t.len().min(8)];
        let shaped = clock.len() == 8
            && clock.as_bytes()[2] == b':'
            && clock.as_bytes()[5] == b':'
            && clock.bytes().filter(u8::is_ascii_digit).count() == 6;
        return Some(if shaped {
            clock.to_string()
        } else {
            text.to_string()
        });
    }
    // zap writes float seconds, pino writes integer milliseconds; a value far past what a
    // seconds-since-epoch could plausibly be is the latter.
    let epoch = value.as_f64()?;
    let secs = if epoch > 1e11 { epoch / 1000.0 } else { epoch } as i64;
    if secs <= 0 {
        return None;
    }
    let day = secs.rem_euclid(86_400);
    Some(format!(
        "{:02}:{:02}:{:02}",
        day / 3600,
        (day % 3600) / 60,
        day % 60
    ))
}

/// A level, normalised across the string and numeric dialects, with the colour it earns.
///
/// The numbers are two scales that overlap and disagree: bunyan and pino count *up* in tens
/// from 10 (trace) to 60 (fatal), while syslog counts *down* from 0 (emerg) to 7 (debug). A
/// value under 10 is read as syslog, and everything else as bunyan — the two never collide,
/// because no bunyan level is below 10.
fn severity(value: &Value) -> (String, &'static str) {
    let name = match value {
        Value::String(text) => text.to_ascii_uppercase(),
        Value::Number(number) => {
            let n = number.as_f64().unwrap_or(-1.0) as i64;
            match n {
                0..=3 => "ERROR",
                4 => "WARN",
                5 | 6 => "INFO",
                7 => "DEBUG",
                ..=9 => "TRACE",
                10..=19 => "TRACE",
                20..=29 => "DEBUG",
                30..=39 => "INFO",
                40..=49 => "WARN",
                50..=59 => "ERROR",
                _ => "FATAL",
            }
            .to_string()
        }
        other => plain(other).to_ascii_uppercase(),
    };
    // Prefixes rather than equality: `WARNING`, `ERR`, `CRITICAL` and `INFORMATION` are all
    // somebody's spelling of these six, and the abbreviation is what fits the column anyway.
    let (text, colour) = match name.as_str() {
        n if n.starts_with("TRACE") => ("TRACE", DIM),
        n if n.starts_with("DEBUG") || n.starts_with("VERBOSE") => ("DEBUG", DIM),
        n if n.starts_with("INFO") => ("INFO", GREEN),
        n if n.starts_with("NOTICE") => ("NOTE", BLUE),
        n if n.starts_with("WARN") => ("WARN", YELLOW),
        n if n.starts_with("ERR") => ("ERROR", RED),
        n if n.starts_with("FATAL") || n.starts_with("CRIT") || n.starts_with("PANIC") => {
            ("FATAL", BOLD)
        }
        // An unknown level still gets the column, clipped to it, so the messages stay aligned.
        _ => return (clip(&name, 5), CYAN),
    };
    (text.to_string(), colour)
}

/// A value as a person reads it: a string is its own text, everything else is compact JSON.
fn plain(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// A field's value beside its key. Strings lose their quotes unless the quotes are load-
/// bearing — a value with a space in it runs into the next `key=` without them.
fn value_text(value: &Value) -> String {
    match value {
        Value::String(text)
            if !text.is_empty() && !text.contains(char::is_whitespace) && !text.contains('"') =>
        {
            text.clone()
        }
        Value::String(text) => format!("{:?}", text),
        other => other.to_string(),
    }
}

fn multiline(value: &Value) -> bool {
    value.as_str().is_some_and(|text| text.contains('\n'))
}

fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Strip SGR, so an assertion is about the text and not about which escape spells dim.
    fn text(line: &str) -> String {
        let mut out = String::new();
        let mut rest = line;
        while let Some(start) = rest.find('\x1b') {
            out.push_str(&rest[..start]);
            let after = &rest[start..];
            let end = after.find('m').expect("an SGR run ends in m");
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        out
    }

    /// One real line per ecosystem, transcribed from each writer's own README/defaults.
    ///
    /// The point of a corpus rather than one synthetic line is the key names: every one of
    /// these calls the same three things something different, and a table that has drifted
    /// from any of them fails here rather than in somebody's pane.
    #[test]
    fn a_line_from_each_ecosystem_renders() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "logrus",
                r#"{"level":"error","msg":"conn failed","time":"2026-08-28T19:14:59Z","port":5432}"#,
                "19:14:59 ERROR conn failed  port=5432",
            ),
            (
                "zap",
                r#"{"level":"info","ts":1756408499.123,"caller":"srv/main.go:42","msg":"listening"}"#,
                "19:14:59 INFO  listening  caller=srv/main.go:42",
            ),
            (
                "pino",
                r#"{"level":30,"time":1756408499123,"pid":11,"hostname":"box","msg":"ready"}"#,
                "19:14:59 INFO  ready",
            ),
            (
                "bunyan",
                r#"{"name":"api","hostname":"box","pid":9,"level":50,"msg":"boom","time":"2026-08-28T19:14:59.000Z","v":0}"#,
                "19:14:59 ERROR boom  name=api",
            ),
            (
                "structlog",
                r#"{"event":"user login","level":"warning","user":"ada"}"#,
                "WARN  user login  user=ada",
            ),
            (
                "serilog",
                r#"{"@t":"2026-08-28T19:14:59.1Z","@l":"Debug","@mt":"cache miss","key":"k1"}"#,
                "19:14:59 DEBUG cache miss  key=k1",
            ),
            (
                "ecs",
                r#"{"@timestamp":"2026-08-28T19:14:59.000Z","log.level":"info","message":"served","http.status":200}"#,
                "19:14:59 INFO  served  http.status=200",
            ),
        ];
        for (writer, line, want) in cases {
            let rendered = render(line).unwrap_or_else(|| panic!("{writer} went unrecognised"));
            assert_eq!(&text(&rendered), want, "{writer}");
        }
    }

    /// The half that matters: what this must **not** claim.
    ///
    /// A claimed line is destroyed — the rewrite is upstream of the screen mirror, so it is
    /// not in the scrollback and not in a reattach snapshot. Every line here is somebody
    /// reading their own data in a shell, and every one of them is shaped like a log.
    #[test]
    fn data_that_merely_looks_like_json_is_left_alone() {
        let untouched = [
            // `cat` of a minified manifest: an object, no roles at all.
            r#"{"name":"cide-ui","version":"0.6.0","private":true,"scripts":{"dev":"vite"}}"#,
            // `jq -c` of an API response. `message` alone is one role, and one is not enough
            // — this is the case the two-role rule exists for.
            r#"{"id":7,"message":"hello","ok":true}"#,
            // A response that says `error`, which is not a level.
            r#"{"error":"not found","status":404}"#,
            "{}",
            // Not an object.
            r#"[{"level":"info","msg":"x"}]"#,
            r#""level: info, msg: x""#,
            // A Go map printed with braces — shaped like JSON, and not JSON.
            "map[level:info msg:started]",
            // Already painted by its writer. See the escape check in `render`.
            "\x1b[32m{\"level\":\"info\",\"msg\":\"green already\"}\x1b[0m",
            // Truncated: the tail of a line whose head went out raw.
            r#"{"level":"info","msg":"cut off"#,
        ];
        for line in untouched {
            assert_eq!(
                render(line),
                None,
                "claimed a line it should have kept: {line}"
            );
        }
    }

    /// A stack gets lines of its own, because a stack folded into one cell is not readable
    /// and is the reason somebody opened the log.
    #[test]
    fn a_multiline_error_is_rendered_as_lines() {
        let line = r#"{"level":"error","msg":"panic","stack":"main.go:1\nmain.go:2","req":"r1"}"#;
        let rendered = text(&render(line).expect("rendered"));
        assert_eq!(
            rendered,
            "ERROR panic  req=r1\nstack:\n    main.go:1\n    main.go:2"
        );
        // A single-line error stays a field: spending three lines on `error=EOF` would make
        // the common case worse to read than the raw JSON was.
        let short = r#"{"level":"error","msg":"read","error":"EOF"}"#;
        assert_eq!(
            text(&render(short).expect("rendered")),
            "ERROR read  error=EOF"
        );
    }

    /// Levels arrive as words in several spellings and as numbers on two disagreeing scales.
    #[test]
    fn every_dialect_of_a_level_lands_in_one_column() {
        let cases = [
            (r#"{"level":"WARNING","msg":"m"}"#, "WARN "),
            (r#"{"level":"err","msg":"m"}"#, "ERROR"),
            (r#"{"level":"critical","msg":"m"}"#, "FATAL"),
            (r#"{"level":"Verbose","msg":"m"}"#, "DEBUG"),
            // bunyan/pino: tens, up.
            (r#"{"level":20,"msg":"m"}"#, "DEBUG"),
            (r#"{"level":60,"msg":"m"}"#, "FATAL"),
            // syslog: single digits, down. 3 is `err`, 7 is `debug` — the opposite direction.
            (r#"{"level":3,"msg":"m"}"#, "ERROR"),
            (r#"{"level":7,"msg":"m"}"#, "DEBUG"),
            // An invented level still gets the column, so the messages stay aligned.
            (r#"{"level":"weird","msg":"m"}"#, "WEIRD"),
            (r#"{"level":"astonishing","msg":"m"}"#, "ASTON"),
        ];
        for (line, want) in cases {
            let rendered = text(&render(line).expect("rendered"));
            assert_eq!(&rendered[..5], want, "{line}");
            assert!(rendered.starts_with(&format!("{want} m")), "{rendered:?}");
        }
    }

    /// A timestamp keeps the writer's own offset when it is a string, and is UTC when it is a
    /// number — the module header carries why, and this is the assertion that says so out loud.
    #[test]
    fn a_timestamp_is_sliced_rather_than_reinterpreted() {
        let offset = r#"{"level":"info","msg":"m","time":"2026-08-28T21:14:59+02:00"}"#;
        assert!(text(&render(offset).expect("rendered")).starts_with("21:14:59"));
        // Not a shape this understands: shown as written rather than as a guess.
        let odd = r#"{"level":"info","msg":"m","time":"Thu Aug 28"}"#;
        assert!(text(&render(odd).expect("rendered")).starts_with("Thu Aug 28 "));
        // Seconds and milliseconds land on the same clock.
        let secs = r#"{"level":"info","msg":"m","ts":1756408499}"#;
        let millis = r#"{"level":"info","msg":"m","ts":1756408499000}"#;
        assert_eq!(
            text(&render(secs).expect("rendered")),
            text(&render(millis).expect("rendered"))
        );
    }

    /// The hyperlink covers the time and the level, and stops there.
    ///
    /// The span matters as much as the link: xterm discards a later provider's link where it
    /// overlaps an earlier one, and its OSC-link provider runs before ours — so a hyperlink
    /// over the whole line would delete the file-path link inside `caller=srv/main.go:42`
    /// with nothing said about it anywhere.
    #[test]
    fn the_link_covers_the_prefix_and_nothing_else() {
        let line = r#"{"level":"error","time":"2026-08-28T19:14:59Z","msg":"m","caller":"srv/main.go:42"}"#;
        let linked = render_linked(line, "cide-log:s:7").expect("rendered");
        let open = "\x1b]8;;cide-log:s:7\x1b\\";
        assert!(linked.starts_with(open), "{linked:?}");
        let (prefix, rest) = linked[open.len()..]
            .split_once(OSC8_END)
            .expect("the hyperlink is closed");
        assert_eq!(text(prefix), "19:14:59 ERROR");
        assert!(
            rest.starts_with(' ') && text(rest).contains("srv/main.go:42"),
            "the message and its fields are outside the link: {rest:?}"
        );
        // Without a uri nothing is marked at all — the same rendering, no escapes beyond SGR.
        assert!(!render(line).expect("rendered").contains("]8;"));
    }

    /// A line whose timestamp this module cannot read and which carries no level has no prefix
    /// to hang a link on, and gets none rather than a zero-width one nobody can click.
    #[test]
    fn a_line_with_no_prefix_gets_no_link() {
        let line = r#"{"time":"whenever","msg":"m","k":"v"}"#;
        let rendered = render_linked(line, "cide-log:s:1").expect("rendered");
        assert!(
            rendered.contains("]8;"),
            "an unreadable timestamp still prints: {rendered:?}"
        );
        let bare = r#"{"ts":0,"msg":"m","level":null}"#;
        if let Some(rendered) = render_linked(bare, "cide-log:s:2") {
            assert!(!rendered.starts_with("\x1b]8"), "{rendered:?}");
        }
    }

    /// The tail predicate, which is what keeps a shell prompt on screen.
    #[test]
    fn only_a_brace_is_worth_holding() {
        assert!(could_start(r#"{"level":"info","msg":"half"#));
        assert!(could_start("  {"));
        assert!(!could_start("user@host:~$ "));
        assert!(!could_start(""));
        assert!(!could_start("Compiling cide-core v0.6.0"));
        assert!(!could_start(&"x".repeat(MAX_LINE + 1)));
    }

    /// Colour is emitted as bare SGR from the eight-colour set, never as a 24-bit literal.
    ///
    /// `xterm`'s `minimumContrastRatio: 3` silently repaints a foreground that lands too near
    /// its background, so a `38;2;r;g;b` here would be *almost* the colour asked for, in a way
    /// that differs per theme and leaves nothing to grep for.
    #[test]
    fn nothing_emits_a_24_bit_colour() {
        let rendered = render(r#"{"level":"error","msg":"m","k":"v"}"#).expect("rendered");
        assert!(!rendered.contains("38;2"), "{rendered:?}");
        assert!(
            rendered.ends_with(RESET) || !rendered.contains('\x1b') || rendered.contains(RESET)
        );
        // And every colour opened is closed: a run left open paints the rest of the pane.
        let opens = rendered.matches('\x1b').count();
        let resets = rendered.matches(RESET).count();
        assert_eq!(
            opens,
            resets * 2,
            "every SGR run is one open and one reset: {rendered:?}"
        );
    }
}
