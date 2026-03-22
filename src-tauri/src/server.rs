//! Axum HTTP server: API routes and static files.

use crate::crud;
use crate::db::DbPool;
use crate::git_support;
use crate::playbook_discovery;
use crate::runner;
use crate::ssh_deployer;
use crate::schemas::*;
use crate::secrets;
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
#[cfg(feature = "embedded-static")]
use axum::{
    body::Body,
    http::{header as http_header, Method, Uri},
};
use std::collections::HashMap;
use std::path::PathBuf;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;

lazy_static::lazy_static! {
    /// Only one ICMP scan at a time so many parallel requests cannot fork unlimited load.
    static ref SSH_SCAN_SEM: tokio::sync::Semaphore = tokio::sync::Semaphore::new(1);
}

#[cfg(feature = "embedded-static")]
#[derive(rust_embed::RustEmbed)]
#[folder = "../static/"]
struct EmbeddedStatic;

/// Where UI assets are loaded from.
#[derive(Clone)]
pub enum StaticSource {
    /// Serve from a directory on disk (development / Tauri).
    Filesystem(PathBuf),
    /// Serve from assets embedded at compile time (`embedded-static` feature).
    #[cfg(feature = "embedded-static")]
    Embedded,
}

#[derive(Clone)]
pub struct AppState {
    pub db: DbPool,
    /// `None` when using compile-time embedded static files.
    pub static_dir: Option<PathBuf>,
}

#[cfg(feature = "embedded-static")]
fn mime_for_path(path: &str) -> &'static str {
    let p = path.to_lowercase();
    if p.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if p.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if p.ends_with(".js") {
        "application/javascript; charset=utf-8"
    } else if p.ends_with(".json") {
        "application/json; charset=utf-8"
    } else if p.ends_with(".svg") {
        "image/svg+xml"
    } else if p.ends_with(".png") {
        "image/png"
    } else if p.ends_with(".ico") {
        "image/x-icon"
    } else {
        "application/octet-stream"
    }
}

fn api_err(status: StatusCode, detail: &str) -> Response {
    (status, Json(serde_json::json!({ "detail": detail }))).into_response()
}

/// Allowed `credentials.kind` values (server-side; prevents odd kinds reaching runners / UI).
fn credential_kind_ok(kind: &str) -> bool {
    matches!(kind, "ssh" | "password" | "vault" | "git")
}

/// Limits oversized credential payloads (abuse / accidental huge pastes / DB bloat).
const MAX_CREDENTIAL_NAME_LEN: usize = 256;
const MAX_CREDENTIAL_EXTRA_LEN: usize = 16 * 1024;
const MAX_CREDENTIAL_SECRET_BYTES: usize = 512 * 1024;

#[allow(clippy::result_large_err)]
fn validate_credential_create(data: &mut CredentialCreate) -> Result<(), Response> {
    data.name = data.name.trim().to_string();
    if data.name.is_empty() {
        return Err(api_err(
            StatusCode::BAD_REQUEST,
            "Credential name is required.",
        ));
    }
    if data.name.len() > MAX_CREDENTIAL_NAME_LEN {
        return Err(api_err(
            StatusCode::BAD_REQUEST,
            "Credential name is too long (max 256 characters).",
        ));
    }
    if let Some(ref ex) = data.extra {
        if ex.len() > MAX_CREDENTIAL_EXTRA_LEN {
            return Err(api_err(
                StatusCode::BAD_REQUEST,
                "Credential extra is too long (max 16 KiB).",
            ));
        }
    }
    if data.secret.len() > MAX_CREDENTIAL_SECRET_BYTES {
        return Err(api_err(
            StatusCode::BAD_REQUEST,
            "Credential secret is too large (max 512 KiB).",
        ));
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn validate_credential_update(data: &CredentialUpdate) -> Result<(), Response> {
    if let Some(ref n) = data.name {
        let t = n.trim();
        if t.is_empty() {
            return Err(api_err(
                StatusCode::BAD_REQUEST,
                "Credential name cannot be empty.",
            ));
        }
        if t.len() > MAX_CREDENTIAL_NAME_LEN {
            return Err(api_err(
                StatusCode::BAD_REQUEST,
                "Credential name is too long (max 256 characters).",
            ));
        }
    }
    if let Some(ref ex) = data.extra {
        if ex.len() > MAX_CREDENTIAL_EXTRA_LEN {
            return Err(api_err(
                StatusCode::BAD_REQUEST,
                "Credential extra is too long (max 16 KiB).",
            ));
        }
    }
    if let Some(ref sec) = data.secret {
        if sec.len() > MAX_CREDENTIAL_SECRET_BYTES {
            return Err(api_err(
                StatusCode::BAD_REQUEST,
                "Credential secret is too large (max 512 KiB).",
            ));
        }
    }
    Ok(())
}

/// rust_embed keys must be relative, single-file paths (no `..`, no absolute).
#[cfg(feature = "embedded-static")]
fn safe_embedded_asset_path(path: &str) -> bool {
    let p = path.trim();
    if p.is_empty() {
        return false;
    }
    if p.contains("..") {
        return false;
    }
    if p.starts_with('/') || p.starts_with('\\') {
        return false;
    }
    // Windows drive letters / URL-style schemes in path keys
    if p.contains(':') {
        return false;
    }
    true
}

async fn serve_index(State(state): State<AppState>) -> Response {
    if let Some(dir) = &state.static_dir {
        let path = dir.join("index.html");
        match std::fs::read_to_string(&path) {
            Ok(html) => Html(html).into_response(),
            Err(_) => (StatusCode::NOT_FOUND, "index.html not found").into_response(),
        }
    } else {
        #[cfg(feature = "embedded-static")]
        {
            match EmbeddedStatic::get("index.html") {
                Some(f) => {
                    let html = String::from_utf8_lossy(f.data.as_ref()).into_owned();
                    Html(html).into_response()
                }
                None => (StatusCode::NOT_FOUND, "index.html not found").into_response(),
            }
        }
        #[cfg(not(feature = "embedded-static"))]
        {
            (StatusCode::INTERNAL_SERVER_ERROR, "embedded static not enabled").into_response()
        }
    }
}

#[cfg(feature = "embedded-static")]
fn embedded_file_response(path: &str) -> Response {
    if !safe_embedded_asset_path(path) {
        return (StatusCode::FORBIDDEN, "invalid path").into_response();
    }
    match EmbeddedStatic::get(path) {
        Some(f) => {
            let mime = mime_for_path(path);
            match Response::builder()
                .status(StatusCode::OK)
                .header(http_header::CONTENT_TYPE, mime)
                .body(Body::from(f.data.into_owned()))
            {
                Ok(resp) => resp.into_response(),
                Err(_) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to build response",
                )
                    .into_response(),
            }
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

#[cfg(feature = "embedded-static")]
async fn serve_embedded_static(Path(path): Path<String>) -> Response {
    embedded_file_response(path.trim_start_matches('/'))
}

#[cfg(feature = "embedded-static")]
async fn serve_embedded_fallback(uri: Uri, method: Method) -> Response {
    if method != Method::GET {
        return (StatusCode::METHOD_NOT_ALLOWED, "method not allowed").into_response();
    }
    let p = uri.path().trim_start_matches('/').trim_end_matches('/');
    if !safe_embedded_asset_path(p) {
        return (StatusCode::FORBIDDEN, "invalid path").into_response();
    }
    if p.starts_with("api") {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    if p.is_empty() {
        return (StatusCode::FOUND, [(http_header::LOCATION, "/")]).into_response();
    }
    match EmbeddedStatic::get(p) {
        Some(f) => {
            let mime = mime_for_path(p);
            match Response::builder()
                .status(StatusCode::OK)
                .header(http_header::CONTENT_TYPE, mime)
                .body(Body::from(f.data.into_owned()))
            {
                Ok(resp) => resp.into_response(),
                Err(_) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to build response",
                )
                    .into_response(),
            }
        }
        None => match EmbeddedStatic::get("index.html") {
            Some(f) => {
                let html = String::from_utf8_lossy(f.data.as_ref()).into_owned();
                Html(html).into_response()
            }
            None => (StatusCode::NOT_FOUND, "not found").into_response(),
        },
    }
}

// --- Projects ---
async fn list_projects(State(state): State<AppState>) -> Result<Json<Vec<ProjectRead>>, Response> {
    let list = crud::get_projects(&state.db);
    let out: Vec<ProjectRead> = list
        .into_iter()
        .map(|p| ProjectRead {
            id: p.0,
            name: p.1,
            description: p.2,
            git_url: p.3,
            git_branch: p.4,
            git_credential_id: p.5,
            created_at: p.6,
            updated_at: p.7,
        })
        .collect();
    Ok(Json(out))
}

async fn get_project(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Json<ProjectRead>, Response> {
    let p = crud::get_project(&state.db, id).ok_or_else(|| api_err(StatusCode::NOT_FOUND, "Project not found"))?;
    Ok(Json(ProjectRead {
        id: p.0,
        name: p.1,
        description: p.2,
        git_url: p.3,
        git_branch: p.4,
        git_credential_id: p.5,
        created_at: p.6,
        updated_at: p.7,
    }))
}

async fn create_project(State(state): State<AppState>, Json(data): Json<ProjectCreate>) -> Result<Json<ProjectRead>, Response> {
    if let Some(ref b) = data.git_branch {
        if let Err(e) = git_support::validate_branch(b) {
            return Err(api_err(StatusCode::BAD_REQUEST, &e));
        }
    }
    // Credentials are scoped to a project_id; the new project has no id yet — set Git credential after create via Edit.
    if data.git_credential_id.is_some() {
        return Err(api_err(
            StatusCode::BAD_REQUEST,
            "Cannot set Git credential when creating a project. Create the project, then edit it to attach a credential.",
        ));
    }
    crud::create_project(&state.db, &data).map(Json).map_err(|_| api_err(StatusCode::INTERNAL_SERVER_ERROR, "Failed to create project"))
}

async fn update_project(State(state): State<AppState>, Path(id): Path<i64>, Json(data): Json<ProjectUpdate>) -> Result<Json<ProjectRead>, Response> {
    if let Some(ref b) = data.git_branch {
        if let Err(e) = git_support::validate_branch(b) {
            return Err(api_err(StatusCode::BAD_REQUEST, &e));
        }
    }
    if let Some(cid) = data.git_credential_id {
        let cred = crud::get_credential(&state.db, cid).ok_or_else(|| api_err(StatusCode::NOT_FOUND, "Credential not found"))?;
        if cred.project_id != id {
            return Err(api_err(StatusCode::BAD_REQUEST, "Credential does not belong to this project"));
        }
    }
    crud::update_project(&state.db, id, &data).ok_or_else(|| api_err(StatusCode::NOT_FOUND, "Project not found")).map(Json)
}

async fn delete_project(State(state): State<AppState>, Path(id): Path<i64>) -> Result<StatusCode, Response> {
    if !crud::delete_project(&state.db, id) {
        return Err(api_err(StatusCode::NOT_FOUND, "Project not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn pull_project(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Json<PullResponse>, Response> {
    let p = crud::get_project(&state.db, id).ok_or_else(|| api_err(StatusCode::NOT_FOUND, "Project not found"))?;
    let git_url = p.3.as_ref().filter(|s| !s.trim().is_empty()).ok_or_else(|| api_err(StatusCode::BAD_REQUEST, "Project has no Git URL. Set Git repo URL in project settings."))?;
    let mut ssh_key = None;
    let mut https_token = None;
    if let Some(cred_id) = p.5 {
        if let Some(cred) = crud::get_credential(&state.db, cred_id) {
            if cred.project_id != p.0 {
                return Err(api_err(
                    StatusCode::BAD_REQUEST,
                    "Git credential does not belong to this project",
                ));
            }
            if let Some(secret) = crud::get_credential_secret(&state.db, cred_id) {
                if cred.kind == "ssh" {
                    ssh_key = Some(secret);
                } else if cred.kind == "git" {
                    https_token = Some(secret);
                }
            }
        }
    }
    let repo_path = git_support::clone_or_pull(
        id,
        git_url,
        p.4.as_deref().unwrap_or("main"),
        ssh_key.as_deref(),
        https_token.as_deref(),
    ).map_err(|e| api_err(StatusCode::BAD_REQUEST, &e))?;
    let playbooks = git_support::list_playbooks_in_repo(&repo_path);
    Ok(Json(PullResponse { ok: true, message: "Pulled successfully.".into(), playbooks }))
}

#[derive(serde::Serialize)]
struct ProjectPlaybooksResponse {
    /// Under `workspace/project_<id>/` (Git clone).
    workspace: Vec<String>,
    /// Under the server process current working directory (optional local files).
    cwd: Vec<String>,
}

async fn get_project_playbooks(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ProjectPlaybooksResponse>, Response> {
    let workspace = playbook_discovery::list_playbooks_for_project(&state.db, id).map_err(|e| {
        match e {
            playbook_discovery::PlaybookListError::ProjectNotFound => {
                api_err(StatusCode::NOT_FOUND, "Project not found")
            }
            playbook_discovery::PlaybookListError::Io(msg) => {
                api_err(StatusCode::INTERNAL_SERVER_ERROR, &msg)
            }
        }
    })?;
    let cwd = playbook_discovery::list_playbooks_from_cwd().unwrap_or_else(|e| {
        tracing::warn!(error = %e, "list_playbooks_from_cwd failed; returning empty cwd list");
        vec![]
    });
    Ok(Json(ProjectPlaybooksResponse { workspace, cwd }))
}

#[derive(serde::Deserialize)]
struct SshScanBody {
    cidr: String,
}

#[derive(serde::Serialize)]
struct SshScanResponse {
    hosts: Vec<ssh_deployer::ScanHost>,
}

async fn ssh_deployer_scan(Json(body): Json<SshScanBody>) -> Result<Json<SshScanResponse>, Response> {
    let _permit = SSH_SCAN_SEM
        .acquire()
        .await
        .map_err(|_| api_err(StatusCode::INTERNAL_SERVER_ERROR, "scan lock unavailable"))?;
    let cidr = body.cidr;
    let hosts = tokio::task::spawn_blocking(move || ssh_deployer::scan_cidr(&cidr))
        .await
        .map_err(|_| api_err(StatusCode::INTERNAL_SERVER_ERROR, "scan task failed"))?
        .map_err(|e| api_err(StatusCode::BAD_REQUEST, &e))?;
    Ok(Json(SshScanResponse { hosts }))
}

#[derive(serde::Deserialize)]
struct SshPubKeyBody {
    credential_id: i64,
    /// Must match the credential’s project (mitigates blind credential-id probing).
    project_id: i64,
}

#[derive(serde::Serialize)]
struct SshPubKeyResponse {
    public_key: String,
}

async fn ssh_deployer_public_key(
    State(state): State<AppState>,
    Json(body): Json<SshPubKeyBody>,
) -> Result<Json<SshPubKeyResponse>, Response> {
    let db = state.db.clone();
    let cred_id = body.credential_id;
    let project_id = body.project_id;
    let public_key = tokio::task::spawn_blocking(move || {
        ssh_deployer::ssh_public_key_for_credential(&db, cred_id, project_id)
    })
    .await
    .map_err(|_| api_err(StatusCode::INTERNAL_SERVER_ERROR, "task failed"))?
    .map_err(|e| {
        if matches!(e.as_str(), "Project not found" | "Credential not found") {
            api_err(StatusCode::NOT_FOUND, &e)
        } else {
            api_err(StatusCode::BAD_REQUEST, &e)
        }
    })?;
    Ok(Json(SshPubKeyResponse { public_key }))
}

#[derive(serde::Serialize)]
struct GenerateKeypairResponse {
    public_key: String,
    private_key_openssh: String,
}

async fn ssh_deployer_generate_keypair() -> Result<Json<GenerateKeypairResponse>, Response> {
    let (public_key, private_key_openssh) =
        tokio::task::spawn_blocking(ssh_deployer::generate_ed25519_keypair)
            .await
            .map_err(|_| api_err(StatusCode::INTERNAL_SERVER_ERROR, "task failed"))?
            .map_err(|e| api_err(StatusCode::BAD_REQUEST, &e))?;
    Ok(Json(GenerateKeypairResponse {
        public_key,
        private_key_openssh,
    }))
}

#[derive(serde::Deserialize)]
struct DeployPubkeyBody {
    project_id: i64,
    /// Saved credential (SSH key or stored password). Omit when using one-time password.
    #[serde(default)]
    credential_id: Option<i64>,
    ips: Vec<String>,
    public_key: String,
    /// One-time SSH login (not stored). Both required together; requires `sshpass` on the Ansible server.
    #[serde(default)]
    ephemeral_username: Option<String>,
    #[serde(default)]
    ephemeral_password: Option<String>,
}

#[derive(serde::Serialize)]
struct DeployPubkeyResponse {
    results: Vec<ssh_deployer::DeployKeyResult>,
}

async fn ssh_deployer_deploy_pubkey(
    State(state): State<AppState>,
    Json(body): Json<DeployPubkeyBody>,
) -> Result<Json<DeployPubkeyResponse>, Response> {
    if body.project_id <= 0 {
        return Err(api_err(
            StatusCode::BAD_REQUEST,
            "Invalid project_id.",
        ));
    }
    if body.ips.len() > 32 {
        return Err(api_err(
            StatusCode::BAD_REQUEST,
            "Too many ips in request (max 32 per deploy).",
        ));
    }

    let has_user = body
        .ephemeral_username
        .as_deref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let has_pass = body
        .ephemeral_password
        .as_ref()
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    if has_user || has_pass {
        if !has_user || !has_pass {
            return Err(api_err(
                StatusCode::BAD_REQUEST,
                "One-time password deploy requires both ephemeral_username and ephemeral_password.",
            ));
        }
        if body.credential_id.is_some_and(|id| id > 0) {
            return Err(api_err(
                StatusCode::BAD_REQUEST,
                "Do not send credential_id together with one-time username/password.",
            ));
        }
    }

    let db = state.db.clone();
    let project_id = body.project_id;
    let ips = body.ips;
    let public_key = body.public_key;

    let results = if has_user && has_pass {
        let u = body.ephemeral_username.unwrap_or_default().trim().to_string();
        let p = body.ephemeral_password.unwrap_or_default();
        tokio::task::spawn_blocking(move || {
            ssh_deployer::deploy_public_key_ephemeral_password(
                &db,
                project_id,
                ips,
                &public_key,
                &u,
                &p,
            )
        })
        .await
        .map_err(|_| api_err(StatusCode::INTERNAL_SERVER_ERROR, "task failed"))?
        .map_err(|e| api_err(StatusCode::BAD_REQUEST, &e))?
    } else {
        let credential_id = body.credential_id.filter(|&id| id > 0).ok_or_else(|| {
            api_err(
                StatusCode::BAD_REQUEST,
                "Select a login credential, or use one-time username and password.",
            )
        })?;
        tokio::task::spawn_blocking(move || {
            ssh_deployer::deploy_public_key_to_hosts(
                &db,
                project_id,
                credential_id,
                ips,
                &public_key,
            )
        })
        .await
        .map_err(|_| api_err(StatusCode::INTERNAL_SERVER_ERROR, "task failed"))?
        .map_err(|e| api_err(StatusCode::BAD_REQUEST, &e))?
    };

    Ok(Json(DeployPubkeyResponse { results }))
}

// --- Inventories ---
async fn list_inventories(State(state): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Json<Vec<InventoryRead>> {
    let project_id = q.get("project_id").and_then(|s| s.parse().ok());
    let list = match project_id {
        Some(pid) => crud::get_inventories_by_project(&state.db, pid),
        None => vec![],
    };
    Json(list)
}

async fn get_inventory(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Json<InventoryRead>, Response> {
    crud::get_inventory(&state.db, id).map(Json).ok_or_else(|| api_err(StatusCode::NOT_FOUND, "Inventory not found"))
}

async fn create_inventory(State(state): State<AppState>, Json(data): Json<InventoryCreate>) -> Result<Json<InventoryRead>, Response> {
    if crud::get_project(&state.db, data.project_id).is_none() {
        return Err(api_err(StatusCode::NOT_FOUND, "Project not found"));
    }
    crud::create_inventory(&state.db, &data).map(Json).map_err(|_| api_err(StatusCode::INTERNAL_SERVER_ERROR, "Failed to create inventory"))
}

async fn update_inventory(State(state): State<AppState>, Path(id): Path<i64>, Json(data): Json<InventoryUpdate>) -> Result<Json<InventoryRead>, Response> {
    crud::update_inventory(&state.db, id, &data).ok_or_else(|| api_err(StatusCode::NOT_FOUND, "Inventory not found")).map(Json)
}

async fn delete_inventory(State(state): State<AppState>, Path(id): Path<i64>) -> Result<StatusCode, Response> {
    if !crud::delete_inventory(&state.db, id) {
        return Err(api_err(StatusCode::NOT_FOUND, "Inventory not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}

// --- Credentials ---
async fn list_credentials(State(state): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Json<Vec<CredentialRead>> {
    let project_id = q.get("project_id").and_then(|s| s.parse().ok());
    let list = match project_id {
        Some(pid) => crud::get_credentials_by_project(&state.db, pid),
        None => vec![],
    };
    Json(list)
}

async fn get_credential(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Json<CredentialRead>, Response> {
    crud::get_credential(&state.db, id).map(Json).ok_or_else(|| api_err(StatusCode::NOT_FOUND, "Credential not found"))
}

async fn create_credential(State(state): State<AppState>, Json(mut data): Json<CredentialCreate>) -> Result<Json<CredentialRead>, Response> {
    if crud::get_project(&state.db, data.project_id).is_none() {
        return Err(api_err(StatusCode::NOT_FOUND, "Project not found"));
    }
    validate_credential_create(&mut data)?;
    let kind_owned = data
        .kind
        .clone()
        .unwrap_or_else(|| "ssh".to_string())
        .trim()
        .to_string();
    if !credential_kind_ok(&kind_owned) {
        return Err(api_err(
            StatusCode::BAD_REQUEST,
            "Invalid credential kind. Use: ssh, password, vault, git.",
        ));
    }
    data.kind = Some(kind_owned);
    let enc = secrets::encrypt_secret(&data.secret);
    crud::create_credential(&state.db, &data, &enc).map(Json).map_err(|_| api_err(StatusCode::INTERNAL_SERVER_ERROR, "Failed to create credential"))
}

async fn update_credential(State(state): State<AppState>, Path(id): Path<i64>, Json(mut data): Json<CredentialUpdate>) -> Result<Json<CredentialRead>, Response> {
    validate_credential_update(&data)?;
    if let Some(ref mut n) = data.name {
        *n = n.trim().to_string();
    }
    if let Some(ref k) = data.kind {
        let t = k.trim();
        if !credential_kind_ok(t) {
            return Err(api_err(
                StatusCode::BAD_REQUEST,
                "Invalid credential kind. Use: ssh, password, vault, git.",
            ));
        }
        data.kind = Some(t.to_string());
    }
    let secret_encrypted = data.secret.as_ref().map(|s| secrets::encrypt_secret(s));
    crud::update_credential(&state.db, id, &data, secret_encrypted.as_deref()).ok_or_else(|| api_err(StatusCode::NOT_FOUND, "Credential not found")).map(Json)
}

async fn delete_credential(State(state): State<AppState>, Path(id): Path<i64>) -> Result<StatusCode, Response> {
    if !crud::delete_credential(&state.db, id) {
        return Err(api_err(StatusCode::NOT_FOUND, "Credential not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}

// --- Job templates ---
async fn list_job_templates(State(state): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Json<Vec<JobTemplateRead>> {
    let project_id = q.get("project_id").and_then(|s| s.parse().ok());
    let list = match project_id {
        Some(pid) => crud::get_job_templates_by_project(&state.db, pid),
        None => vec![],
    };
    Json(list)
}

async fn get_job_template(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Json<JobTemplateRead>, Response> {
    crud::get_job_template(&state.db, id).map(Json).ok_or_else(|| api_err(StatusCode::NOT_FOUND, "Job template not found"))
}

async fn get_next_run(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Json<NextRunResponse>, Response> {
    let jt = crud::get_job_template(&state.db, id).ok_or_else(|| api_err(StatusCode::NOT_FOUND, "Job template not found"))?;
    let next = jt.schedule_enabled
        .then_some(jt.schedule_cron.as_ref())
        .flatten()
        .filter(|s| !s.is_empty())
        .and_then(|cron| crate::scheduler::next_run_iso(cron, jt.schedule_tz.as_deref().unwrap_or("UTC")));
    Ok(Json(NextRunResponse { next_run: next }))
}

async fn create_job_template(State(state): State<AppState>, Json(data): Json<JobTemplateCreate>) -> Result<Json<JobTemplateRead>, Response> {
    validate_schedule_fields(data.schedule_cron.as_ref(), data.schedule_tz.as_ref())?;
    if crud::get_project(&state.db, data.project_id).is_none() {
        return Err(api_err(StatusCode::NOT_FOUND, "Project not found"));
    }
    if let Some(iid) = data.inventory_id {
        let inv = crud::get_inventory(&state.db, iid).ok_or_else(|| api_err(StatusCode::NOT_FOUND, "Inventory not found"))?;
        if inv.project_id != data.project_id {
            return Err(api_err(StatusCode::BAD_REQUEST, "Inventory does not belong to this project"));
        }
    }
    if let Some(cid) = data.credential_id {
        let cred = crud::get_credential(&state.db, cid).ok_or_else(|| api_err(StatusCode::NOT_FOUND, "Credential not found"))?;
        if cred.project_id != data.project_id {
            return Err(api_err(StatusCode::BAD_REQUEST, "Credential does not belong to this project"));
        }
    }
    crud::create_job_template(&state.db, &data).map(Json).map_err(|_| api_err(StatusCode::INTERNAL_SERVER_ERROR, "Failed to create job template"))
}

async fn update_job_template(State(state): State<AppState>, Path(id): Path<i64>, Json(data): Json<JobTemplateUpdate>) -> Result<Json<JobTemplateRead>, Response> {
    validate_schedule_fields(data.schedule_cron.as_ref(), data.schedule_tz.as_ref())?;
    let jt = crud::get_job_template(&state.db, id).ok_or_else(|| api_err(StatusCode::NOT_FOUND, "Job template not found"))?;
    if let Some(cid) = data.credential_id.or(jt.credential_id) {
        let cred = crud::get_credential(&state.db, cid).ok_or_else(|| api_err(StatusCode::NOT_FOUND, "Credential not found"))?;
        if cred.project_id != jt.project_id {
            return Err(api_err(StatusCode::BAD_REQUEST, "Credential does not belong to this project"));
        }
    }
    let effective_inventory = data.inventory_id.or(jt.inventory_id);
    if let Some(iid) = effective_inventory {
        let inv = crud::get_inventory(&state.db, iid).ok_or_else(|| api_err(StatusCode::NOT_FOUND, "Inventory not found"))?;
        if inv.project_id != jt.project_id {
            return Err(api_err(StatusCode::BAD_REQUEST, "Inventory does not belong to this project"));
        }
    }
    crud::update_job_template(&state.db, id, &data).ok_or_else(|| api_err(StatusCode::NOT_FOUND, "Job template not found")).map(Json)
}

async fn delete_job_template(State(state): State<AppState>, Path(id): Path<i64>) -> Result<StatusCode, Response> {
    if !crud::delete_job_template(&state.db, id) {
        return Err(api_err(StatusCode::NOT_FOUND, "Job template not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}

// --- Jobs ---
async fn list_jobs(State(state): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Json<Vec<JobListSummary>> {
    let project_id = q.get("project_id").and_then(|s| s.parse().ok());
    let limit = q.get("limit").and_then(|s| s.parse().ok()).unwrap_or(100).clamp(1, 500);
    let list = match project_id {
        Some(pid) => crud::get_jobs_by_project(&state.db, pid, limit),
        None => crud::get_recent_jobs(&state.db, limit),
    };
    Json(list)
}

async fn get_job(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Json<JobRead>, Response> {
    crud::get_job(&state.db, id).map(Json).ok_or_else(|| api_err(StatusCode::NOT_FOUND, "Job not found"))
}

async fn delete_job(State(state): State<AppState>, Path(id): Path<i64>) -> Result<StatusCode, Response> {
    if !crud::delete_job(&state.db, id) {
        return Err(api_err(StatusCode::NOT_FOUND, "Job not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Resolved playbook path, inventory/extra text, and decrypted credential material for a job run.
struct ResolvedPlaybookRun {
    playbook_path: String,
    inventory_content: String,
    extra_vars: String,
    ssh_key: Option<String>,
    ssh_password: Option<String>,
    vault_pass: Option<String>,
    credential_extra: String,
}

fn resolve_playbook_and_creds(
    db: &DbPool,
    jt: &JobTemplateRead,
    inv_content: &str,
    extra: &str,
) -> Result<ResolvedPlaybookRun, String> {
    let project = crud::get_project(db, jt.project_id).ok_or("Project not found")?;
    let mut playbook_path = jt.playbook_path.clone();
    let inv_content = inv_content.to_string();
    let extra = extra.to_string();

    if let Some(ref git_url) = project.3 {
        if !git_url.trim().is_empty() {
            let mut ssh_key = None;
            let mut https_token = None;
            if let Some(cred_id) = project.5 {
                if let Some(cred) = crud::get_credential(db, cred_id) {
                    if cred.project_id != project.0 {
                        return Err("Git credential does not belong to this project.".into());
                    }
                    if let Some(secret) = crud::get_credential_secret(db, cred_id) {
                        if cred.kind == "ssh" {
                            ssh_key = Some(secret);
                        } else if cred.kind == "git" {
                            https_token = Some(secret);
                        }
                    }
                }
            }
            let repo_path = git_support::clone_or_pull(
                project.0,
                git_url,
                project.4.as_deref().unwrap_or("main"),
                ssh_key.as_deref(),
                https_token.as_deref(),
            )?;
            let candidate = repo_path.join(playbook_path.trim_start_matches(['/', '\\']));
            let candidate = candidate.canonicalize().map_err(|e| e.to_string())?;
            let repo_abs = repo_path.canonicalize().map_err(|e| e.to_string())?;
            if !candidate.starts_with(repo_abs) {
                return Err("Playbook path escapes the repository directory.".into());
            }
            playbook_path = candidate.to_string_lossy().to_string();
        }
    }

    if !std::path::Path::new(&playbook_path).is_absolute() {
        playbook_path = std::fs::canonicalize(&playbook_path).unwrap_or_else(|_| PathBuf::from(&playbook_path)).to_string_lossy().to_string();
    }

    // Restrict playbook to server working directory (prevents arbitrary path execution for local projects).
    let playbook_abs_buf = PathBuf::from(&playbook_path);
    if let Ok(playbook_canon) = playbook_abs_buf.canonicalize() {
        if let Ok(cwd) = std::env::current_dir() {
            if let Ok(allowed_root) = cwd.canonicalize() {
                if !playbook_canon.starts_with(&allowed_root) {
                    return Err("Playbook path is outside the allowed directory.".into());
                }
            }
        }
    }

    let mut ssh_key = None;
    let mut ssh_password = None;
    let mut vault_pass = None;
    let mut credential_extra = String::new();
    if let Some(cred_id) = jt.credential_id {
        if let Some(cred) = crud::get_credential(db, cred_id) {
            if cred.project_id != jt.project_id {
                return Err("Credential does not belong to this project.".into());
            }
            credential_extra = cred.extra.clone();
            if let Some(secret) = crud::get_credential_secret(db, cred_id) {
                match cred.kind.as_str() {
                    "ssh" => ssh_key = Some(secret),
                    "password" => ssh_password = Some(secret),
                    "vault" => vault_pass = Some(secret),
                    _ => {}
                }
            }
        }
    }

    Ok(ResolvedPlaybookRun {
        playbook_path,
        inventory_content: inv_content,
        extra_vars: extra,
        ssh_key,
        ssh_password,
        vault_pass,
        credential_extra,
    })
}

pub fn launch_job_template_by_id_impl(db: &DbPool, job_template_id: i64) -> Option<impl FnOnce() + Send> {
    let jt = crud::get_job_template(db, job_template_id)?;
    let inv_content = jt.inventory_id
        .and_then(|iid| crud::get_inventory(db, iid))
        .map(|i| i.content)
        .unwrap_or_default();
    let extra = jt.extra_vars.clone();
    let r = resolve_playbook_and_creds(db, &jt, &inv_content, &extra).ok()?;
    let job = crud::create_job(
        db,
        jt.project_id,
        Some(jt.id),
        &r.playbook_path,
        &r.inventory_content,
        &r.extra_vars,
        "pending",
    )
    .ok()?;
    let job_id = job.id;
    let db2 = db.clone();
    let playbook_path = r.playbook_path.clone();
    let inventory_content = r.inventory_content.clone();
    let extra_vars = r.extra_vars.clone();
    let ssh_key = r.ssh_key.clone();
    let ssh_password = r.ssh_password.clone();
    let vault_pass = r.vault_pass.clone();
    let credential_extra = r.credential_extra.clone();
    Some(move || {
        let _ = runner::run_playbook(runner::PlaybookRunParams {
            db: &db2,
            job_id,
            playbook_path: &playbook_path,
            inventory_content: &inventory_content,
            extra_vars: &extra_vars,
            ssh_key: ssh_key.as_deref(),
            ssh_password: ssh_password.as_deref(),
            vault_password: vault_pass.as_deref(),
            credential_extra: &credential_extra,
        });
    })
}

async fn launch_job(State(state): State<AppState>, Json(data): Json<JobLaunch>) -> Result<Json<JobRead>, Response> {
    let jt = crud::get_job_template(&state.db, data.job_template_id).ok_or_else(|| api_err(StatusCode::NOT_FOUND, "Job template not found"))?;
    let inv_content = jt.inventory_id
        .and_then(|iid| crud::get_inventory(&state.db, iid))
        .map(|i| i.content)
        .unwrap_or_default();
    let extra = data.extra_vars_override.as_ref().filter(|s| !s.trim().is_empty()).cloned().unwrap_or_else(|| jt.extra_vars.clone());
    let resolved = resolve_playbook_and_creds(&state.db, &jt, &inv_content, &extra)
        .map_err(|e| api_err(StatusCode::BAD_REQUEST, &e))?;
    let job = crud::create_job(
        &state.db,
        jt.project_id,
        Some(jt.id),
        &resolved.playbook_path,
        &resolved.inventory_content,
        &resolved.extra_vars,
        "pending",
    )
    .map_err(|_| api_err(StatusCode::INTERNAL_SERVER_ERROR, "Failed to create job"))?;
    let job_id = job.id;
    let db_clone = state.db.clone();
    let playbook_path2 = resolved.playbook_path.clone();
    let inv_content2 = resolved.inventory_content.clone();
    let extra2 = resolved.extra_vars.clone();
    let ssh_key = resolved.ssh_key.clone();
    let ssh_password = resolved.ssh_password.clone();
    let vault_pass = resolved.vault_pass.clone();
    let credential_extra2 = resolved.credential_extra.clone();
    std::thread::spawn(move || {
        let _ = runner::run_playbook(runner::PlaybookRunParams {
            db: &db_clone,
            job_id,
            playbook_path: &playbook_path2,
            inventory_content: &inv_content2,
            extra_vars: &extra2,
            ssh_key: ssh_key.as_deref(),
            ssh_password: ssh_password.as_deref(),
            vault_password: vault_pass.as_deref(),
            credential_extra: &credential_extra2,
        });
    });
    crud::get_job(&state.db, job.id).map(Json).ok_or_else(|| api_err(StatusCode::INTERNAL_SERVER_ERROR, "Job not found"))
}

fn cors_relax_lan() -> bool {
    matches!(
        std::env::var("ANSIBLE_UI_RELAX_CORS").ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE")
    )
}

fn cors_layer() -> CorsLayer {
    let methods = [
        axum::http::Method::GET,
        axum::http::Method::POST,
        axum::http::Method::PATCH,
        axum::http::Method::DELETE,
        axum::http::Method::OPTIONS,
    ];
    let headers = [axum::http::header::CONTENT_TYPE];

    if cors_relax_lan() {
        tracing::warn!(
            "ANSIBLE_UI_RELAX_CORS is enabled: CORS allows any Origin. Use only on trusted networks or behind a firewall."
        );
        return CorsLayer::new()
            .allow_origin(AllowOrigin::any())
            .allow_methods(methods)
            .allow_headers(headers);
    }

    let mut allowed_origins: Vec<HeaderValue> = [
        "http://127.0.0.1:14300",
        "http://localhost:14300",
        "null",
    ]
    .iter()
    .filter_map(|s| HeaderValue::try_from(*s).ok())
    .collect();
    if let Ok(extra) = std::env::var("ANSIBLE_UI_EXTRA_ORIGINS") {
        for o in extra.split(',') {
            let o = o.trim();
            // AllowOrigin::list panics on wildcard; reject empty and "*"
            if o.is_empty() || o == "*" {
                continue;
            }
            if let Ok(h) = HeaderValue::try_from(o) {
                allowed_origins.push(h);
            }
        }
    }
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(allowed_origins))
        .allow_methods(methods)
        .allow_headers(headers)
}

/// API routes only (nested under `/api`).
fn api_routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/projects", get(list_projects).post(create_project))
        // matchit 0.7 (used by axum 0.7) requires `:param`, not `{param}` — braces are literal.
        .route("/projects/:id/pull", post(pull_project))
        .route("/projects/:id/playbooks", get(get_project_playbooks))
        .route("/projects/:id", get(get_project).patch(update_project).delete(delete_project))
        .route("/ssh_deployer/scan", post(ssh_deployer_scan))
        .route("/ssh_deployer/public_key", post(ssh_deployer_public_key))
        .route(
            "/ssh_deployer/generate_keypair",
            post(ssh_deployer_generate_keypair),
        )
        .route("/ssh_deployer/deploy_pubkey", post(ssh_deployer_deploy_pubkey))
        .route("/inventories", get(list_inventories).post(create_inventory))
        .route("/inventories/:id", get(get_inventory).patch(update_inventory).delete(delete_inventory))
        .route("/credentials", get(list_credentials).post(create_credential))
        .route("/credentials/:id", get(get_credential).patch(update_credential).delete(delete_credential))
        .route("/job_templates", get(list_job_templates).post(create_job_template))
        .route("/job_templates/:id", get(get_job_template).patch(update_job_template).delete(delete_job_template))
        .route("/job_templates/:id/next_run", get(get_next_run))
        .route("/jobs", get(list_jobs))
        .route("/jobs/launch", post(launch_job))
        .route("/jobs/:id", get(get_job).delete(delete_job))
        .with_state(state)
}

const BODY_LIMIT_BYTES: usize = 2 * 1024 * 1024;
const MAX_SCHEDULE_CRON_LEN: usize = 256;
const MAX_SCHEDULE_TZ_LEN: usize = 64;

#[allow(clippy::result_large_err)]
fn validate_schedule_fields(
    cron: Option<&String>,
    tz: Option<&String>,
) -> Result<(), Response> {
    if let Some(c) = cron {
        if c.len() > MAX_SCHEDULE_CRON_LEN {
            return Err(api_err(
                StatusCode::BAD_REQUEST,
                "schedule_cron is too long.",
            ));
        }
    }
    if let Some(t) = tz {
        if t.len() > MAX_SCHEDULE_TZ_LEN {
            return Err(api_err(
                StatusCode::BAD_REQUEST,
                "schedule_tz is too long.",
            ));
        }
    }
    Ok(())
}

pub fn app(source: StaticSource, db: DbPool) -> Router {
    let cors = cors_layer();
    match source {
        #[cfg(feature = "embedded-static")]
        StaticSource::Embedded => {
            let state = AppState {
                db,
                static_dir: None,
            };
            let api = api_routes(state.clone());
            // Catch-all must be the entire tail of its route template. `/static/*path` panics in
            // matchit ("only allowed at the end of a route"); nest so the inner route is `/*path`.
            let embedded_static = Router::new().route("/*path", get(serve_embedded_static));
            Router::new()
                .route("/", get(serve_index))
                .nest("/api", api)
                .nest("/static", embedded_static)
                .layer(RequestBodyLimitLayer::new(BODY_LIMIT_BYTES))
                .layer(SetResponseHeaderLayer::if_not_present(
                    header::X_CONTENT_TYPE_OPTIONS,
                    HeaderValue::from_static("nosniff"),
                ))
                .layer(SetResponseHeaderLayer::if_not_present(
                    header::X_FRAME_OPTIONS,
                    HeaderValue::from_static("SAMEORIGIN"),
                ))
                .layer(cors)
                .with_state(state)
                .fallback(get(serve_embedded_fallback))
        }
        StaticSource::Filesystem(static_dir) => {
            let state = AppState {
                db,
                static_dir: Some(static_dir.clone()),
            };
            let api = api_routes(state.clone());
            let static_service = ServeDir::new(static_dir);
            Router::new()
                .route("/", get(serve_index))
                .nest_service("/static", static_service.clone())
                .nest("/api", api)
                .layer(RequestBodyLimitLayer::new(BODY_LIMIT_BYTES))
                .layer(SetResponseHeaderLayer::if_not_present(
                    header::X_CONTENT_TYPE_OPTIONS,
                    HeaderValue::from_static("nosniff"),
                ))
                .layer(SetResponseHeaderLayer::if_not_present(
                    header::X_FRAME_OPTIONS,
                    HeaderValue::from_static("SAMEORIGIN"),
                ))
                .layer(cors)
                .with_state(state)
                .fallback_service(static_service)
        }
    }
}
