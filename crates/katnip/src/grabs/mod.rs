//! Pointer grabs for interactive window move and resize
//! (SUPER+drag), ported from Smithay's smallvil example.

pub mod move_grab;
pub mod resize_grab;

pub use move_grab::MoveSurfaceGrab;
pub use resize_grab::{ResizeSurfaceGrab, edges_for_point};
