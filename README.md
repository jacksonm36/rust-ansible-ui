# Ansible Control Panel (Tauri + Rust)

A **desktop** version of the [vibe-coded](https://github.com/jacksonm36/vibe-coded) Ansible web UI, built with **Tauri 2** (Rust backend) and the same Red Hat–style frontend. Run playbooks, manage inventories, credentials, and job templates from a single window.

## Features

- **Projects** – Organize playbooks and inventories by project  
- **Git / GitHub** – Clone playbooks from any repo; use **Pull** to sync  
- **Inventories** – Store INI/YAML inventories in SQLite  
- **Credentials** – Encrypted in DB: SSH key, SSH password, Ansible Vault, Git HTTPS token  
- **Job templates** – Playbook + inventory + credentials; launch and view logs  
- **Job history** – Status, duration, and full output for every run  
- **Scheduled jobs** – Cron-style schedules (daily/weekly/monthly)  
- **Red Hat–style UI** – Dark theme, sidebar, dashboard (same as original)

---

## Setup instructions

### 1. Prerequisites

Install these on your machine before cloning:

| Tool | Why | Install |
|------|-----|---------|
| **Rust** (1.70+) | Build the app | [rustup.rs](https://rustup.rs) — then `rustup update` |
| **Ansible** | Runs playbooks | e.g. `pip install ansible` or your OS package manager |
| **Git** | Clone/pull playbooks | [git-scm.com](https://git-scm.com) |
| **Node.js** (optional) | Tauri CLI via npm | [nodejs.org](https://nodejs.org) — only needed for `npm run tauri …` |

On **Windows**, also install **Microsoft C++ Build Tools** if Rust asks for them (Visual Studio Installer → “Desktop development with C++”).

### 2. Clone the repository

**HTTPS:**

```bash
git clone https://github.com/jacksonm36/tauri_ansible_rust.git
cd tauri_ansible_rust
```

**SSH** (if you use SSH keys with GitHub):

```bash
git clone git@github.com:jacksonm36/tauri_ansible_rust.git
cd tauri_ansible_rust
```

Replace `jacksonm36` with your GitHub username if you forked the repo.

### 3. (Optional) Environment variables

Create a `.env` file in the folder where you run the server, or set variables in your shell:

| Variable | Purpose |
|----------|---------|
| `ANSIBLE_UI_SECRET_KEY` | **Optional:** first **32 UTF-8 bytes** used as AES key. If unset or too short, the app loads `ANSIBLE_UI_KEYFILE` or `<database-dir>/ansible_ui_secret.key`; if missing, it **generates** a random key and writes that file. **Back up the key file** — without it, stored credentials cannot be decrypted. |
| `ANSIBLE_UI_KEYFILE` | Override path to the secret key file (base64 of 32 bytes, one line; or raw 32 bytes; or 64 hex chars). |
| `DATABASE_URL` | SQLite path. Default: `./data/ansible_ui.db`. Example: `DATABASE_URL=sqlite:///C:/path/to/ansible_ui.db` |
| `ANSIBLE_HOST_KEY_CHECKING` | `True` / `False` — passed to Ansible (default `False` if unset). |
| `ANSIBLE_UI_BIND` | Listen address, e.g. `0.0.0.0:14300` (all interfaces) or `127.0.0.1:14300` (default). |
| `ANSIBLE_UI_EXTRA_ORIGINS` | Comma-separated CORS origins if you open the UI from another host/port (e.g. `http://192.168.1.10:14300`). |
| `ANSIBLE_UI_SCRIPT_TIMEOUT_SECS` | Max seconds for **script** templates (default `3600`); process is killed afterward. |
| `ANSIBLE_UI_PLAYBOOK_TIMEOUT_SECS` | Max seconds for **ansible-playbook** (default `3600`). Falls back to `ANSIBLE_UI_JOB_TIMEOUT_SECS` if unset. |

### 4. Run the app

**Option A — Browser only (simplest, no Tauri icons)**

From the **project root**:

```bash
cd src-tauri
cargo run --bin ansible-server --no-default-features --features server-only
```

Open **http://127.0.0.1:14300** in your browser.

> Use `--no-default-features` so Tauri is not built and you do **not** need icon files.

**Single binary (UI embedded, no `static/` folder)**

For deployment (e.g. Linux server), build with **`embedded-static`**: the web UI is compiled into `ansible-server`.

```bash
cd src-tauri
cargo build --release --bin ansible-server --no-default-features --features "server-only,embedded-static"
# Binary: target/release/ansible-server
```

Run it from any directory; set `ANSIBLE_UI_BIND` if you need to listen on all interfaces:

```bash
ANSIBLE_UI_BIND=0.0.0.0:14300 ./ansible-server
```

If you browse from another machine, add CORS origins, e.g.:

```bash
export ANSIBLE_UI_EXTRA_ORIGINS=http://192.168.1.5:14300
```

**Linux: automated install + systemd**

As **root**, from the cloned repo:

```bash
sudo bash scripts/install-linux.sh
```

This script will:

- Install OS packages (Debian/Ubuntu `apt`, Fedora `dnf`, or Arch `pacman`): compiler, OpenSSL dev, git, ansible, etc.
- Install **Rust** via **rustup** if `cargo` is missing
- Build **`ansible-server`** in release mode with **embedded UI**
- Install the binary to **`/usr/local/bin/ansible-server`**
- Create user **`ansible-ui`** and data dir **`/var/lib/ansible-ui`**
- Install and enable **`ansible-ui.service`** (listens on **`0.0.0.0:14300`**; DB under `/var/lib/ansible-ui`)

Commands after install:

```bash
sudo systemctl status ansible-ui
sudo journalctl -u ansible-ui -f
```

Set a strong **`ANSIBLE_UI_SECRET_KEY`** in a drop-in (recommended):

```bash
sudo systemctl edit ansible-ui
# Add:
# [Service]
# Environment=ANSIBLE_UI_SECRET_KEY=your-32-plus-character-secret-here
sudo systemctl daemon-reload
sudo systemctl restart ansible-ui
```

**Option B — Tauri desktop (dev)**

1. **Terminal 1** — start the API server:

   ```bash
   cd src-tauri
   cargo run --bin ansible-server --no-default-features --features server-only
   ```

2. **Terminal 2** — from the **project root**:

   ```bash
   npm install
   npm run tauri dev
   ```

   The window loads `http://127.0.0.1:14300` once the server is up.

**Option C — Production Tauri build**

1. Add icons under `src-tauri/icons/` (or generate from a PNG):

   ```bash
   npm install
   npm run tauri icon path/to/your/icon.png
   ```

2. Build:

   ```bash
   npm run tauri build
   ```

   Installers/output are under `src-tauri/target/release/bundle/`.

### 5. First-time checklist

- [ ] Rust installed (`rustc --version`)
- [ ] `ansible-playbook` works in a terminal (`ansible-playbook --version`)
- [ ] `git` works (`git --version`)
- [ ] Cloned repo and `cd` into project root
- [ ] For server-only: `cd src-tauri` and run the `cargo run … server-only` command above
- [ ] Set `ANSIBLE_UI_SECRET_KEY` before storing real credentials in production

---

## Requirements (summary)

- **Rust** (1.70+) – [rustup.rs](https://rustup.rs)
- **Node.js** – For Tauri CLI when using `npm run tauri …`
- **Ansible** – The app runs `ansible-playbook` and scripts (.sh, .ps1, .py, etc.) as subprocesses
- **Git** – For cloning playbooks from GitHub/GitLab

---

## Configuration

- **Database** – SQLite at `./data/ansible_ui.db` (created automatically, relative to the process **current working directory** — usually `src-tauri` when you run `cargo run` from there). Override: `DATABASE_URL=sqlite:///path/to/ansible_ui.db`
- **Credential encryption** – Set `ANSIBLE_UI_SECRET_KEY` to a random string of **32+ characters** in production

---

## Project layout

```
tauri_ansible_rust/
├── deploy/                 # ansible-ui.service (systemd)
├── scripts/                # install-linux.sh
├── static/                 # Frontend: index.html, css/, js/
├── src-tauri/
│   ├── src/
│   │   ├── main.rs, lib.rs
│   │   ├── server.rs       # Axum API + static serving
│   │   ├── db.rs           # SQLite schema and init
│   │   ├── crud.rs         # CRUD for all entities
│   │   ├── schemas.rs      # Request/response types
│   │   ├── secrets.rs      # AES-256-GCM credential encryption
│   │   ├── git_support.rs  # Clone/pull, list playbooks
│   │   ├── runner.rs       # Ansible/script execution
│   │   └── scheduler.rs    # Cron-based job runs
│   ├── Cargo.toml
│   └── tauri.conf.json
├── package.json
└── README.md
```

---

## API

- `GET/POST/PATCH/DELETE /api/projects`, `POST /api/projects/:id/pull`
- `GET/POST/PATCH/DELETE /api/inventories`
- `GET/POST/PATCH/DELETE /api/credentials`
- `GET/POST/PATCH/DELETE /api/job_templates`, `GET /api/job_templates/:id/next_run`
- `GET /api/jobs`, `GET/DELETE /api/jobs/:id`, `POST /api/jobs/launch`

The browser UI uses `API = '/api'` against the same origin (port **14300**).

---

## Icons

For a full Tauri build you need icons under `src-tauri/icons/`. Generate from a PNG:

```bash
npm install
npm run tauri icon path/to/your/icon.png
```

---

## License

MIT (same as vibe-coded).
