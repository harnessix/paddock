//! Relevant exact closed-enum stream shapes from
//! 851faba:src/eventstream/schema.rs. The unrelated event kinds are omitted.
#![allow(dead_code)]
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Line {
    pub v: u32,
    pub t: String,
    #[serde(flatten)]
    pub event: Event,
}
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    Grant(Grant),
    Stream(Stream),
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantWhat {
    Folder,
    Rule,
    Clipboard,
    Device,
    Network,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantSource {
    You,
    QuestionPanel,
    Approver,
    Profile,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Applies {
    Live,
    NextStart,
    ReopenFiles,
}
#[derive(Debug, Deserialize)]
pub struct Grant {
    pub app: String,
    pub what: GrantWhat,
    pub action: String,
    pub detail: String,
    pub source: GrantSource,
    pub applies: Applies,
    #[serde(default)]
    pub id: Option<String>,
}
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StreamState {
    Started,
    Lost,
}
#[derive(Debug, Deserialize)]
pub struct Stream {
    pub state: StreamState,
}
