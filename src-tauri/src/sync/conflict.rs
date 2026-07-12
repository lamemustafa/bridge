use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ConflictResolution {
    Tally,
    Axal,
    Manual,
}
