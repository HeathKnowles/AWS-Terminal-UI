use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct ServiceModel {
    #[serde(default)]
    pub version: String,
    pub metadata: Metadata,
    #[serde(default)]
    pub operations: HashMap<String, Operation>,
    #[serde(default)]
    pub shapes: HashMap<String, Shape>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    #[serde(default)]
    pub api_version: String,
    #[serde(default)]
    pub endpoint_prefix: String,
    #[serde(default)]
    pub service_full_name: String,
    #[serde(default)]
    pub service_id: String,
    #[serde(default)]
    pub service_abbreviation: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    pub name: String,
    #[serde(default)]
    pub input: Option<ShapeRef>,
    #[serde(default)]
    pub output: Option<ShapeRef>,
    #[serde(default)]
    pub documentation: String,
    #[serde(default)]
    pub deprecated: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShapeRef {
    pub shape: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Shape {
    #[serde(rename = "type")]
    pub type_name: String,
    #[serde(default)]
    pub members: HashMap<String, Member>,
    #[serde(default)]
    pub member: Option<ShapeRef>,
    #[serde(default)]
    pub key: Option<ShapeRef>,
    #[serde(default)]
    pub value: Option<ShapeRef>,
    #[serde(default)]
    pub required: Vec<String>,
    #[serde(default)]
    pub enum_values: Vec<String>,
    #[serde(default)]
    pub documentation: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub shape: String,
    #[serde(default)]
    pub documentation: String,
    #[serde(default)]
    pub location_name: Option<String>,
    #[serde(default)]
    pub deprecated: bool,
    #[serde(default)]
    pub idempotency_token: bool,
    #[serde(default)]
    pub jsonvalue: bool,
    #[serde(default)]
    pub streaming: bool,
}
