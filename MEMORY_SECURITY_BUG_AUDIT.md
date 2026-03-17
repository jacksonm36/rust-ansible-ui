# Memory, Code, SQL Injection & Security Audit

**Date:** 2025-03-17  
**Scope:** `vibe-coded-tauri` (Rust backend + static frontend)

---

## Executive summary

| Category           | Finding |
|--------------------|--------|
| **SQL injection**  | None found — all queries use parameterized `params![]` / `?1` |
| **Memory bugs**     | Unbounded job output (DoS/OOM risk); Mutex poison panic possible |
| **Code bugs**       | Scheduler runs 0–90s early; `repo_path.to_str().unwrap()` can panic on non-UTF8 paths; several `.unwrap()` on DB ops |
| **Security**        | No auth; permissive CORS; weak default encryption key; local playbook path not restricted; host key check disabled |

---

## 1. SQL injection

**Status: No vulnerabilities found.**

- **crud.rs:** Every `prepare` / `execute` uses `params![...]` or `params![id]`. No string concatenation or format into SQL.
- **db.rs:** Only DDL and fixed migrations; no user input in SQL.
- **server.rs:** IDs come from Axum `Path(i64)` (parsed as integer; non-numeric 404). Query params (`project_id`, `limit`) are parsed and passed as values to crud.

**Recommendation:** Keep using parameterized queries for any new endpoints.

---

## 2. Memory and resource issues

### 2.1 Unbounded job output (Medium)

**Files:** `runner.rs`, `crud.rs`  
**Issue:** `run_playbook` / `run_script` read full stdout/stderr into a `String` and pass it to `update_job_status`, which writes it to SQLite. A job that produces very large output (e.g. gigabytes) can cause high memory use and DB bloat.

**Recommendation:** Cap output size (e.g. truncate to 1–5 MB) before storing, or stream to a file and store a path/reference.

### 2.2 Mutex poison (Low)

**File:** `crud.rs`  
**Issue:** `fn conn(db: &DbPool) -> MutexGuard<...> { db.lock().unwrap() }` — if a thread panics while holding the lock, the mutex is poisoned and subsequent `.unwrap()` will panic.

**Recommendation:** Use `lock().unwrap_or_else(|e| e.into_inner())` to recover from poison, or handle errors without panicking.

### 2.3 DB panic on error (Low)

**Files:** `crud.rs`  
**Issue:** Many `prepare(...).unwrap()` and `query_map(...).unwrap()` — any DB error (disk full, busy, corrupt) will panic the server.

**Recommendation:** Prefer `?` or `.map_err()` and return HTTP 500 instead of panicking.

---

## 3. Code bugs

### 3.1 Scheduler runs 0–90 seconds early (Medium)

**File:** `scheduler.rs`  
**Issue:** `next_run_utc` uses `schedule.upcoming(Utc).next()`, which is the next *future* run. The check `_now.signed_duration_since(next).num_seconds().abs() <= 90` is true when the next run is 0–90 seconds *ahead*, so the job runs *before* the scheduled time. Intended behavior is usually “run once when we’re within 0–90s *after* the scheduled time.”

**Recommendation:** Use a “previous occurrence” (or last-run time per template) and run when `now` is within 0–90s after that time; or adopt a scheduler that supports “run after” semantics.

### 3.2 Panic on non-UTF8 repo path (Low)

**File:** `git_support.rs` (clone)  
**Issue:** `repo_path.to_str().unwrap()` — on Windows, paths with invalid UTF-8 can panic.

**Recommendation:** Use `repo_path.to_string_lossy().into_owned()` or handle `None` and return an error.

### 3.3 Temp file path display (Low)

**File:** `runner.rs`  
**Issue:** `format!("@{}", f.path().display())` is fine for the process; `.to_string_lossy()` is safer if the path were ever logged or sent to another API.

**Recommendation:** No change required for correctness; optional hardening if paths are ever exposed.

---

## 4. Security vulnerabilities

### 4.1 No authentication (High)

**Files:** `server.rs`, all routes  
**Issue:** All API endpoints are unauthenticated. Anyone who can reach the server can read/change projects, credentials, and run jobs.

**Recommendation:** Add auth (API key, session, or basic auth) and protect all API routes.

### 4.2 CORS allows any origin (High)

**File:** `server.rs`  
**Issue:** `CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any)` — any website can call the API if the user has the server running.

**Recommendation:** Restrict to the app origin(s), e.g. `http://127.0.0.1:14300` and the Tauri custom protocol.

### 4.3 Weak default encryption key (High)

**File:** `secrets.rs`  
**Issue:** If `ANSIBLE_UI_SECRET_KEY` is unset or shorter than 32 bytes, the code uses a deterministic key from `DefaultHasher` and a fixed salt. Credentials at rest are then weak.

**Recommendation:** Require a 32+ byte secret in production; refuse to start or encrypt if not set.

### 4.4 Local playbook path not restricted (Medium)

**Files:** `server.rs`, `runner.rs`  
**Issue:** For Git projects, playbook path is validated to stay under the repo. For non-Git (local) projects, `playbook_path` is only canonicalized; it can point to any file on the system and be executed.

**Recommendation:** For local projects, restrict execution to a configured project root and ensure the resolved path is under it.

### 4.5 Host key checking disabled (Low)

**File:** `runner.rs`  
**Issue:** `ANSIBLE_HOST_KEY_CHECKING=False` disables SSH host key verification and can allow MITM.

**Recommendation:** Prefer enabling it; document how to add known hosts or make it configurable.

### 4.6 Credential not scoped to project (Low)

**Files:** `crud.rs`, `server.rs`  
**Issue:** Job template can reference any credential by ID; there is no check that the credential belongs to the same project.

**Recommendation:** Validate that the credential’s `project_id` matches the job template’s project (or a shared credential pool).

### 4.7 Unbounded request body / input size (Low)

**Files:** `schemas.rs`, `server.rs`  
**Issue:** No max length on JSON body or on string fields (name, description, content, playbook_path, extra_vars, output_log). Very large payloads can cause memory/CPU load.

**Recommendation:** Enforce body size limits in Axum and optional max lengths on string fields in API validation.

### 4.8 XSS — job status in HTML (Low, defense in depth)

**File:** `static/js/app.js`  
**Issue:** `job.status` is interpolated into `innerHTML` in the job modal and tables (e.g. `badge-${j.status}`, header text). Status is currently only set server-side to `pending`/`running`/`success`/`failed`, so risk is low.

**Recommendation:** Use `escapeHtml(job.status)` wherever status is inserted into HTML.

---

## 5. Positive findings

- **SQL:** All DB access is parameterized; no injection.
- **XSS:** User-controlled data (playbook_path, output_log, names, descriptions, etc.) is passed through `escapeHtml()` in the frontend; only `job.status` is not escaped.
- **Path traversal (Git):** Playbook path is checked with `candidate.starts_with(repo_abs)` so it cannot escape the repo.
- **Secrets:** AES-256-GCM; temp credential files have restricted permissions on Unix.
- **Process invocation:** `Command::new("git").args([...])` and similar use array args; no shell, so no shell injection from URL/branch/path.
- **Inventory temp file:** Previously could panic; now handled with `match` and error return (see existing audit).

---

## 6. Summary table

| Severity | Category   | Count |
|----------|------------|-------|
| High     | Security   | 3 (no auth, CORS, weak key) |
| Medium   | Memory/Bug | 2 (unbounded output, scheduler early run) |
| Low      | Bug/Security | 6 (mutex poison, DB unwrap, path UTF-8, host key, credential scope, input size, status XSS) |

Implementing the high and medium items will materially improve security and stability; the low items improve robustness and defense in depth.
