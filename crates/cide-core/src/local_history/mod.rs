//! Root-scoped local recovery storage and eligibility, without a webview dependency.
mod policy;
mod store;
pub use policy::{
    HistoryEligibility, RootIdentity, deepest_root, validate_policy, validate_relative,
};
pub use store::HistoryStore;
