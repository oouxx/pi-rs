use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceCollision {
    pub resource_type: String,
    pub name: String,
    pub winner_path: String,
    pub loser_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ResourceDiagnostic {
    #[serde(rename = "warning")]
    Warning {
        message: String,
        path: String,
    },
    #[serde(rename = "collision")]
    Collision {
        message: String,
        path: String,
        collision: ResourceCollision,
    },
}