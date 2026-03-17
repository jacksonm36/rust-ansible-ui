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

## Requirements

- **Rust** (1.70+) – [rustup.rs](https://rustup.rs)
- **Node.js** (for Tauri CLI; optional if using `cargo tauri`)
- **Ansible** – Must be installed on the system (e.g. `pip install ansible` or system package). The app runs `ansible-playbook` and scripts (.sh, .ps1, .py, etc.) as subprocesses.
- **Git** (optional) – For cloning playbooks from GitHub/GitLab

## Build and run

### Server only (no Tauri window, no icons required)

To build and run only the HTTP server (e.g. use the UI in your browser at http://127.0.0.1:14300):

```bash
cd src-tauri
cargo build --bin ansible-server --features server-only
cargo run --bin ansible-server --features server-only
```

Then open **http://127.0.0.1:14300** in your browser. The `server-only` feature skips the Tauri build step so you don’t need icon files.

### Development (Tauri desktop app)

From the project root (`vibe-coded-tauri`):

1. **Start the server first** (in one terminal):
   ```bash
   cd src-tauri
   cargo run --bin ansible-server --features server-only
   ```
2. **Then run the Tauri app** (in another terminal):
   ```bash
   cargo tauri dev
   ```
   Tauri will wait for http://127.0.0.1:14300 to be up, then open the app window.

Alternatively, if you have Node: `npm install` then `npm run tauri dev` (you still need the server running, or add icons and use the in-app server).

### Production build (Tauri + icons)

For a full Tauri build you need app icons in `src-tauri/icons/` (e.g. `icon.ico` for Windows). Generate them from a PNG:

```bash
npm run tauri icon path/to/your/icon.png
```

Then:

```bash
npm run tauri build
```

Output is under `src-tauri/target/release/` (and the Tauri bundle in `src-tauri/target/release/bundle/`).

## Configuration

- **Database** – SQLite at `./data/ansible_ui.db` (created automatically). Override with env:  
  `DATABASE_URL=sqlite:///path/to/ansible_ui.db`
- **Credential encryption** – Set a 32+ character key in production:  
  `ANSIBLE_UI_SECRET_KEY=your-random-32-char-string`

## Project layout

```
vibe-coded-tauri/
├── static/                 # Frontend (from vibe-coded): index.html, css/, js/
├── src-tauri/
│   ├── src/
│   │   ├── main.rs, lib.rs
│   │   ├── server.rs      # Axum API + static serving
│   │   ├── db.rs          # SQLite schema and init
│   │   ├── crud.rs        # CRUD for all entities
│   │   ├── schemas.rs     # Request/response types
│   │   ├── secrets.rs     # AES-256-GCM credential encryption
│   │   ├── git_support.rs # Clone/pull, list playbooks
│   │   ├── runner.rs      # Ansible/script execution
│   │   └── scheduler.rs   # Cron-based job runs
│   ├── Cargo.toml
│   └── tauri.conf.json
├── package.json
└── README.md
```

## API

Same as the Python app:

- `GET/POST/PATCH/DELETE /api/projects`, `POST /api/projects/:id/pull`
- `GET/POST/PATCH/DELETE /api/inventories`
- `GET/POST/PATCH/DELETE /api/credentials`
- `GET/POST/PATCH/DELETE /api/job_templates`, `GET /api/job_templates/:id/next_run`
- `GET /api/jobs`, `GET/DELETE /api/jobs/:id`, `POST /api/jobs/launch`

When running the Tauri app, the window points at `http://127.0.0.1:14300`; the frontend uses `API = '/api'` as before.

## Icons

For a full build you need Tauri app icons under `src-tauri/icons/` (e.g. 32x32.png, 128x128.png, icon.ico, icon.icns). You can generate them with:

```bash
npm run tauri icon path/to/your/icon.png
```

## License

MIT (same as vibe-coded).
