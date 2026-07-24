//! pm-core – Prompt Manager core library (zero dependencies).
//!
//! Owns the on-disk library format (prompts, recipes, order), the template
//! renderer and small JSON/YAML codecs shared by the app, REST API and MCP
//! server.

pub mod json;
pub mod model;
pub mod render;
pub mod storage;
pub mod util;
pub mod yamlish;

pub use json::Json;
pub use model::{Prompt, Recipe};
pub use storage::{Library, StoreError};
