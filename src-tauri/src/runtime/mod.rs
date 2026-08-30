pub mod detector;
pub mod installer;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A Node.js runtime the launcher can use.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub path: PathBuf,
    pub version: String,
    pub source: NodeSource,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NodeSource {
    System,
    Bundled,
}

/// A managed DeepSeek Harness installation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DshInfo {
    pub path: PathBuf,
    pub version: String,
}

/// Public (serde) shapes used by the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodePub {
    pub version: String,
    pub source: NodeSource,
    pub path: String,
}

impl From<&NodeInfo> for NodePub {
    fn from(n: &NodeInfo) -> Self {
        Self {
            version: n.version.clone(),
            source: n.source,
            path: n.path.to_string_lossy().to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DshPub {
    pub version: String,
    pub path: String,
}

impl From<&DshInfo> for DshPub {
    fn from(d: &DshInfo) -> Self {
        Self {
            version: d.version.clone(),
            path: d.path.to_string_lossy().to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvStatus {
    pub node: Option<NodePub>,
    pub dsh: Option<DshPub>,
    pub ready: bool,
}

impl EnvStatus {
    pub fn from_parts(node: Option<&NodeInfo>, dsh: Option<&DshInfo>) -> Self {
        let ready = node
            .map(|n| detector::node_major(&n.version).map(|m| m >= detector::MIN_NODE_MAJOR).unwrap_or(false))
            .unwrap_or(false)
            && dsh.is_some();
        Self {
            node: node.map(NodePub::from),
            dsh: dsh.map(DshPub::from),
            ready,
        }
    }
}

/// Progress message for environment preparation, pushed to the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvProgress {
    pub stage: String,
    pub message: String,
    pub error: Option<String>,
    pub error_details: Option<String>,
}

impl EnvProgress {
    pub fn new(stage: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            stage: stage.into(),
            message: message.into(),
            error: None,
            error_details: None,
        }
    }

    pub fn fail(stage: impl Into<String>, message: impl Into<String>, details: Option<String>) -> Self {
        let message = message.into();
        Self {
            stage: stage.into(),
            message: message.clone(),
            error: Some(message),
            error_details: details,
        }
    }
}


