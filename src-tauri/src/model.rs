use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSummary {
    pub id: i64,
    pub root: String,
    pub root_id: i64,
    pub status: String,
    pub entries: u64,
    pub logical_bytes: u64,
    pub allocated_bytes: u64,
    pub allocated_estimated: bool,
    pub candidate_bytes: u64,
    pub errors: u64,
    pub skipped: u64,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryRow {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub relative_path: String,
    pub kind: String,
    pub logical_bytes: u64,
    pub allocated_bytes: u64,
    pub modified_at: Option<i64>,
    pub accessed_at: Option<i64>,
    pub complete: bool,
    pub error: Option<String>,
    pub category: String,
    pub disposition: String,
    pub reason: String,
    pub consequence: String,
    pub uncertainty: String,
    pub project: Option<String>,
    pub evidence: Vec<String>,
    pub removable: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryPage {
    pub items: Vec<EntryRow>,
    pub total: u64,
    pub total_bytes: u64,
    pub offset: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroup {
    pub hash: String,
    pub bytes_each: u64,
    pub items: Vec<EntryRow>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanItem {
    pub id: i64,
    pub relative_path: String,
    pub logical_bytes: u64,
    pub reason: String,
    pub consequence: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupPlan {
    pub token: String,
    pub scan_id: i64,
    pub mode: String,
    pub items: Vec<PlanItem>,
    pub estimated_bytes: u64,
    pub expires_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupItemResult {
    pub id: i64,
    pub relative_path: String,
    pub status: String,
    pub message: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupResult {
    pub items: Vec<CleanupItemResult>,
    pub moved_bytes: u64,
    pub deleted_bytes: u64,
    pub free_space_change: Option<i64>,
    pub interrupted: bool,
}
