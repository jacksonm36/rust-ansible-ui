const API = '/api';

function qs(sel, el = document) { return el.querySelector(sel); }
function qsAll(sel, el = document) { return el.querySelectorAll(sel); }

function apiErrorDetail(detail) {
  if (typeof detail === 'string') return detail;
  if (Array.isArray(detail)) return detail.map(d => d.msg || JSON.stringify(d)).join('; ');
  if (detail && typeof detail === 'object') return detail.msg || JSON.stringify(detail);
  return null;
}

function showError(e) {
  const msg = e && typeof e === 'object' && e.message ? e.message : (typeof e === 'string' ? e : String(e));
  alert(msg || 'An error occurred');
}

async function fetchJSON(url, opts = {}) {
  const r = await fetch(url, { headers: { 'Content-Type': 'application/json', ...opts.headers }, ...opts });
  if (!r.ok) {
    const body = await r.json().catch(() => ({}));
    const msg = apiErrorDetail(body.detail) || body.detail || r.statusText;
    throw new Error(msg);
  }
  return r.status === 204 ? null : r.json();
}

let currentPage = 'dashboard';
let projects = [];
let inventories = [];
let credentials = [];
let jobTemplates = [];
let jobs = [];

/** SSH Key Deployer: last scan CIDR + results */
let sshDeployerState = { cidr: '192.168.1.0/24', hosts: [] };

const REFRESH_INTERVAL_MS = 4000;
let refreshIntervalId = null;
let jobPollIntervalId = null;
let refreshBusy = false;

function clearRefresh() {
  if (refreshIntervalId) {
    clearInterval(refreshIntervalId);
    refreshIntervalId = null;
  }
}

function startRefresh() {
  clearRefresh();
  refreshIntervalId = setInterval(async () => {
    if (refreshBusy) return;
    if (jobPollIntervalId) return; // Job modal already polls aggressively
    refreshBusy = true;
    try {
      await loadForPage(currentPage);
      render();
    } catch (err) {
      console.error('Auto-refresh:', err);
    } finally {
      refreshBusy = false;
    }
  }, REFRESH_INTERVAL_MS);
}

function setPage(page) {
  currentPage = page;
  qsAll('.sidebar-nav a').forEach(a => {
    a.classList.toggle('active', a.dataset.page === page);
  });
  render();
  clearRefresh();
  // Auto-refresh on all main pages so project/credential/template changes
  // and job status updates appear without manual reload.
  startRefresh();
  reloadAndRender().catch(console.error);
}

function render() {
  const content = qs('#content');
  if (currentPage === 'dashboard') content.innerHTML = renderDashboard();
  else if (currentPage === 'projects') content.innerHTML = renderProjects();
  else if (currentPage === 'inventories') content.innerHTML = renderInventories();
  else if (currentPage === 'credentials') content.innerHTML = renderCredentials();
  else if (currentPage === 'templates') content.innerHTML = renderTemplates();
  else if (currentPage === 'jobs') content.innerHTML = renderJobs();
  else if (currentPage === 'ssh-deployer') content.innerHTML = renderSshDeployer();
  bindContentEvents();
}

function bindContentEvents() {
  qsAll('.nav-link').forEach(a => {
    a.onclick = (e) => { e.preventDefault(); setPage(a.dataset.page); };
  });
  qsAll('[data-action]').forEach(el => {
    const action = el.dataset.action;
    const id = el.dataset.id ? parseInt(el.dataset.id, 10) : null;
    el.onclick = () => runAction(action, id, el);
  });
}

// Delegate clicks so modal buttons (e.g. Close) work when added dynamically
document.addEventListener('click', (e) => {
  const el = e.target.closest('[data-action]');
  if (!el || el.closest('#content')) return; // #content uses bindContentEvents
  e.preventDefault();
  const action = el.dataset.action;
  const id = el.dataset.id ? parseInt(el.dataset.id, 10) : null;
  runAction(action, id, el);
});

function runAction(action, id, el) {
  if (action === 'close-modal') { closeModal(); reloadAndRender(); return; }
  if (action === 'create-project') openProjectModal();
  if (action === 'edit-project') openProjectModal(id);
  if (action === 'delete-project') deleteProject(id);
  if (action === 'create-inventory') openInventoryModal();
  if (action === 'edit-inventory') openInventoryModal(id);
  if (action === 'delete-inventory') deleteInventory(id);
  if (action === 'create-credential') openCredentialModal();
  if (action === 'edit-credential') openCredentialModal(id);
  if (action === 'delete-credential') deleteCredential(id);
  if (action === 'create-template') { openTemplateModal().catch(showError); return; }
  if (action === 'edit-template') { openTemplateModal(id).catch(showError); return; }
  if (action === 'delete-template') deleteTemplate(id);
  if (action === 'launch-job') launchJob(id);
  if (action === 'view-job') viewJob(id);
  if (action === 'delete-job') deleteJob(id);
  if (action === 'delete-job-history') deleteJobHistory();
  if (action === 'pull-project') pullProject(id);
  if (action === 'ssh-scan') { runSshScan().catch(showError); return; }
  if (action === 'ssh-select-reachable') { sshSelectReachable(true); return; }
  if (action === 'ssh-select-none') { sshSelectReachable(false); return; }
  if (action === 'ssh-show-pubkey') { showSshPublicKeyModal().catch(showError); return; }
  if (action === 'ssh-add-inventory') { addScannedHostsToInventory(); return; }
}

function renderDashboard() {
  const recent = jobs.slice(0, 10);
  const running = jobs.filter(j => j.status === 'running').length;
  const failed = jobs.filter(j => j.status === 'failed').length;
  return `
    <h1 class="page-title">Dashboard</h1>
    <div class="dash-cards">
      <div class="dash-card"><h3>${projects.length}</h3><p>Projects</p></div>
      <div class="dash-card"><h3>${jobTemplates.length}</h3><p>Job Templates</p></div>
      <div class="dash-card"><h3>${jobs.length}</h3><p>Total Jobs</p></div>
      <div class="dash-card"><h3>${running}</h3><p>Running</p></div>
      <div class="dash-card"><h3>${failed}</h3><p>Failed</p></div>
    </div>
    <div class="card">
      <div class="card-header">Recent Jobs</div>
      <div class="table-wrap">
        <table>
          <thead><tr><th>ID</th><th>Playbook</th><th>Status</th><th>Started</th><th></th></tr></thead>
          <tbody>
            ${recent.length ? recent.map(j => `
              <tr>
                <td>${j.id}</td>
                <td>${escapeHtml(j.playbook_path)}</td>
                <td><span class="badge badge-${jobStatusBadgeClass(j.status)}">${escapeHtml(j.status)}</span></td>
                <td>${j.started_at ? new Date(j.started_at).toLocaleString() : '—'}</td>
                <td><button class="btn btn-sm btn-secondary" data-action="view-job" data-id="${j.id}">View</button></td>
              </tr>
            `).join('') : '<tr><td colspan="5" class="empty-state">No jobs yet. Create a job template and launch a job.</td></tr>'}
          </tbody>
        </table>
      </div>
    </div>
  `;
}

function yamlInventoryFromIps(ips) {
  if (!ips.length) return '[scanned]\n';
  const lines = ['all:', '  children:', '    scanned:', '      hosts:'];
  ips.forEach(ip => {
    const hid = 'h' + ip.replace(/\./g, '_');
    lines.push(`        ${hid}:`);
    lines.push(`          ansible_host: ${ip}`);
  });
  return lines.join('\n');
}

function renderSshDeployer() {
  const { cidr, hosts } = sshDeployerState;
  const projectOpts = projects.length
    ? projects.map(p => `<option value="${p.id}">${escapeHtml(p.name)}</option>`).join('')
    : '<option value="">No projects</option>';
  const credOpts = credentials.filter(c => c.kind === 'ssh').map(c =>
    `<option value="${c.id}" data-project-id="${c.project_id}">${escapeHtml(c.name)} (project ${c.project_id})</option>`).join('');
  const rows = hosts.length
    ? hosts.map(h => `
      <tr>
        <td><input type="checkbox" class="ssh-host-cb" data-ip="${escapeHtml(h.ip)}"></td>
        <td><code>${escapeHtml(h.ip)}</code></td>
        <td>${h.alive ? '<span class="badge badge-success">reachable</span>' : '<span class="badge badge-pending">no reply</span>'}</td>
      </tr>`).join('')
    : '<tr><td colspan="3" class="empty-state">Run a scan to list addresses (ICMP from the server). Max 1024 hosts per scan.</td></tr>';
  return `
    <h1 class="page-title">SSH Key Deployer</h1>
    <p class="text-muted">Discover hosts on your LAN from <strong>this server</strong>, add them to an inventory, then use an SSH credential + job template to run Ansible (e.g. <code>authorized_key</code>) or paste your public key manually.</p>
    <div class="card">
      <div class="card-header">Network scan</div>
      <div style="padding:1rem;">
        <div class="form-group">
          <label>IPv4 CIDR</label>
          <input type="text" id="ssh-cidr" value="${escapeHtml(cidr)}" placeholder="192.168.1.0/24" style="max-width:320px;">
        </div>
        <button type="button" class="btn btn-primary" data-action="ssh-scan">Scan network</button>
        <span class="text-muted" style="margin-left:12px;">Uses OS <code>ping</code>. Many hosts block ICMP; unreachable ≠ down.</span>
      </div>
    </div>
    <div class="card">
      <div class="card-header">Scan results</div>
      <div class="table-wrap">
        <table>
          <thead><tr><th style="width:44px"></th><th>Address</th><th>ICMP</th></tr></thead>
          <tbody>${rows}</tbody>
        </table>
      </div>
      <div style="padding:0.75rem 1rem;display:flex;gap:8px;flex-wrap:wrap;align-items:center;">
        <button type="button" class="btn btn-sm btn-secondary" data-action="ssh-select-reachable">Select reachable</button>
        <button type="button" class="btn btn-sm btn-secondary" data-action="ssh-select-none">Clear</button>
        <span style="flex:1"></span>
        <label class="text-muted">Project</label>
        <select id="ssh-inv-project">${projectOpts}</select>
        <button type="button" class="btn btn-sm btn-primary" data-action="ssh-add-inventory">Add selected to inventory…</button>
      </div>
    </div>
    <div class="card">
      <div class="card-header">SSH public key (from credential)</div>
      <div style="padding:1rem;">
        <p class="text-muted">Derives the public line with <code>ssh-keygen -y</code> on the server (OpenSSH required).</p>
        <div class="form-group">
          <label>SSH private key credential</label>
          <select id="ssh-cred"><option value="">— Select —</option>${credOpts || '<option value="" disabled>No SSH credentials</option>'}</select>
        </div>
        <button type="button" class="btn btn-secondary" data-action="ssh-show-pubkey">Show public key &amp; tips</button>
      </div>
    </div>
  `;
}

async function runSshScan() {
  const input = qs('#ssh-cidr');
  const cidr = input ? input.value.trim() : '';
  if (!cidr) { alert('Enter an IPv4 CIDR (e.g. 192.168.1.0/24).'); return; }
  const btn = qs('[data-action="ssh-scan"]');
  if (btn) { btn.disabled = true; btn.textContent = 'Scanning…'; }
  try {
    const data = await fetchJSON(`${API}/ssh_deployer/scan`, { method: 'POST', body: JSON.stringify({ cidr }) });
    sshDeployerState = { cidr, hosts: data.hosts || [] };
    render();
  } catch (e) {
    showError(e);
  } finally {
    const b2 = qs('[data-action="ssh-scan"]');
    if (b2) { b2.disabled = false; b2.textContent = 'Scan network'; }
  }
}

function sshSelectReachable(onlyAlive) {
  const alive = new Set((sshDeployerState.hosts || []).filter(h => h.alive).map(h => h.ip));
  qsAll('.ssh-host-cb').forEach(cb => {
    const ip = cb.dataset.ip;
    if (onlyAlive) cb.checked = alive.has(ip);
    else cb.checked = false;
  });
}

function addScannedHostsToInventory() {
  const selected = [...qsAll('.ssh-host-cb:checked')].map(cb => cb.dataset.ip).filter(Boolean);
  if (!selected.length) { alert('Select at least one host.'); return; }
  const sel = qs('#ssh-inv-project');
  const project_id = sel ? parseInt(sel.value, 10) : NaN;
  if (!project_id) { alert('Choose a project.'); return; }
  const content = yamlInventoryFromIps(selected);
  const suggestName = `scanned-${new Date().toISOString().slice(0, 10)}`;
  openInventoryModal(null, { project_id, content, suggestName });
}

async function showSshPublicKeyModal() {
  const sel = qs('#ssh-cred');
  const opt = sel && sel.selectedOptions && sel.selectedOptions[0];
  const credential_id = sel && sel.value ? parseInt(sel.value, 10) : 0;
  const project_id = opt && opt.dataset.projectId ? parseInt(opt.dataset.projectId, 10) : 0;
  if (!credential_id || !project_id) { alert('Select an SSH private key credential.'); return; }
  const data = await fetchJSON(`${API}/ssh_deployer/public_key`, {
    method: 'POST',
    body: JSON.stringify({ credential_id, project_id }),
  });
  const pk = data.public_key;
  showModal(
    'SSH public key',
    `<p>Add this line to <code>~/.ssh/authorized_keys</code> on each target (for the user you SSH as).</p>
     <textarea readonly style="width:100%;min-height:80px;font-family:monospace;font-size:12px;">${escapeHtml(pk)}</textarea>
     <p class="text-muted" style="margin-top:12px;">From your workstation (if you have password SSH access):</p>
     <pre style="overflow:auto;font-size:12px;background:var(--bg-elevated);padding:8px;border-radius:4px;">ssh-copy-id -i ~/.ssh/your_key.pub user@host</pre>
     <p class="text-muted">Or use Ansible <code>authorized_key</code> with a password credential in a job template.</p>`,
    '<button class="btn btn-secondary" data-action="close-modal">Close</button>'
  );
}

function renderProjects() {
  return `
    <h1 class="page-title">Projects</h1>
    <div class="card">
      <div class="card-header">
        Projects
        <button class="btn btn-primary btn-sm" data-action="create-project">+ Add Project</button>
      </div>
      <div class="table-wrap">
        <table>
          <thead><tr><th>Name</th><th>Description</th><th>Git repo</th><th>Updated</th><th></th></tr></thead>
          <tbody>
            ${projects.length ? projects.map(p => `
              <tr>
                <td>${escapeHtml(p.name)}</td>
                <td>${escapeHtml(p.description || '—')}</td>
                <td>${p.git_url ? escapeHtml(p.git_url) : '—'}</td>
                <td>${new Date(p.updated_at).toLocaleString()}</td>
                <td>
                  ${p.git_url ? `<button class="btn btn-sm btn-primary" data-action="pull-project" data-id="${p.id}" title="Pull playbooks from Git">Pull</button>` : ''}
                  <button class="btn btn-sm btn-secondary" data-action="edit-project" data-id="${p.id}">Edit</button>
                  <button class="btn btn-sm btn-danger" data-action="delete-project" data-id="${p.id}">Delete</button>
                </td>
              </tr>
            `).join('') : '<tr><td colspan="5" class="empty-state">No projects. Create one to get started.</td></tr>'}
          </tbody>
        </table>
      </div>
    </div>
  `;
}

function renderInventories() {
  return `
    <h1 class="page-title">Inventories</h1>
    <div class="card">
      <div class="card-header">
        Inventories
        <button class="btn btn-primary btn-sm" data-action="create-inventory">+ Add Inventory</button>
      </div>
      <div class="table-wrap">
        <table>
          <thead><tr><th>Name</th><th>Project</th><th>Updated</th><th></th></tr></thead>
          <tbody>
            ${inventories.length ? inventories.map(inv => {
              const proj = projects.find(p => p.id === inv.project_id);
              return `
              <tr>
                <td>${escapeHtml(inv.name)}</td>
                <td>${proj ? escapeHtml(proj.name) : inv.project_id}</td>
                <td>${new Date(inv.updated_at).toLocaleString()}</td>
                <td>
                  <button class="btn btn-sm btn-secondary" data-action="edit-inventory" data-id="${inv.id}">Edit</button>
                  <button class="btn btn-sm btn-danger" data-action="delete-inventory" data-id="${inv.id}">Delete</button>
                </td>
              </tr>`;
            }).join('') : '<tr><td colspan="4" class="empty-state">No inventories. Add one for a project.</td></tr>'}
          </tbody>
        </table>
      </div>
    </div>
  `;
}

function renderCredentials() {
  return `
    <h1 class="page-title">Credentials</h1>
    <div class="card">
      <div class="card-header">
        Credentials
        <button class="btn btn-primary btn-sm" data-action="create-credential">+ Add Credential</button>
      </div>
      <div class="table-wrap">
        <table>
          <thead><tr><th>Name</th><th>Kind</th><th>Project</th><th></th></tr></thead>
          <tbody>
            ${credentials.length ? credentials.map(c => {
              const proj = projects.find(p => p.id === c.project_id);
              return `
              <tr>
                <td>${escapeHtml(c.name)}</td>
                <td>${escapeHtml(c.kind)}</td>
                <td>${proj ? escapeHtml(proj.name) : c.project_id}</td>
                <td>
                  <button class="btn btn-sm btn-secondary" data-action="edit-credential" data-id="${c.id}">Edit</button>
                  <button class="btn btn-sm btn-danger" data-action="delete-credential" data-id="${c.id}">Delete</button>
                </td>
              </tr>`;
            }).join('') : '<tr><td colspan="4" class="empty-state">No credentials. Add SSH or Vault credentials.</td></tr>'}
          </tbody>
        </table>
      </div>
    </div>
  `;
}

function renderTemplates() {
  return `
    <h1 class="page-title">Job Templates</h1>
    <div class="card">
      <div class="card-header">
        Job Templates
        <button class="btn btn-primary btn-sm" data-action="create-template">+ Add Template</button>
      </div>
      <div class="table-wrap">
        <table>
          <thead><tr><th>Name</th><th>Playbook</th><th>Project</th><th></th></tr></thead>
          <tbody>
            ${jobTemplates.length ? jobTemplates.map(jt => {
              const proj = projects.find(p => p.id === jt.project_id);
              const sched = jt.schedule_enabled && jt.schedule_cron ? `<span class="badge badge-running" title="${escapeHtml(jt.schedule_cron)}">Schedule</span>` : '';
              return `
              <tr>
                <td>${escapeHtml(jt.name)} ${sched}</td>
                <td>${escapeHtml(jt.playbook_path)}</td>
                <td>${proj ? escapeHtml(proj.name) : jt.project_id}</td>
                <td>
                  <button class="btn btn-sm btn-primary" data-action="launch-job" data-id="${jt.id}">Launch</button>
                  <button class="btn btn-sm btn-secondary" data-action="edit-template" data-id="${jt.id}">Edit</button>
                  <button class="btn btn-sm btn-danger" data-action="delete-template" data-id="${jt.id}">Delete</button>
                </td>
              </tr>`;
            }).join('') : '<tr><td colspan="4" class="empty-state">No job templates. Create one to run playbooks.</td></tr>'}
          </tbody>
        </table>
      </div>
    </div>
  `;
}

function renderJobs() {
  return `
    <h1 class="page-title">Jobs</h1>
    <div class="card">
      <div class="card-header">
        Job history
        <button class="btn btn-danger btn-sm float-right" data-action="delete-job-history">Clear all</button>
      </div>
      <div class="table-wrap">
        <table>
          <thead><tr><th>ID</th><th>Playbook</th><th>Status</th><th>Started</th><th>Finished</th><th></th></tr></thead>
          <tbody>
            ${jobs.length ? jobs.map(j => `
              <tr>
                <td>${j.id}</td>
                <td>${escapeHtml(j.playbook_path)}</td>
                <td><span class="badge badge-${jobStatusBadgeClass(j.status)}">${escapeHtml(j.status)}</span></td>
                <td>${j.started_at ? new Date(j.started_at).toLocaleString() : '—'}</td>
                <td>${j.finished_at ? new Date(j.finished_at).toLocaleString() : '—'}</td>
                <td>
                  <button class="btn btn-sm btn-secondary" data-action="view-job" data-id="${j.id}">View log</button>
                  <button class="btn btn-sm btn-danger" data-action="delete-job" data-id="${j.id}">Delete</button>
                </td>
              </tr>
            `).join('') : '<tr><td colspan="6" class="empty-state">No jobs yet.</td></tr>'}
          </tbody>
        </table>
      </div>
    </div>
  `;
}

function escapeHtml(s) {
  if (s == null) return '';
  const div = document.createElement('div');
  div.textContent = s;
  return div.innerHTML;
}

const JOB_STATUS_BADGES = new Set(['pending', 'running', 'success', 'failed']);

/** Safe suffix for <span class="badge badge-*"> — never put raw API strings in class names. */
function jobStatusBadgeClass(status) {
  const s = typeof status === 'string' ? status : '';
  return JOB_STATUS_BADGES.has(s) ? s : 'pending';
}

function extractAnsibleUser(extra) {
  const src = (extra || '').replace(/\r\n/g, '\n');
  for (const line of src.split('\n')) {
    const m = line.match(/^\s*ansible_user\s*:\s*(.+?)\s*$/);
    if (m && m[1]) return m[1].replace(/^['"]|['"]$/g, '').trim();
  }
  return '';
}

function upsertAnsibleUser(extra, user) {
  const lines = ((extra || '').replace(/\r\n/g, '\n')).split('\n');
  const out = [];
  for (const line of lines) {
    if (!/^\s*ansible_user\s*:/.test(line)) out.push(line);
  }
  const u = (user || '').trim();
  if (u) out.unshift(`ansible_user: ${u}`);
  return out.join('\n').replace(/^\n+/, '').replace(/\n{3,}/g, '\n\n');
}

function upsertSudoConfig(extra, becomeUser, sshUser) {
  const lines = ((extra || '').replace(/\r\n/g, '\n')).split('\n');
  const out = [];
  for (const line of lines) {
    if (/^\s*ansible_become\s*:/.test(line)) continue;
    if (/^\s*ansible_become_method\s*:/.test(line)) continue;
    if (/^\s*ansible_become_user\s*:/.test(line)) continue;
    out.push(line);
  }
  out.unshift('ansible_become_method: sudo');
  out.unshift('ansible_become: true');
  let bUser = (becomeUser || '').trim();
  const sUser = (sshUser || '').trim().toLowerCase();
  // Common case: SSH as non-root, escalate to root.
  if (!bUser && sUser && sUser !== 'root') bUser = 'root';
  if (bUser) out.unshift(`ansible_become_user: ${bUser}`);
  return out.join('\n').replace(/^\n+/, '').replace(/\n{3,}/g, '\n\n');
}

function upsertYamlKey(extra, key, value) {
  const lines = ((extra || '').replace(/\r\n/g, '\n')).split('\n');
  const out = [];
  for (const line of lines) {
    if (new RegExp(`^\\s*${key}\\s*:`).test(line)) continue;
    out.push(line);
  }
  const v = (value || '').trim();
  if (v) out.unshift(`${key}: ${v}`);
  return out.join('\n').replace(/^\n+/, '').replace(/\n{3,}/g, '\n\n');
}

function looksLikePublicKey(secret) {
  const s = (secret || '').trim();
  if (!s) return false;
  return /^(ssh-rsa|ssh-ed25519|ecdsa-sha2-nistp\d+|sk-ssh-ed25519@openssh\.com|sk-ecdsa-sha2-nistp256@openssh\.com)\s+/i.test(s);
}

function looksLikePrivateKey(secret) {
  const s = (secret || '').trim();
  return /-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----/i.test(s);
}

// Schedule builder: friendly object <-> cron (backend)
// cron = "minute hour day month dow" (dow: 0=Sun, 1=Mon, ..., 6=Sat)
function scheduleToCron(s) {
  if (!s || !s.enabled) return null;
  const min = Number(s.minute) || 0;
  const hour = Number(s.hour) ?? 2;
  const tz = (s.tz || 'UTC').trim() || 'UTC';
  if (s.repeat === 'daily') return `${min} ${hour} * * *`;
  if (s.repeat === 'weekly' && s.weekDays && s.weekDays.length) {
    const dow = s.weekDays.map(d => d === 7 ? 0 : d).sort((a, b) => a - b).join(',');
    return `${min} ${hour} * * ${dow}`;
  }
  if (s.repeat === 'monthly' && s.monthDays && s.monthDays.length) {
    const dom = s.monthDays.sort((a, b) => a - b).join(',');
    return `${min} ${hour} ${dom} * *`;
  }
  return null;
}

function cronToSchedule(cronStr, tz) {
  const def = { enabled: false, hour: 2, minute: 0, repeat: 'daily', weekDays: [], monthDays: [], tz: tz || 'UTC' };
  if (!cronStr || !cronStr.trim()) return def;
  const parts = cronStr.trim().split(/\s+/);
  if (parts.length < 5) return def;
  const [min, hour, dom, month, dow] = parts;
  def.enabled = true;
  def.minute = parseInt(min, 10) || 0;
  def.hour = parseInt(hour, 10) ?? 2;
  if (dom === '*' && dow === '*') {
    def.repeat = 'daily';
  } else if (dom === '*' && dow !== '*') {
    def.repeat = 'weekly';
    def.weekDays = dow.split(',').map(s => parseInt(s.trim(), 10)).filter(n => !isNaN(n) && n >= 0 && n <= 6);
  } else if (dom !== '*' && dow === '*') {
    def.repeat = 'monthly';
    def.monthDays = dom.split(',').map(s => parseInt(s.trim(), 10)).filter(n => !isNaN(n) && n >= 1 && n <= 31);
  }
  if (def.repeat === 'weekly') def.weekDays = def.weekDays.map(n => n === 0 ? 7 : n);
  def.tz = tz || def.tz;
  return def;
}

function showModal(title, body, footer = '') {
  const overlay = qs('#modal-overlay');
  const modal = qs('#modal');
  modal.innerHTML = `<div class="modal-header">${escapeHtml(title)}</div><div class="modal-body">${body}</div><div class="modal-footer">${footer}</div>`;
  overlay.classList.remove('hidden');
}

function closeModal() {
  if (jobPollIntervalId) {
    clearInterval(jobPollIntervalId);
    jobPollIntervalId = null;
  }
  qs('#modal-overlay').classList.add('hidden');
}

qs('#modal-overlay').onclick = (e) => { if (e.target === e.currentTarget) closeModal(); };

function openProjectModal(id) {
  const p = id ? projects.find(x => x.id === id) : null;
  const credOptions = p
    ? credentials.filter(c => (c.kind === 'ssh' || c.kind === 'git') && c.project_id === p.id).map(c => `<option value="${c.id}" ${p.git_credential_id === c.id ? 'selected' : ''}>${escapeHtml(c.name)} (${escapeHtml(c.kind)})</option>`).join('')
    : '';
  showModal(
    p ? 'Edit Project' : 'New Project',
    `
      <div class="form-group">
        <label>Name</label>
        <input type="text" id="modal-name" value="${p ? escapeHtml(p.name) : ''}" placeholder="Project name">
      </div>
      <div class="form-group">
        <label>Description</label>
        <textarea id="modal-desc" placeholder="Optional">${p ? escapeHtml(p.description || '') : ''}</textarea>
      </div>
      <div class="form-group">
        <label>Git / GitHub repo URL (optional)</label>
        <input type="text" id="modal-git-url" value="${p && p.git_url ? escapeHtml(p.git_url) : ''}" placeholder="https://github.com/owner/repo.git or git@github.com:owner/repo.git">
      </div>
      <div class="form-group">
        <label>Git branch</label>
        <input type="text" id="modal-git-branch" value="${p && p.git_branch ? escapeHtml(p.git_branch) : 'main'}" placeholder="main">
      </div>
      <div class="form-group">
        <label>Git credential (for private repos: SSH key or Git token)</label>
        <select id="modal-git-cred"><option value="">— None (public repo) —</option>${credOptions || '<option value="" disabled>Save project first, then add credentials for this project and edit to attach</option>'}</select>
      </div>
    `,
    `<button class="btn btn-secondary" data-action="close-modal">Cancel</button>
     <button class="btn btn-primary" id="modal-save-project" data-id="${id || ''}">Save</button>`
  );
  qs('#modal-save-project').onclick = async () => {
    const name = qs('#modal-name').value.trim();
    if (!name) return;
    const git_url = qs('#modal-git-url').value.trim() || null;
    const git_branch = qs('#modal-git-branch').value.trim() || 'main';
    const gcid = qs('#modal-git-cred').value;
    const git_credential_id = gcid ? parseInt(gcid, 10) : null;
    try {
      if (id) {
        await fetchJSON(`${API}/projects/${id}`, { method: 'PATCH', body: JSON.stringify({ name, description: qs('#modal-desc').value, git_url, git_branch, git_credential_id }) });
      } else {
        const created = await fetchJSON(`${API}/projects`, { method: 'POST', body: JSON.stringify({ name, description: qs('#modal-desc').value, git_url, git_branch }) });
        if (git_credential_id != null && created && created.id) {
          await fetchJSON(`${API}/projects/${created.id}`, { method: 'PATCH', body: JSON.stringify({ git_credential_id }) });
        }
      }
      closeModal();
      reloadAndRender();
    } catch (e) { showError(e); }
  };
}

function openInventoryModal(id, opts = {}) {
  const inv = id ? inventories.find(x => x.id === id) : null;
  const initialContent = inv ? inv.content : (opts.content ?? '');
  const defaultName = inv ? inv.name : (opts.suggestName || '');
  const selectedPid = inv ? inv.project_id : opts.project_id;
  showModal(
    inv ? 'Edit Inventory' : 'New Inventory',
    `
      <div class="form-group">
        <label>Project</label>
        <select id="modal-inv-project">${projects.map(p => `<option value="${p.id}" ${p.id === selectedPid ? 'selected' : ''}>${escapeHtml(p.name)}</option>`).join('')}</select>
      </div>
      <div class="form-group">
        <label>Name</label>
        <input type="text" id="modal-name" value="${escapeHtml(defaultName)}" placeholder="Inventory name">
      </div>
      <div class="form-group">
        <label>Content (INI or YAML)</label>
        <textarea id="modal-content" placeholder="[all]\nhost1\nhost2" style="min-height:180px">${escapeHtml(initialContent)}</textarea>
      </div>
    `,
    `<button class="btn btn-secondary" data-action="close-modal">Cancel</button>
     <button class="btn btn-primary" id="modal-save-inv" data-id="${id || ''}">Save</button>`
  );
  const sel = qs('#modal-inv-project');
  if (!inv && projects[0] && (selectedPid == null || selectedPid === undefined)) sel.value = projects[0].id;
  qs('#modal-save-inv').onclick = async () => {
    const name = qs('#modal-name').value.trim();
    const project_id = parseInt(sel.value, 10);
    if (!name || !project_id) return;
    try {
      if (id) await fetchJSON(`${API}/inventories/${id}`, { method: 'PATCH', body: JSON.stringify({ name, content: qs('#modal-content').value }) });
      else await fetchJSON(`${API}/inventories`, { method: 'POST', body: JSON.stringify({ project_id, name, content: qs('#modal-content').value }) });
      closeModal();
      reloadAndRender();
    } catch (e) { showError(e); }
  };
}

function openCredentialModal(id) {
  const c = id ? credentials.find(x => x.id === id) : null;
  const sshUser = c ? extractAnsibleUser(c.extra || '') : '';
  showModal(
    c ? 'Edit Credential' : 'New Credential',
    `
      <div class="form-group">
        <label>Project</label>
        <select id="modal-cred-project">${projects.map(p => `<option value="${p.id}" ${c && c.project_id === p.id ? 'selected' : ''}>${escapeHtml(p.name)}</option>`).join('')}</select>
      </div>
      <div class="form-group">
        <label>Name</label>
        <input type="text" id="modal-name" value="${c ? escapeHtml(c.name) : ''}" placeholder="Credential name">
      </div>
      <div class="form-group">
        <label>Kind</label>
        <select id="modal-kind">
          <option value="ssh" ${c && c.kind === 'ssh' ? 'selected' : ''}>SSH private key (remote servers / Git SSH)</option>
          <option value="password" ${c && c.kind === 'password' ? 'selected' : ''}>SSH password</option>
          <option value="vault" ${c && c.kind === 'vault' ? 'selected' : ''}>Ansible Vault password</option>
          <option value="git" ${c && c.kind === 'git' ? 'selected' : ''}>Git HTTPS token (GitHub/GitLab)</option>
        </select>
      </div>
      <div class="form-group">
        <label>Secret</label>
        <textarea id="modal-secret" placeholder="${c ? 'Leave blank to keep existing' : 'Paste private key, password, or token'}">${c ? '' : ''}</textarea>
      </div>
      <div class="form-group">
        <label>Extra (optional YAML)</label>
        <textarea id="modal-cred-extra" placeholder="e.g. ansible_user: root&#10;ansible_ssh_common_args: '-o PreferredAuthentications=password'">${c ? escapeHtml(c.extra || '') : ''}</textarea>
        <small class="text-muted">SSH jobs run as the <strong>service account</strong> (e.g. ansible-ui) unless you set <code>ansible_user</code> here or in inventory.</small>
      </div>
      <div class="form-group">
        <label>SSH User</label>
        <div style="display:flex; gap:8px;">
          <input type="text" id="modal-ssh-user" value="${escapeHtml(sshUser)}" placeholder="e.g. root or ubuntu">
          <button type="button" class="btn btn-sm btn-secondary" id="modal-apply-ssh-user">Use for SSH</button>
        </div>
      </div>
      <div class="form-group">
        <label>Sudo</label>
        <div style="display:flex; gap:8px;">
          <input type="text" id="modal-sudo-user" placeholder="optional become user (e.g. root)">
          <button type="button" class="btn btn-sm btn-secondary" id="modal-apply-sudo">Use sudo</button>
        </div>
      </div>
      <div class="form-group">
        <label>Sudo Password (optional)</label>
        <div style="display:flex; gap:8px;">
          <input type="password" id="modal-sudo-pass" placeholder="optional sudo password">
          <button type="button" class="btn btn-sm btn-secondary" id="modal-apply-sudo-pass">Use sudo password</button>
        </div>
        <small class="text-muted">This writes <code>ansible_become_password</code> to Extra YAML (plain text).</small>
      </div>
    `,
    `<button class="btn btn-secondary" data-action="close-modal">Cancel</button>
     <button class="btn btn-primary" id="modal-save-cred" data-id="${id || ''}">Save</button>`
  );
  const sel = qs('#modal-cred-project');
  const sshUserInput = qs('#modal-ssh-user');
  const extraInput = qs('#modal-cred-extra');
  const sudoUserInput = qs('#modal-sudo-user');
  const sudoPassInput = qs('#modal-sudo-pass');
  qs('#modal-apply-ssh-user').onclick = () => {
    extraInput.value = upsertAnsibleUser(extraInput.value, sshUserInput.value);
  };
  qs('#modal-apply-sudo').onclick = () => {
    let sudoUser = sudoUserInput.value;
    if (!sudoUser.trim() && sshUserInput.value.trim() && sshUserInput.value.trim().toLowerCase() !== 'root') {
      sudoUser = 'root';
      sudoUserInput.value = 'root';
    }
    extraInput.value = upsertSudoConfig(extraInput.value, sudoUser, sshUserInput.value);
  };
  qs('#modal-apply-sudo-pass').onclick = () => {
    let sudoPass = sudoPassInput.value;
    if (!sudoPass.trim() && qs('#modal-kind').value === 'password') {
      // Common case: sudo password equals SSH password.
      sudoPass = qs('#modal-secret').value || '';
      sudoPassInput.value = sudoPass;
    }
    extraInput.value = upsertYamlKey(extraInput.value, 'ansible_become_password', sudoPass);
  };
  if (!c && projects[0]) sel.value = projects[0].id;
  qs('#modal-save-cred').onclick = async () => {
    const name = qs('#modal-name').value.trim();
    const project_id = parseInt(sel.value, 10);
    const kind = qs('#modal-kind').value;
    const secret = qs('#modal-secret').value;
    const mergedExtra = upsertAnsibleUser(extraInput.value, sshUserInput.value);
    if (!name || !project_id) return;
    if (!id && !secret) { alert('Secret is required for new credential'); return; }
    if (kind === 'ssh' && secret && looksLikePublicKey(secret) && !looksLikePrivateKey(secret)) {
      alert('This looks like an SSH public key (e.g. ssh-rsa ...). For SSH credentials, paste a PRIVATE key block (-----BEGIN ... PRIVATE KEY-----).');
      return;
    }
    try {
      if (id) {
        const body = { name, kind, extra: mergedExtra };
        if (secret) body.secret = secret;
        await fetchJSON(`${API}/credentials/${id}`, { method: 'PATCH', body: JSON.stringify(body) });
      } else await fetchJSON(`${API}/credentials`, { method: 'POST', body: JSON.stringify({ project_id, name, kind, secret: secret || 'x', extra: mergedExtra }) });
      closeModal();
      reloadAndRender();
    } catch (e) { showError(e); }
  };
}

async function openTemplateModal(id) {
  const jt = id ? jobTemplates.find(x => x.id === id) : null;
  const invOptions = inventories.map(inv => `<option value="${inv.id}" ${jt && jt.inventory_id === inv.id ? 'selected' : ''}>${escapeHtml(inv.name)} (${inv.project_id})</option>`).join('');
  const credOptions = credentials.map(c => `<option value="${c.id}" ${jt && jt.credential_id === c.id ? 'selected' : ''}>${escapeHtml(c.name)}</option>`).join('');
  const savedPath = jt ? jt.playbook_path : '';
  showModal(
    jt ? 'Edit Job Template' : 'New Job Template',
    `
      <div class="form-group">
        <label>Project</label>
        <select id="modal-tpl-project">${projects.map(p => `<option value="${p.id}" ${jt && jt.project_id === p.id ? 'selected' : ''}>${escapeHtml(p.name)}</option>`).join('')}</select>
      </div>
      <div class="form-group">
        <label>Name</label>
        <input type="text" id="modal-name" value="${jt ? escapeHtml(jt.name) : ''}" placeholder="Template name">
      </div>
      <div class="form-group">
        <label>Playbook or script path</label>
        <select id="modal-playbook-select"><option value="">— Loading… —</option></select>
        <input type="text" id="modal-playbook-custom" placeholder="Custom path (e.g. subdir/site.yml or script.sh)" style="display:none;margin-top:8px;width:100%;box-sizing:border-box;">
        <small class="text-muted">Lists files from the project workspace after <strong>Pull</strong>, plus paths under the server working directory. Pick <em>Custom path…</em> to type any path.</small>
      </div>
      <div class="form-group">
        <label>Inventory</label>
        <select id="modal-inv"><option value="">— None —</option>${invOptions}</select>
      </div>
      <div class="form-group">
        <label>Credential (SSH key, SSH password, or Vault)</label>
        <select id="modal-cred"><option value="">— None —</option>${credOptions}</select>
      </div>
      <div class="form-group">
        <label>Extra vars (YAML/JSON)</label>
        <textarea id="modal-extra">${jt ? escapeHtml(jt.extra_vars || '') : ''}</textarea>
      </div>
      <div class="schedule-builder card">
        <div class="schedule-builder-header">
          <label class="schedule-toggle"><input type="checkbox" id="modal-schedule-enabled" ${jt && jt.schedule_enabled ? 'checked' : ''}> Run on schedule</label>
        </div>
        <div class="schedule-builder-body" id="modal-schedule-body">
          <div class="schedule-time-row">
            <label>Time</label>
            <select id="modal-schedule-hour">${Array.from({ length: 24 }, (_, i) => `<option value="${i}">${String(i).padStart(2, '0')}:00</option>`).join('')}</select>
            <span class="schedule-time-sep">:</span>
            <select id="modal-schedule-minute">${[0, 15, 30, 45].map(m => `<option value="${m}">${String(m).padStart(2, '0')}</option>`).join('')}</select>
          </div>
          <div class="form-group">
            <label>Repeat</label>
            <select id="modal-schedule-repeat">
              <option value="daily">Every day</option>
              <option value="weekly">Every week (pick days)</option>
              <option value="monthly">Every month (pick days)</option>
            </select>
          </div>
          <div class="schedule-days-wrap" id="modal-schedule-week-wrap">
            <label>Days of week</label>
            <div class="schedule-days" id="modal-schedule-week-days" role="group">
              ${['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'].map((d, i) => `<label class="schedule-day"><input type="checkbox" data-dow="${i + 1}" value="${i + 1}">${d}</label>`).join('')}
            </div>
          </div>
          <div class="schedule-days-wrap" id="modal-schedule-month-wrap" style="display:none">
            <label>Days of month</label>
            <div class="schedule-month-days" id="modal-schedule-month-days" role="group">
              ${Array.from({ length: 31 }, (_, i) => i + 1).map(d => `<label class="schedule-day"><input type="checkbox" data-dom="${d}" value="${d}">${d}</label>`).join('')}
            </div>
          </div>
          <div class="form-group schedule-tz-row">
            <label>Timezone</label>
            <select id="modal-schedule-tz">
              <option value="UTC">UTC</option>
              <option value="Europe/London">Europe/London</option>
              <option value="Europe/Paris">Europe/Paris</option>
              <option value="Europe/Budapest">Europe/Budapest</option>
              <option value="America/New_York">America/New_York</option>
              <option value="America/Los_Angeles">America/Los_Angeles</option>
              <option value="Asia/Tokyo">Asia/Tokyo</option>
              <option value="__other__">Other</option>
            </select>
            <input type="text" id="modal-schedule-tz-other" class="schedule-tz-other" placeholder="e.g. Pacific/Auckland" style="display:none">
          </div>
          <div class="schedule-summary" id="modal-schedule-summary"></div>
          <div class="schedule-next-run" id="modal-next-run-wrap"><label>Next run</label><p id="modal-next-run" class="text-muted">—</p></div>
        </div>
      </div>
    `,
    `<button class="btn btn-secondary" data-action="close-modal">Cancel</button>
     <button class="btn btn-primary" id="modal-save-tpl" data-id="${id || ''}">Save</button>`
  );
  if (!jt && projects[0]) qs('#modal-tpl-project').value = projects[0].id;
  const scheduleEnabled = qs('#modal-schedule-enabled');
  const scheduleBody = qs('#modal-schedule-body');
  const repeatSelect = qs('#modal-schedule-repeat');
  const weekWrap = qs('#modal-schedule-week-wrap');
  const monthWrap = qs('#modal-schedule-month-wrap');
  const weekDaysEl = qs('#modal-schedule-week-days');
  const monthDaysEl = qs('#modal-schedule-month-days');
  const summaryEl = qs('#modal-schedule-summary');
  const nextRunEl = qs('#modal-next-run');
  const tzSelect = qs('#modal-schedule-tz');
  const tzOther = qs('#modal-schedule-tz-other');

  const s0 = cronToSchedule(jt && jt.schedule_enabled ? jt.schedule_cron : null, jt && jt.schedule_tz ? jt.schedule_tz : 'UTC');
  scheduleBody.style.opacity = s0.enabled ? '1' : '0.6';
  qs('#modal-schedule-hour').value = s0.hour;
  qs('#modal-schedule-minute').value = s0.minute;
  repeatSelect.value = s0.repeat;
  weekDaysEl.querySelectorAll('input').forEach(cb => { cb.checked = s0.weekDays.includes(parseInt(cb.value, 10)); });
  monthDaysEl.querySelectorAll('input').forEach(cb => { cb.checked = s0.monthDays.includes(parseInt(cb.value, 10)); });
  if (['UTC', 'Europe/London', 'Europe/Paris', 'Europe/Budapest', 'America/New_York', 'America/Los_Angeles', 'Asia/Tokyo'].includes(s0.tz)) {
    tzSelect.value = s0.tz;
    tzOther.style.display = 'none';
  } else {
    tzSelect.value = '__other__';
    tzOther.value = s0.tz;
    tzOther.style.display = 'inline-block';
  }

  function getScheduleFromForm() {
    const tzVal = tzSelect.value === '__other__' ? tzOther.value.trim() || 'UTC' : tzSelect.value;
    const weekDays = Array.from(weekDaysEl.querySelectorAll('input:checked')).map(cb => parseInt(cb.value, 10));
    const monthDays = Array.from(monthDaysEl.querySelectorAll('input:checked')).map(cb => parseInt(cb.value, 10));
    return {
      enabled: scheduleEnabled.checked,
      hour: parseInt(qs('#modal-schedule-hour').value, 10),
      minute: parseInt(qs('#modal-schedule-minute').value, 10),
      repeat: repeatSelect.value,
      weekDays: repeatSelect.value === 'weekly' ? weekDays : [],
      monthDays: repeatSelect.value === 'monthly' ? monthDays : [],
      tz: tzVal,
    };
  }

  function updateSummary() {
    const s = getScheduleFromForm();
    if (!s.enabled) { summaryEl.textContent = ''; return; }
    const timeStr = `${String(s.hour).padStart(2, '0')}:${String(s.minute).padStart(2, '0')}`;
    if (s.repeat === 'daily') summaryEl.textContent = `Runs every day at ${timeStr} ${s.tz}`;
    else if (s.repeat === 'weekly' && s.weekDays.length) {
      const dayNames = ['', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
      const names = s.weekDays.sort((a,b)=>a-b).map(d => dayNames[d]).filter(Boolean).join(', ');
      summaryEl.textContent = `Runs every ${names} at ${timeStr} ${s.tz}`;
    } else if (s.repeat === 'monthly' && s.monthDays.length) {
      const days = s.monthDays.sort((a,b)=>a-b).join(', ');
      summaryEl.textContent = `Runs on day(s) ${days} at ${timeStr} ${s.tz}`;
    } else summaryEl.textContent = 'Pick days above';
  }

  function toggleScheduleUI() {
    const on = scheduleEnabled.checked;
    scheduleBody.style.opacity = on ? '1' : '0.6';
    scheduleBody.style.pointerEvents = on ? 'auto' : 'none';
    weekWrap.style.display = repeatSelect.value === 'weekly' ? 'block' : 'none';
    monthWrap.style.display = repeatSelect.value === 'monthly' ? 'block' : 'none';
    updateSummary();
    if (on && id) fetchJSON(`${API}/job_templates/${id}/next_run`).then(r => { nextRunEl.textContent = r.next_run ? new Date(r.next_run).toLocaleString() : '—'; }).catch(() => {});
    else if (on) nextRunEl.textContent = 'Save to see next run';
    else nextRunEl.textContent = '—';
  }

  scheduleEnabled.onchange = toggleScheduleUI;
  repeatSelect.onchange = () => { weekWrap.style.display = repeatSelect.value === 'weekly' ? 'block' : 'none'; monthWrap.style.display = repeatSelect.value === 'monthly' ? 'block' : 'none'; updateSummary(); };
  qs('#modal-schedule-hour').onchange = updateSummary;
  qs('#modal-schedule-minute').onchange = updateSummary;
  weekDaysEl.querySelectorAll('input').forEach(cb => { cb.onchange = updateSummary; });
  monthDaysEl.querySelectorAll('input').forEach(cb => { cb.onchange = updateSummary; });
  tzSelect.onchange = () => { tzOther.style.display = tzSelect.value === '__other__' ? 'inline-block' : 'none'; updateSummary(); };
  toggleScheduleUI();

  async function refreshPlaybookSelect() {
    const pid = parseInt(qs('#modal-tpl-project').value, 10);
    const sel = qs('#modal-playbook-select');
    const custom = qs('#modal-playbook-custom');
    sel.innerHTML = '<option value="">— Loading… —</option>';
    try {
      const data = await fetchJSON(`${API}/projects/${pid}/playbooks`);
      const ws = data.workspace || [];
      const cwdList = data.cwd || [];
      let html = '<option value="">— Select playbook or script —</option>';
      if (ws.length) {
        html += '<optgroup label="Project workspace (Git)">';
        ws.forEach(p => { html += `<option value="${escapeHtml(p)}">${escapeHtml(p)}</option>`; });
        html += '</optgroup>';
      }
      if (cwdList.length) {
        html += '<optgroup label="Server working directory">';
        cwdList.forEach(p => { html += `<option value="${escapeHtml(p)}">${escapeHtml(p)}</option>`; });
        html += '</optgroup>';
      }
      html += '<option value="__custom__">Custom path…</option>';
      sel.innerHTML = html;
      const allPaths = [...ws, ...cwdList];
      if (savedPath && allPaths.includes(savedPath)) sel.value = savedPath;
      else if (savedPath) {
        sel.value = '__custom__';
        custom.value = savedPath;
        custom.style.display = 'block';
      } else {
        custom.style.display = 'none';
        custom.value = '';
      }
    } catch (e) {
      console.error(e);
      sel.innerHTML = '<option value="">— Could not list playbooks —</option><option value="__custom__">Custom path…</option>';
      if (savedPath) {
        sel.value = '__custom__';
        custom.value = savedPath;
        custom.style.display = 'block';
      }
    }
    sel.onchange = () => {
      if (sel.value === '__custom__') {
        custom.style.display = 'block';
        if (!custom.value.trim() && savedPath) custom.value = savedPath;
      } else {
        custom.style.display = 'none';
      }
    };
  }

  qs('#modal-tpl-project').addEventListener('change', () => { refreshPlaybookSelect().catch(console.error); });
  await refreshPlaybookSelect();

  qs('#modal-save-tpl').onclick = async () => {
    const name = qs('#modal-name').value.trim();
    const selPb = qs('#modal-playbook-select');
    const customPb = qs('#modal-playbook-custom');
    const playbook_path = selPb.value === '__custom__' ? customPb.value.trim() : selPb.value.trim();
    const project_id = parseInt(qs('#modal-tpl-project').value, 10);
    const invVal = qs('#modal-inv').value;
    const credVal = qs('#modal-cred').value;
    const inventory_id = invVal ? parseInt(invVal, 10) : null;
    const credential_id = credVal ? parseInt(credVal, 10) : null;
    const extra_vars = qs('#modal-extra').value;
    const s = getScheduleFromForm();
    const schedule_enabled = s.enabled;
    const schedule_cron = schedule_enabled ? scheduleToCron(s) : null;
    const schedule_tz = schedule_enabled ? s.tz : null;
    if (!name || !playbook_path || !project_id) return;
    if (schedule_enabled && !schedule_cron) { alert('Pick at least one day for weekly/monthly schedule.'); return; }
    try {
      const body = { name, playbook_path, inventory_id, credential_id, extra_vars, schedule_enabled, schedule_cron, schedule_tz };
      if (id) await fetchJSON(`${API}/job_templates/${id}`, { method: 'PATCH', body: JSON.stringify(body) });
      else await fetchJSON(`${API}/job_templates`, { method: 'POST', body: JSON.stringify({ project_id, ...body }) });
      closeModal();
      reloadAndRender();
    } catch (e) { showError(e); }
  };
}

async function deleteProject(id) {
  if (!confirm('Delete this project and all its inventories, credentials, and templates?')) return;
  try {
    await fetchJSON(`${API}/projects/${id}`, { method: 'DELETE' });
    reloadAndRender();
  } catch (e) { showError(e); }
}

async function deleteInventory(id) {
  if (!confirm('Delete this inventory?')) return;
  try {
    await fetchJSON(`${API}/inventories/${id}`, { method: 'DELETE' });
    reloadAndRender();
  } catch (e) { showError(e); }
}

async function deleteCredential(id) {
  if (!confirm('Delete this credential?')) return;
  try {
    await fetchJSON(`${API}/credentials/${id}`, { method: 'DELETE' });
    reloadAndRender();
  } catch (e) { showError(e); }
}

async function deleteTemplate(id) {
  if (!confirm('Delete this job template?')) return;
  try {
    await fetchJSON(`${API}/job_templates/${id}`, { method: 'DELETE' });
    reloadAndRender();
  } catch (e) { showError(e); }
}

async function pullProject(id) {
  const p = projects.find(x => x.id === id);
  if (!p || !p.git_url) return;
  try {
    const res = await fetchJSON(`${API}/projects/${id}/pull`, { method: 'POST' });
    const list = (res.playbooks || []).length
      ? '<ul class="playbook-list">' + (res.playbooks || []).map(pb => '<li><code>' + escapeHtml(pb) + '</code></li>').join('') + '</ul>'
      : '<p class="empty-state">No supported files found. We look for: .yml, .yaml, .sh, .bash, .ps1, .bat, .cmd, .tf, .hcl, .py, .rb and similar (case-insensitive).</p>';
    showModal(
      'Pull from Git',
      `<p>${escapeHtml(res.message || 'Pulled successfully.')}</p><p><strong>Files found (use in Job Templates):</strong></p><p class="text-muted" style="font-size:0.85rem;margin-top:0.25rem;">.yml/.yaml = Ansible playbooks (run with inventory). .sh, .ps1, .py, etc. = scripts (run directly).</p>${list}`,
      '<button class="btn btn-primary" data-action="close-modal">Close</button>'
    );
  } catch (e) {
    showError(e);
  }
}

async function launchJob(templateId) {
  try {
    const job = await fetchJSON(`${API}/jobs/launch`, { method: 'POST', body: JSON.stringify({ job_template_id: templateId, extra_vars_override: '' }) });
    viewJob(job.id);
    reloadAndRender();
  } catch (e) { showError(e); }
}

async function deleteJob(id) {
  if (!confirm('Delete this job from history?')) return;
  try {
    await fetchJSON(`${API}/jobs/${id}`, { method: 'DELETE' });
    await reloadAndRender();
  } catch (e) { showError(e); }
}

async function deleteJobHistory() {
  if (!jobs.length) return;
  if (!confirm('Delete all jobs from history?')) return;
  try {
    await Promise.all(jobs.map(j => fetchJSON(`${API}/jobs/${j.id}`, { method: 'DELETE' })).reverse());
    await reloadAndRender();
  } catch (e) { showError(e); }
}
function jobModalBody(job) {
  return `
    <p><strong>Playbook:</strong> ${escapeHtml(job.playbook_path)}</p>
    <p><strong>Status:</strong> <span class="badge badge-${jobStatusBadgeClass(job.status)}">${escapeHtml(job.status)}</span></p>
    <p><strong>Started:</strong> ${job.started_at ? new Date(job.started_at).toLocaleString() : '—'}</p>
    <p><strong>Finished:</strong> ${job.finished_at ? new Date(job.finished_at).toLocaleString() : '—'}</p>
    <div class="form-group">
      <label>Output</label>
      <pre class="log-output">${escapeHtml(job.output_log || '(no output yet)')}</pre>
    </div>
  `;
}

function viewJob(id) {
  if (jobPollIntervalId) {
    clearInterval(jobPollIntervalId);
    jobPollIntervalId = null;
  }
  fetchJSON(`${API}/jobs/${id}`).then(job => {
    const modal = qs('#modal');
    modal.innerHTML = `
      <div class="modal-header">Job #${job.id} — ${escapeHtml(job.status)}</div>
      <div class="modal-body" id="job-modal-body">${jobModalBody(job)}</div>
      <div class="modal-footer"><button class="btn btn-primary" data-action="close-modal">Close</button></div>
    `;
    qs('#modal-overlay').classList.remove('hidden');

    const poll = () => {
      fetchJSON(`${API}/jobs/${id}`).then(j => {
        const header = modal.querySelector('.modal-header');
        const body = modal.querySelector('#job-modal-body');
        if (header) header.textContent = `Job #${j.id} — ${j.status}`;
        if (body) body.innerHTML = jobModalBody(j);
        if (j.status === 'success' || j.status === 'failed') {
          if (jobPollIntervalId) {
            clearInterval(jobPollIntervalId);
            jobPollIntervalId = null;
          }
          reloadAndRender();
        }
      }).catch(() => {});
    };

    if (job.status === 'pending' || job.status === 'running') {
      jobPollIntervalId = setInterval(poll, 1500);
    }
  }).catch(e => showError(e));
}

async function loadForPage(page = currentPage) {
  try {
    if (page === 'jobs') {
      jobs = await fetchJSON(`${API}/jobs?limit=100`);
      return;
    }

    projects = await fetchJSON(`${API}/projects`);

    if (page === 'ssh-deployer') {
      inventories = [];
      credentials = [];
      for (const p of projects) {
        const invList = await fetchJSON(`${API}/inventories?project_id=${p.id}`);
        inventories.push(...invList);
        const credList = await fetchJSON(`${API}/credentials?project_id=${p.id}`);
        credentials.push(...credList);
      }
      return;
    }

    // Keep arrays fresh only for pages that need them.
    if (page === 'dashboard' || page === 'templates') {
      jobTemplates = [];
      for (const p of projects) {
        const tplList = await fetchJSON(`${API}/job_templates?project_id=${p.id}`);
        jobTemplates.push(...tplList);
      }
    }
    if (page === 'inventories' || page === 'templates') {
      inventories = [];
      for (const p of projects) {
        const invList = await fetchJSON(`${API}/inventories?project_id=${p.id}`);
        inventories.push(...invList);
      }
    }
    if (page === 'credentials' || page === 'templates' || page === 'projects') {
      credentials = [];
      for (const p of projects) {
        const credList = await fetchJSON(`${API}/credentials?project_id=${p.id}`);
        credentials.push(...credList);
      }
    }
    if (page === 'dashboard') {
      jobs = await fetchJSON(`${API}/jobs?limit=100`);
    }
  } catch (e) {
    console.error(e);
    // Keep existing data so one failed poll doesn't wipe the UI
  }
}

async function reloadAndRender() {
  await loadForPage(currentPage);
  render();
}

// Init: nav + load data + render + auto-refresh
qsAll('.sidebar-nav a').forEach(a => {
  a.onclick = (e) => { e.preventDefault(); setPage(a.dataset.page); };
});
reloadAndRender().finally(() => {
  startRefresh();
});
