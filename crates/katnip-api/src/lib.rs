//! Katnip plugin API contract.
//!
//! The stable surface plugins program against: the `Plugin` trait, event
//! hooks (`window_open`, `focus_change`, `workspace_switch`, ...), command
//! registration, and the ABI version used for handshake when loading native
//! `.so` plugins. Kept deliberately small; populated in M6.
