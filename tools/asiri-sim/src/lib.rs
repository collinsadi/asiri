//! The Asiri simulator (architecture §18): an in-process relay, any number of headless clients
//! built on the real engine, and a man-in-the-middle proxy that plays a malicious relay.
//!
//! The end-to-end suites in `tests/` use it, and so does the `asiri-sim` command line for
//! running a local development relay.

pub mod buddy;
pub mod evil;
pub mod harness;

pub use evil::{Action, Proxy};
pub use harness::{
    Client, Harness, LIVEKIT_KEY, LIVEKIT_SECRET, Options, PATIENCE, Recorder, eventually,
};
