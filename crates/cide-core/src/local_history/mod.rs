//! Root-scoped local recovery storage and eligibility, without a webview dependency.
mod comparison;
mod gates;
pub use gates::{EditorIdentity, RestoreGates, RestorePermit, WriteLease};
mod coordinator;
mod policy;
pub use coordinator::{HistoryCoordinator, HistoryTreeChange};
mod store;
pub use policy::{
    HistoryEligibility, RootIdentity, deepest_root, validate_policy, validate_relative,
};
pub use store::HistoryStore;
