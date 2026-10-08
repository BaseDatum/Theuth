// 144-W1 owns this file (the body is wave 2's).
//! The insight hook (§4.3): called by the ingest job once a source is `completed`. Wave 2 fills it
//! with the default-insight side call; until then it does nothing.

use crate::notebook::Source;

/// Called after every successful ingest with the completed source.
pub fn after_ingest(_source: &Source) {}
