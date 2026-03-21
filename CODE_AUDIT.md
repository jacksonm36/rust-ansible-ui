# Code audit — bugs, security, quality

**Scope:** `src-tauri` (Rust), `static/` (JS), `deploy/`, `scripts/`  
**Date:** 2026-03-21

---

## Fixes applied in this pass

| Area | Issue | Change |
|------|--------|--------|
| **Security** | Job template could reference another project’s **inventory** | Validate `inventory.project_id == template.project_id` on create/update template |
| **Security** | **Git credential** on pull / playbook resolve could be from another project | Require `cred.project_id == project.id` in `pull_project`, `resolve_playbook_and_creds`, and `update_project` |
| **Security** | **Create project** could set `git_credential_id` before a project id exists (ambiguous / cross-project) | Reject `git_credential_id` on `POST /api/projects`; UI creates project then `PATCH` if needed |
| **Security** | **CORS** `ANSIBLE_UI_EXTRA_ORIGINS=*` could panic `AllowOrigin::list` | Skip `*` and empty tokens when parsing extra origins |
| **Security** | Embedded static **fallback** lacked `..` check | Reject paths containing `..` before lookup |
| **UX / Security** | Project modal listed **all** Git creds; new project could pick wrong one | New project: no cred dropdown choices until edit; edit: only creds for that `project_id` |
| **Security** | Predictable **default AES key** when env unset | If `ANSIBLE_UI_SECRET_KEY` unset/short: load keyfile (`ANSIBLE_UI_KEYFILE` or `<db-dir>/ansible_ui_secret.key`); else **generate** 32 random bytes, persist (base64), cache in `OnceLock` |
| **Security / Ops** | Playbook **timeout** did not **kill** the child | `Child` + `try_wait` + `kill()`; scripts use the same path |
| **Correctness** | Scheduler used in-memory last-run | Persist cron **slot** in `schedule_last_fire_utc`; 180s grace window; reset when schedule off or cron/tz changes |

---

## Security (remaining / accepted risk)

| Item | Severity | Notes |
|------|----------|--------|
| **No API authentication** | High | Any client that can reach the port can use the API. Mitigate with firewall, reverse proxy + auth, or VPN. |
| **Encryption key file** | Medium | Back up `ansible_ui_secret.key` (or set `ANSIBLE_UI_SECRET_KEY`). A **new** generated key cannot decrypt credentials encrypted with an old key. |
| **CORS allow-list** | Medium | Defaults + `ANSIBLE_UI_EXTRA_ORIGINS`; **`null`** origin allowed for some local/embed cases — remove if you only trust explicit origins. |
| **Ansible host keys** | Low | Default `ANSIBLE_HOST_KEY_CHECKING=False`; set `True` when acceptable. |
| **Playbook path** | Medium | Non-Git playbooks must stay under process **cwd**; Git playbooks under repo root. |
| **SSRF / Git URL** | Low | URLs go to `git` as a single argv (no shell); still only use trusted URLs. |

---

## Bugs & behavior (known)

| Item | Notes |
|------|--------|
| **Scheduler / TZ** | Cron evaluated in **UTC**; `schedule_tz` does not shift tick times (see `scheduler.rs`). |
| **Timeouts** | `ANSIBLE_UI_SCRIPT_TIMEOUT_SECS`, `ANSIBLE_UI_PLAYBOOK_TIMEOUT_SECS` / `ANSIBLE_UI_JOB_TIMEOUT_SECS` (default 3600s, capped at 7d). |
| **`mime_for_path`** | Lowercases full path string each call (minor alloc); acceptable. |

---

## Code quality

| Item | Notes |
|------|--------|
| **SQL** | CRUD uses parameterized queries — no string-built SQL. |
| **Panics** | DB list paths use `match` instead of `unwrap` where refactored; mutex poison recovered in `conn()`. |
| **Output** | Job logs capped (~2 MiB) before DB write. |
| **Single binary** | `embedded-static` serves UI from memory; no `static/` dir at runtime. |
| **Frontend** | User-controlled strings generally passed through `escapeHtml()`; job `status` escaped in HTML. |

---

## Operational

| Item | Notes |
|------|--------|
| **systemd** | Prefer `ANSIBLE_UI_SECRET_KEY` or a persistent volume for the DB directory (so `ansible_ui_secret.key` survives restarts). |
| **install-linux.sh** | Run as root; review package names per distro; `SKIP_BUILD=1` for prebuilt binary. |

---

## Quick verification

```bash
cd src-tauri
cargo test --no-default-features --features server-only 2>/dev/null || true
cargo check --no-default-features --features "server-only,embedded-static"
```

---

*Re-run this audit after large feature changes.*
