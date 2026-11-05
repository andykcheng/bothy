//! Offline maps (DESIGN §16): OpenStreetMap vector tiles in PMTiles archives, one per region.
//!
//! - `pmt`: PMTiles v3 reading (local mmap archive for serving tiles)
//! - `mvt`: vector tile decoding (names and points for the gazetteer)
//! - `places`: offline place search (gazetteer built from the tiles)
//! - `extract`: cut a region out of a remote PMTiles build with HTTP range requests

pub mod extract;
pub mod mvt;
pub mod places;
pub mod pmt;
