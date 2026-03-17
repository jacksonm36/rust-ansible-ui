//! API request/response schemas (serde).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectRead {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub git_url: Option<String>,
    pub git_branch: Option<String>,
    pub git_credential_id: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct ProjectCreate {
    pub name: String,
    pub description: Option<String>,
    pub git_url: Option<String>,
    pub git_branch: Option<String>,
    pub git_credential_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ProjectUpdate {
    pub name: Option<String>,
    pub description: Option<String>,
    pub git_url: Option<String>,
    pub git_branch: Option<String>,
    pub git_credential_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InventoryRead {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub description: String,
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct InventoryCreate {
    pub project_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub content: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct InventoryUpdate {
    pub name: Option<String>,
    pub description: Option<String>,
    pub content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialRead {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub kind: String,
    pub extra: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CredentialCreate {
    pub project_id: i64,
    pub name: String,
    pub kind: Option<String>,
    pub secret: String,
    pub extra: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CredentialUpdate {
    pub name: Option<String>,
    pub kind: Option<String>,
    pub extra: Option<String>,
    pub secret: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobTemplateRead {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub description: String,
    pub playbook_path: String,
    pub inventory_id: Option<i64>,
    pub credential_id: Option<i64>,
    pub extra_vars: String,
    pub schedule_enabled: bool,
    pub schedule_cron: Option<String>,
    pub schedule_tz: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct JobTemplateCreate {
    pub project_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub playbook_path: String,
    pub inventory_id: Option<i64>,
    pub credential_id: Option<i64>,
    pub extra_vars: Option<String>,
    pub schedule_enabled: Option<bool>,
    pub schedule_cron: Option<String>,
    pub schedule_tz: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct JobTemplateUpdate {
    pub name: Option<String>,
    pub description: Option<String>,
    pub playbook_path: Option<String>,
    pub inventory_id: Option<i64>,
    pub credential_id: Option<i64>,
    pub extra_vars: Option<String>,
    pub schedule_enabled: Option<bool>,
    pub schedule_cron: Option<String>,
    pub schedule_tz: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRead {
    pub id: i64,
    pub project_id: i64,
    pub job_template_id: Option<i64>,
    pub status: String,
    pub playbook_path: String,
    pub extra_vars: String,
    pub output_log: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobListSummary {
    pub id: i64,
    pub project_id: i64,
    pub job_template_id: Option<i64>,
    pub status: String,
    pub playbook_path: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct JobLaunch {
    pub job_template_id: i64,
    pub extra_vars_override: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PullResponse {
    pub ok: bool,
    pub message: String,
    pub playbooks: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct NextRunResponse {
    pub next_run: Option<String>,
}
