//! Screen widgets. Each view builds a self-contained GTK widget tree and wires
//! its own behavior against the shared [`crate::ctx::AppCtx`].

pub mod dashboard;
pub mod history;
pub mod new_job;
pub mod profiles;
pub mod settings;
