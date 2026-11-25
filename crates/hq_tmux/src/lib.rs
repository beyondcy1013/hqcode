//! Parameterized, persistent tmux-backed PTY session engine.
//!
//! Designed so HQCode and compatible downstream control planes (e.g. webClx)
//! can run side-by-side on the same host with isolated tmux sockets, session
//! names, and state files while sharing identical Terminal-I/O-Only semantics.

pub mod input_pacing;
pub mod manager;
pub mod tmux_cmd;

pub use manager::{
    CreateSessionRequest, SessionMetadata, SessionSnapshot, TerminalManager, TerminalOutputChunk,
    TerminalStreamEvent, TerminalViewportSize,
};
pub use tmux_cmd::TmuxNamespace;
