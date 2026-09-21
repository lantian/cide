//! cide's first listener another machine may reach. (M72)
//!
//! > *"Need to develop an mobile app (android/ios) that will allowe me to connect to cide and see
//! > projects of opened cide and select the project and see it's activly running claude (and
//! > agents) sessions, agents and tasks … Make sure that i able to connect to several running
//! > cide (for example to normal instance and for DEV instance)."*
//!
//! Every socket cide had before this one was loopback or a `0600` unix socket, and each of them
//! was addressed to a process on this machine: the IDE server to a `claude` that was forked here,
//! the hook socket to a binary cide installed, the agent RPC to a child whose identity comes out
//! of its own environment. This one is addressed to a device that is somewhere else, held by a
//! person, and it therefore has to answer questions none of the others do — who is that, may they,
//! and what exactly are they allowed to see.
//!
//! # The shape
//!
//! * [`RemoteServer`] binds, accepts, and runs one task per connection.
//! * [`RemoteHost`] is the only way into cide. Implemented by `cide-app`; faked by the tests.
//! * [`DeviceStore`] is who may connect, and the pairing window that lets a new device join.
//! * The vocabulary — every frame either end may send — is [`cide_ipc::remote`], because it is a
//!   wire type and because that is the crate `cargo xtask codegen` exports TypeScript from.
//!
//! # What this crate deliberately does not know
//!
//! It has no `cide-core`, no `cide-pty`, no `cide-claude` and no `tauri`. It does not know what a
//! workspace is, what `claude` is, or how a terminal works. That is not minimalism for its own
//! sake: the CI job that enumerates workspace members and fails on `tauri|wry|tao` covers this
//! crate for free, and the fake-host tests below are only possible because there is nothing
//! underneath to stand up.
//!
//! # The one rule about what crosses this wire
//!
//! A [`cide_ipc::Workspace`] never does. It carries `Settings`, which carries provider API keys
//! and proxy passwords in plaintext. The types in [`cide_ipc::remote`] cannot name it, and
//! `cide-app`'s `a_projection_carries_no_credential` is what keeps that true as fields are added.

pub mod devices;
pub mod host;
pub mod keys;
pub(crate) mod screen;
pub mod seal;
pub mod server;

pub use devices::{Device, DeviceStore};
pub use host::RemoteHost;
pub use server::{RemoteEvent, RemoteServer, ServerEvent};

/// Everything this crate refuses to do, and why.
#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error("the remote listener could not bind: {0}")]
    Bind(String),
    #[error("{0}")]
    Devices(String),
    #[error("{0}")]
    Pairing(String),
}
