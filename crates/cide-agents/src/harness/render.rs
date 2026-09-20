//! What every stdout-bound harness's rendering shares. (M44)
//!
//! `opencode.rs` wrote the first `render_event`, and its header carries the whole argument for
//! rendering upstream of the mirror. What is here is the half of it that is not about
//! opencode's event shape at all: the fixed mirror size, the live marker and the discipline
//! around it, and the small string helpers every one-line rendering needs. Codex's stream is a
//! different JSON with the same needs, and the marker discipline in particular — a cursor-up
//! erase that assumes the marker never wrapped, a `compose` that draws it again under every
//! line while a step is open — is the kind of thing two copies let drift: one harness's pane
//! keeps a stale marker and nothing in either file's tests can see the other's.
//!
//! So this module holds the machinery and each harness holds only what reads *its* events:
//! `opencode.rs` keeps `render_tool`, `display_tool` and the `part.state` timing; `codex.rs`
//! keeps its item table. `pub(super)` throughout, because nothing outside `harness/` renders.

use cide_ipc::RunState;
use cide_pty::Rendered;
use serde_json::Value;

use super::RenderState;

/// The states no output line may move, which are [`Harness::observe`]'s two documented refusals.
///
/// * **`Paused`** — the child is `SIGSTOP`ped. A line that was already in the coalescer when the
///   signal landed must not thaw the row; only a resume clears a freeze cide asserted.
/// * **`Finished` / `Failed`** — terminal. The last few lines of a dying child's output are read
///   after its exit has been reaped, so without this a finished run goes back to `Running` and
///   stays there for ever.
///
/// `Idle` is emphatically not one of them — a rule that now outlives its only producer: since
/// the 06202dd6 fix no line maps to `Idle`, so this harness's runs never hold it. It stays
/// un-absorbed anyway, because the rule is about recovery, not production: if a run ever *is*
/// `Idle` (a hand-set state in a test, a variant a future slice produces), a line from its child
/// must still be able to move it back to `Running` rather than freeze it there.
pub(super) fn absorbs(current: &RunState) -> bool {
    matches!(
        current,
        RunState::Paused { .. } | RunState::Finished { .. } | RunState::Failed { .. }
    )
}

/// The mirror a run's session is spawned with — and, since M42, never resized from.
///
/// Wide, because `vt100` does not reflow: a row is wrapped at the width the mirror had when the
/// bytes arrived, and a later resize *truncates* rows to the new width and clears every wrap
/// flag. The 80-column default a headless run used to get handed every pane its history as
/// 80-column fragments with hard breaks — "text is not on full width". At 200 columns almost no
/// rendered line wraps at all, the few that do keep their flag for `cide-pty`'s replay to join,
/// and the pane's own terminal soft-wraps each logical line at whatever width it actually has.
/// Fixed (`SpawnSpec::fixed_size`) because the child prints lines and never reads the size, so
/// the only thing a resize could do is the damage above. The row count is the mirror's viewport
/// and matters to nothing but the scrollback's accounting.
pub const RUN_COLS: u16 = 200;

pub const RUN_ROWS: u16 = 50;

/// How much of a tool's title survives on its one line. The whole call is a click away.
pub(super) const TITLE_BUDGET: usize = 160;

pub(super) const DIM: &str = "\x1b[2m";

pub(super) const BOLD: &str = "\x1b[1m";

pub(super) const CYAN: &str = "\x1b[36m";

pub(super) const RED: &str = "\x1b[31m";

pub(super) const RESET: &str = "\x1b[0m";

/// The row that says the run is doing something right now. (M42)
///
/// Short on purpose — it must never wrap, because [`ERASE_MARKER`] goes up exactly one row.
pub(super) const MARKER: &str = "\x1b[2m▸ working…\x1b[0m";

/// Cursor up one row, to column 0, clear that row: the [`MARKER`] is gone before the next line
/// is written. Every byte of this reaches the vt100 mirror and the pane alike, so a replay of
/// the mirror never shows a marker that was erased.
pub(super) const ERASE_MARKER: &str = "\x1b[1A\r\x1b[2K";

/// A line that is not one of ours — the CLI's own prose. Kept as it arrived, unless a marker
/// is on screen: then it goes under the erase like any rendered line, or the next erase would
/// take the prose instead of the marker.
pub(super) fn prose(state: &mut RenderState, line: &str) -> Rendered {
    if !state.marker {
        return Rendered::Keep;
    }
    compose(state, Some(line.trim_end().to_string()))
}

/// The marker bookkeeping around one rendering. `text` is what this line draws, or `None` for
/// a line that draws nothing itself but may move the marker (a `step_start`).
///
/// Also where [`RenderState::last_line_unix_ms`] is kept, because this is the single funnel both
/// harnesses' renderings pass through: a line that draws something is the end of whatever
/// silence preceded it, and [`measured_ms`] reads exactly that gap. A line that draws nothing —
/// a `step_start`, a dropped `item.updated` — deliberately does not move it, or the silence a
/// thinking block is measured over would be cut short by a marker redraw nobody saw.
pub(super) fn compose(state: &mut RenderState, text: Option<String>) -> Rendered {
    if text.is_some() {
        state.last_line_unix_ms = state.now_unix_ms.or(state.last_line_unix_ms);
    }
    let mut out = String::new();
    if state.marker {
        out.push_str(ERASE_MARKER);
    }
    if let Some(text) = &text {
        out.push_str(text);
    }
    if state.in_step {
        if text.is_some() {
            out.push('\n');
        }
        out.push_str(MARKER);
    }
    state.marker = state.in_step;
    if out.is_empty() {
        Rendered::Drop
    } else {
        Rendered::Replace(out)
    }
}

/// The input document with its top-level values flattened — `{"command":"ls"}` reads `ls`.
pub(super) fn compact_input(input: &Value) -> String {
    match input {
        Value::Object(map) => map
            .values()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        other => other.to_string(),
    }
}

/// `chars`, not bytes: a byte slice of UTF-8 panics on a boundary, and this crate is linked
/// into a process built with `panic = "abort"`.
pub(super) fn clip(text: &str, budget: usize) -> String {
    if text.chars().count() <= budget {
        return text.to_string();
    }
    let kept: String = text.chars().take(budget).collect();
    format!("{}…", kept.trim_end())
}

/// Whitespace runs collapsed to one space — a reasoning fragment arrives with its own layout.
pub(super) fn one_line_of(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `10454` reads `10.5k`; small counts stay exact.
pub(super) fn thousands(n: u64) -> String {
    if n < 1_000 {
        return n.to_string();
    }
    format!("{:.1}k", n as f64 / 1_000.0)
}

/// `8ms`, `1.2s`, `2m05s` — three shapes, because a build and a `read` are three orders of
/// magnitude apart and one format reads badly at one end or the other.
///
/// Shared since M62: the thought row spells a duration and codex has no formatter of its own.
pub(super) fn duration(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1_000.0)
    } else {
        format!("{}m{:02}s", ms / 60_000, (ms % 60_000) / 1_000)
    }
}

/// The dim tail every one-line rendering ends with: how long it took, and the handle a click
/// resolves. The handle is **last** and spelled `#<n>` because `ui/src/terminal/runLinks.ts`
/// reads it off the end of the line; the two leading spaces separate it from the title.
///
/// One producer, since M62, for the same reason `cide_git::push::preview` is one: a row spelled
/// independently in the other harness drifts out of that file's grammar, and the only symptom is
/// that one harness's rows quietly stop being clickable.
pub(super) fn tail(ms: Option<u64>, handle: Option<u64>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(ms) = ms {
        parts.push(duration(ms));
    }
    if let Some(handle) = handle {
        parts.push(format!("#{handle}"));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("  {DIM}{}{RESET}", parts.join(" "))
    }
}

/// A block of the model's thinking, collapsed to one row. (M62)
///
/// The reasoning text itself is **not** drawn. It used to be — whitespace-flattened and clipped
/// to 240 characters behind this same glyph — and that is what this replaces: a long, truncated,
/// unopenable line per thinking block, pushing the tool calls and the model's actual words off
/// the screen. What is worth a glance is how long the model thought; the whole text is behind
/// the handle, exactly as a tool call's input and output are.
///
/// `ms` is `None` when nothing measured it, and then **no duration is printed at all**. A `0ms`
/// would be a measurement cide did not make, and it is the shape a forgotten clock produces —
/// see [`RenderState::now_unix_ms`].
pub(super) fn thought_line(ms: Option<u64>, handle: Option<u64>) -> String {
    format!("{DIM}∴ thought{RESET}{}", tail(ms, handle))
}

/// How long the run said nothing before this line — the duration of a thinking block no harness
/// states a clock for. (M62)
///
/// Codex records no timing on an item at all (`render_command`'s note says so), and opencode's
/// `time.end` is optional on a part, so a row that could only print a *stated* duration would
/// silently print none on the harness that needs it most. The gap between the previous rendered
/// line and this one is the interval in which the run produced nothing, which for a reasoning
/// item is the thinking — measured on the coalescer thread as each line arrives, the same clock
/// and the same moment `logring` stamps a kept line with.
///
/// `None` rather than zero wherever it cannot be computed: before the first rendered line of a
/// run, and on any caller that did not set the clock.
pub(super) fn measured_ms(state: &RenderState) -> Option<u64> {
    let now = state.now_unix_ms?;
    let last = state.last_line_unix_ms?;
    now.checked_sub(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The row two harnesses draw and one file parses. (M62)
    ///
    /// Every assertion here is about a spelling `ui/src/terminal/runLinks.ts` reads: a glyph, one
    /// space, a whitespace-free word, and the `#<n>` last on the line. Two literals in two
    /// languages with no compiler between them is how a click quietly stops doing anything, so
    /// `ui/scripts/check-json-log.mjs` drives `parseRunLine` over these exact strings.
    #[test]
    fn a_thought_is_one_row_that_names_its_duration_and_its_handle() {
        assert_eq!(
            strip(&thought_line(Some(4_100), Some(8))),
            "∴ thought  4.1s #8"
        );
        // Neither half is required, and an absent duration draws nothing at all — a `0ms` there
        // would be a measurement nobody made.
        assert_eq!(strip(&thought_line(None, Some(8))), "∴ thought  #8");
        assert_eq!(strip(&thought_line(Some(8), None)), "∴ thought  8ms");
        assert_eq!(strip(&thought_line(None, None)), "∴ thought");

        // One producer, so a tool call's tail and a thought's cannot drift apart.
        assert_eq!(strip(&tail(Some(1_234), Some(7))), "  1.2s #7");
    }

    /// Three orders of magnitude, three shapes — a `read` and a build are that far apart.
    #[test]
    fn a_duration_reads_at_every_scale() {
        assert_eq!(duration(8), "8ms");
        assert_eq!(duration(1_234), "1.2s");
        assert_eq!(duration(125_000), "2m05s");
    }

    /// The measured fallback, and the two ways it must answer *unknown* rather than zero.
    #[test]
    fn a_silence_is_measured_only_when_both_ends_are_known() {
        let state = RenderState {
            now_unix_ms: Some(9_000),
            last_line_unix_ms: Some(6_500),
            ..RenderState::default()
        };
        assert_eq!(measured_ms(&state), Some(2_500));

        // A caller that never set the clock. `RenderState` derives `Default`, and a bare `u64`
        // here would have made this 1970 and the answer a confident nonsense.
        assert_eq!(measured_ms(&RenderState::default()), None);
        assert_eq!(
            measured_ms(&RenderState {
                now_unix_ms: Some(9_000),
                ..RenderState::default()
            }),
            None,
            "the first line of a run ended no silence"
        );
    }

    /// `compose` moves the clock on for a line that draws, and leaves it for one that does not —
    /// or the silence a thinking block is measured over would be cut short by a marker redraw
    /// nobody saw.
    #[test]
    fn only_a_line_that_draws_ends_a_silence() {
        let mut state = RenderState {
            now_unix_ms: Some(1_000),
            ..RenderState::default()
        };
        compose(&mut state, Some("● bash  ls".to_string()));
        assert_eq!(state.last_line_unix_ms, Some(1_000));

        state.now_unix_ms = Some(4_000);
        state.in_step = true;
        compose(&mut state, None);
        assert_eq!(
            state.last_line_unix_ms,
            Some(1_000),
            "a step_start drew nothing and ended no silence"
        );
        assert_eq!(measured_ms(&state), Some(3_000));
    }

    /// The SGR removed, so an assertion is about the characters a person sees — and about the
    /// characters `runLinks.ts` reads, which is the buffer text, never the escapes.
    fn strip(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars();
        while let Some(ch) = chars.next() {
            if ch != '\x1b' {
                out.push(ch);
                continue;
            }
            for ch in chars.by_ref() {
                if ch == 'm' {
                    break;
                }
            }
        }
        out
    }
}
