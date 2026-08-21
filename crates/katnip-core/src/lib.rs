//! Katnip window management core.
//!
//! Pure WM logic with no backend dependencies: window tree, workspaces,
//! tiling layouts (dwindling first), focus tracking, gaps and borders.
//! Populated starting in M1 so layout math can be unit-tested in isolation.
