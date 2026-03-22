# Code quality & security review (rust-ansible-ui)

**Scope:** `src-tauri` server (`ansible-server`), static UI, deploy scripts.  
**Last updated:** 2026-03-17 (credential field limits)

---

## Security

### Critical / high

| Topic | Risk | Notes |
|-------|------|--------|
| **No authentication** | **Critical** on any reachable network | Full API access: run playbooks, read job logs, CRUD projects/inventories/credentials metadata, Git pull. Anyone who can reach the HTTP port can abuse the control plane. **Mitigation:** bind to `127.0.0.1` + reverse proxy with **TLS + auth**; or VPN; do not expose `:80`/`:14300` to the public Internet without controls. |
| **RELAX CORS** | **High** when misused | `ANSIBLE_UI_RELAX_CORS=1` allows any browser `Origin`. OK on isolated LAN with firewall; dangerous if the UI is reachable from untrusted clients. |
| **Stored secrets** | **Medium** (expected) | Credentials encrypted at rest (AES-256-GCM) with key from env / keyfile / auto-generated file. **Back up the key**; protect DB + key file permissions (`ansible-ui` user, `0o600`). |
| **Arbitrary command execution** | **Medium** (by design) | Job templates run `ansible-playbook` or whitelisted script interpreters on paths resolved under allowed roots. A malicious operator with API access can run arbitrary playbooks/scripts the service user can execute—treat API access as **root-equivalent** on managed hosts. |
| **Git SSH** | **Medium** | `StrictHostKeyChecking=accept-new` eases first connect but allows trust-on-first-use MITM until host key is pinned. Prefer known_hosts management for strict environments. |
| **SSH Key Deployer** | **High** if API exposed | `POST .../generate_keypair` returns a **private key** in JSON (same trust model as API: no auth). `POST .../deploy_pubkey` runs `ssh`/`sshpass` to append keys on selected hosts using stored credentials — powerful; keep API on loopback or trusted LAN. Deploy uses `StrictHostKeyChecking=accept-new` (same TOFU caveat as Git). **Unix/Linux/macOS server only** for deploy; password path needs `sshpass`. **One-time deploy** accepts `ephemeral_username` / `ephemeral_password` in the JSON body (not stored); treat like any credential secret in transit (HTTPS / trusted network). |
| **SSRF / internal network** | **Medium** | Inventories and playbooks can target internal IPs; Ansible runs as service user. This is normal for Ansible; restrict who can edit data and network egress if needed. |

### Defensive measures already in place

- Credential `kind` allow-list; project scoping for Git creds, inventories, templates.
- Playbook path restricted under server working tree / git repo after canonicalize.
- Embedded static: path traversal blocked (`..`, absolute paths, `:` in keys).
- Request body limit (Axum `RequestBodyLimitLayer`).
- Job output capped before DB storage.
- `X-Content-Type-Options: nosniff`, `X-Frame-Options: SAMEORIGIN`.
- CORS allow-list by default; optional explicit origins via `ANSIBLE_UI_EXTRA_ORIGINS`.
- Inventory / credential YAML text normalized (CRLF, BOM, NUL) before Ansible to avoid subtle SSH/parser issues.
- `database_parent_dir()` in `secrets.rs` aligned with `db::db_path()` for correct keyfile placement with `sqlite:///...` URLs.
- **`POST /api/ssh_deployer/scan`:** global `tokio::sync::Semaphore(1)` so only **one ICMP scan runs at a time** (extra requests wait in queue instead of multiplying process load).
- **`POST /api/ssh_deployer/public_key`:** JSON must include **`project_id`**; server checks the project exists and the credential belongs to that project. Wrong project returns **404** with the same message as a missing credential to reduce ID enumeration.
- **Playbook listing:** `playbook_discovery::PlaybookListError` distinguishes missing project vs I/O instead of string-matching errors.
- **Deploy pubkey:** `ansible_user` from credential Extra is restricted to safe characters; IPs validated as IPv4; max 32 hosts per request; duplicate identical `authorized_keys` lines skipped (`grep -qxF` on remote). `project_id` must be positive; requests cannot combine `credential_id` with one-time `ephemeral_*` fields.
- **Public key from credential:** Derived with the **`ssh-key`** crate (OpenSSH PEM) first, then `ssh-keygen -y` fallback; avoids many host `libcrypto` / OpenSSL mismatches when reading keys from the DB.
- **Credential API:** Create/update reject empty names, enforce max **256** chars for name, **16 KiB** for `extra`, **512 KiB** for plaintext `secret` (abuse / DB bloat mitigation).
- **Embedded static responses:** HTTP response build failures return 500 instead of panicking on `unwrap()`.

### Frontend

- User-controlled strings in tables/modals generally passed through `escapeHtml` before `innerHTML`.
- Job status CSS classes use **`jobStatusBadgeClass()`** (allow-list: `pending` / `running` / `success` / `failed`) so raw API values are not interpolated into `class` attributes.
- Remaining XSS risk is low for same-origin API data but **any new `innerHTML` without escaping is high risk**.

---

## Code quality

### Strengths

- Clear separation: `server`, `runner`, `crud`, `git_support`, `secrets`, `scheduler`.
- SQL via parameterized queries (`rusqlite` `params!`).
- Mutex poison recovery in hot paths where used.
- Errors returned to API clients as JSON `detail` without leaking stack traces in normal flow.

### Technical debt / improvements (non-blocking)

| Area | Suggestion |
|------|------------|
| **Types** | `crud::get_project` / `get_projects` tuples and `resolve_playbook_and_creds` return type are hard to read; consider small structs. |
| **`run_playbook` arity** | Nine parameters; a `PlaybookRunParams` struct would simplify call sites. |
| **`unwrap` / `expect`** | Acceptable for regex compile, HTTP builder after known-good values, startup (`init_db`, bind). `secrets` encrypt uses `expect` on fixed 32-byte key—OK. |
| **Scheduler** | `schedule_tz` documented as partial; cron evaluated in UTC—document clearly in UI. |
| **Clippy** | Run `cargo clippy -- -D warnings` in CI when feasible; several style warnings remain optional. |

---

## Checklist before production

- [ ] API not exposed without **reverse proxy auth + TLS** (or equivalent).
- [ ] `ANSIBLE_UI_RELAX_CORS` only if required; prefer `ANSIBLE_UI_EXTRA_ORIGINS`.
- [ ] `ANSIBLE_UI_SECRET_KEY` or backed-up keyfile + restrictive permissions on `/var/lib/ansible-ui`.
- [ ] Firewall: limit who can reach nginx / `ANSIBLE_UI_BIND`.
- [ ] `sshpass` installed if using SSH **password** credentials.
- [ ] Job templates point to **playbook files**, not repo directories only.

---

## Tools

```bash
cd src-tauri
cargo check --bin ansible-server --no-default-features --features "server-only,embedded-static"
cargo clippy --bin ansible-server --no-default-features --features "server-only,embedded-static"
```

---

*This document is a living checklist, not a penetration test report.*
