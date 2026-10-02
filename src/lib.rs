#![warn(clippy::pedantic)]

use serde::{Deserialize, Serialize};

pub mod retry;

/// Entry in the lists seen in the json files under the root.
#[derive(Serialize, Deserialize)]
pub struct MergeListing {
    pub source_package: String,
    pub short_description: String,
    pub link: String,
    pub uploaded: UploadedStatus,
    pub age: i32,
    pub user: String,
    pub uploader: Option<String>,
    #[serde(default)]
    pub teams: Vec<String>,
    pub binaries: Vec<String>,
    pub base_version: Option<String>,
    pub left_version: Option<String>,
    pub right_version: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadedStatus {
    Outstanding,
    New,
}
