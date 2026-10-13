//! Header for a native clipboard image write. Pixels ride in the raw frame's payload, never
//! in a JSON array; this is the other direction of the clipboard bitmap-to-PNG paste path.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Eight-bit RGBA, row-major, with exactly `width * height * 4` payload bytes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ClipboardImageWrite {
    pub width: u32,
    pub height: u32,
}
