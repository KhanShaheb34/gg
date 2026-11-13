//! Commit graph layout: shapes the commit list into lanes, colours and line segments.

pub mod layout;

pub use layout::{Edge, EdgeHalf, LaneAllocator, RowLayout};
