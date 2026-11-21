use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DwellLane {
    Card,
    Terminal,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Dwell,
    Agent,
}

impl ActivityKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dwell => "dwell",
            Self::Agent => "agent",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DwellData {
    pub lane: DwellLane,
    pub page: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentWorkData {
    pub profile_id: String,
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_number: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    #[serde(default)]
    pub manual: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ActivityDetail {
    Dwell(DwellData),
    Agent(AgentWorkData),
}

impl ActivityDetail {
    pub fn dwell(&self) -> Option<&DwellData> {
        match self {
            Self::Dwell(d) => Some(d),
            Self::Agent(_) => None,
        }
    }

    pub fn agent(&self) -> Option<&AgentWorkData> {
        match self {
            Self::Agent(a) => Some(a),
            Self::Dwell(_) => None,
        }
    }
}

fn is_false(v: &bool) -> bool {
    !*v
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivitySegment {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    pub kind: ActivityKind,
    pub start_ms: u64,
    pub end_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_id: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub open: bool,
    pub data: ActivityDetail,
}

impl ActivitySegment {
    pub fn duration_ms(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }

    pub fn dwell_data(&self) -> Option<&DwellData> {
        (self.kind == ActivityKind::Dwell)
            .then(|| self.data.dwell())
            .flatten()
    }

    pub fn agent_data(&self) -> Option<&AgentWorkData> {
        (self.kind == ActivityKind::Agent)
            .then(|| self.data.agent())
            .flatten()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DwellBucketTotals {
    pub card: u64,
    pub terminal: u64,
    pub other: u64,
}

impl DwellBucketTotals {
    pub fn total(&self) -> u64 {
        self.card
            .saturating_add(self.terminal)
            .saturating_add(self.other)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectActivityTotals {
    pub dwell_ms: u64,
    pub agent_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentBucketStat {
    pub profile_id: String,
    pub provider: String,
    pub model: String,
    pub ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityBucket {
    pub start_ms: u64,
    pub end_ms: u64,
    pub dwell: DwellBucketTotals,
    pub agent_ms: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub by_project: BTreeMap<String, ProjectActivityTotals>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub by_agent: Vec<AgentBucketStat>,
}
