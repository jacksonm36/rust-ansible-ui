# Security and Bug Audit — Vibe-Coded Tauri

**Date:** 2025-03-17  
**Scope:** `vibe-coded-tauri` (Rust backend + static frontend)

---

## Executive summary

- **Critical:** One bug can cause a **panic** when the inventory temp file cannot be created (fixed in code).
- **High:** No authentication on the API; CORS allows any origin; default secret key is weak if env is unset.
- **Medium:** Playbook path for non-Git projects is not restricted to a safe directory; scheduler timezone ignored; script runs have no timeout.
- **Low:** Host key checking disabled for Ansible; credential cross-project use not validated.

---

## Critical

### 1. Panic when inventory temp file creation fails (BUG — FIXED)

**File:** `src-tauri/src/runner.rs`  
**Issue:** `NamedTempFile::with_suffix(".ini")` can fail (e.g. no temp dir, permissions). The code stores the result in an `Option` and later uses `inv_file.as_ref().unwrap()` when building `args`, which panics if the option is `None`.

**Fix:** Create the inventory temp file with `match`; on failure, set job status to `"failed"` and return an error message. Use the created file’s path in `args` so no `unwrap()` is needed.

---

## High (security)

### 2. No authentication on API

**Files:** `server.rs`, all API routes  
**Issue:** All endpoints (projects, inventories, credentials, jobs, templates) are unauthenticated. Anyone who can reach the server can read and change data, including decrypted credentials.

**Recommendation:** Add authentication (e.g. API key header, session cookie, or basic auth) and enforce it on every API route. For a desktop-only app on localhost this is less critical but still recommended if the UI is ever exposed.

### 3. CORS allows any origin

**File:** `src-tauri/src/server.rs`  
**Issue:** CORS is configured to allow any origin (e.g. `Layer::from(CorsLayer::permissive())` or equivalent). Any website can call the API if the user has the server running.

**Recommendation:** Restrict to the Tauri origin and/or `http://127.0.0.1:14300` (and the exact port used).

### 4. Weak default secret key

**File:** `src-tauri/src/secrets.rs`  
**Issue:** If `ANSIBLE_UI_SECRET_KEY` is unset or short, key derivation may be predictable (e.g. DefaultHasher or short salt). Encrypted credentials could be easier to attack.

**Recommendation:** Require a long, random `ANSIBLE_UI_SECRET_KEY` in production (e.g. 32+ bytes); refuse to start or to encrypt if not set. Document this in README/deployment.

---

## Medium

### 5. Playbook path not restricted (non-Git projects)

**Files:** `server.rs`, `runner.rs`  
**Issue:** For Git projects, playbook path is validated to be under the repo directory. For non-Git (local) projects, `playbook_path` is canonicalized and run without a whitelist. A user (or compromised UI) could set a path to any file on the system and execute it as a playbook/script.

**Recommendation:** For local projects, restrict execution to a configured project root (e.g. a single “playbooks” or project directory) and ensure the canonical path is under that root.

### 6. Scheduler timezone not applied

**File:** `src-tauri/src/scheduler.rs`  
**Issue:** Cron next-run is computed with `schedule.upcoming(Utc).next()`. The stored `schedule_tz` (e.g. `"America/New_York"`) is not used, so “next run” is in UTC only and may not match user expectations.

**Recommendation:** Use a timezone-aware cron library or convert `Utc::now()` to the user’s timezone before computing the next occurrence, and store/display next run in that timezone.

### 7. Script runs have no timeout

**File:** `src-tauri/src/runner.rs`  
**Issue:** `run_script()` accepts `_timeout_secs` but does not use it; scripts can run indefinitely.

**Recommendation:** Enforce a timeout (e.g. spawn with a timeout or use a process wrapper that kills after N seconds).

### 8. Job timeout only for playbook, not script

**File:** `src-tauri/src/runner.rs`  
**Issue:** The 3600s timeout is applied to the Ansible playbook path via `rx.recv_timeout`. Script execution has no timeout.

**Recommendation:** Apply a configurable timeout to script runs as well (see #7).

---

## Low

### 9. Ansible host key checking disabled

**File:** `src-tauri/src/runner.rs`  
**Issue:** `ANSIBLE_HOST_KEY_CHECKING=False` is set, which disables SSH host key verification and can facilitate MITM if an attacker can redirect SSH connections.

**Recommendation:** Prefer enabling host key checking and documenting how to add known hosts; or make this configurable per job/project.

### 10. Credential ID not scoped to project

**Files:** `crud.rs`, `server.rs`  
**Issue:** When running a job, the credential is loaded by ID without checking that it belongs to the same project or tenant. In a multi-project setup, project A could theoretically use project B’s credential if IDs are known.

**Recommendation:** Validate that the credential is associated with the job’s project (or with a global credential pool) before use.

### 11. Frontend: modal header uses job.id and job.status

**File:** `static/js/app.js`  
**Issue:** `Job #${job.id}` and `job.status` are interpolated into HTML. `id` is numeric; `status` is server-controlled (`pending`/`running`/`success`/`failed`). Risk is low but for consistency these could be escaped or set via `textContent`.

**Recommendation:** Use `escapeHtml()` for status when setting innerHTML, or set the header via `textContent` for the whole line.

---

## Positive findings

- **SQL:** Parameterized queries used in `crud.rs`; no raw SQL concatenation.
- **XSS:** User-controlled content (playbook_path, output_log, etc.) is passed through `escapeHtml()` in the job modal and tables.
- **Path traversal (Git):** Playbook path is validated to be under the repo path (`candidate.starts_with(repo_abs)`).
- **Secrets:** Credentials encrypted at rest with AES-256-GCM; temp files for keys/vault have restricted permissions on Unix.
- **Git URL:** Passed as a single argument to `git clone`, so no shell injection via URL.

---

## Summary table

| Severity | Count | Addressed |
|----------|--------|-----------|
| Critical | 1     | Yes (panic fix) |
| High     | 3     | Recommendations only |
| Medium   | 4     | Recommendations only |
| Low      | 3     | Recommendations only |

Implementing the high and medium recommendations will significantly improve security and correctness, especially if the server is ever exposed beyond localhost or used in a multi-user environment.
