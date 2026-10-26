//! Root-scoped local recovery storage and eligibility, without a webview dependency.
mod policy;
pub use policy::{
    HistoryEligibility, RootIdentity, deepest_root, validate_policy, validate_relative,
};
