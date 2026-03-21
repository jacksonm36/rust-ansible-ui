//! CRUD operations for projects, inventories, credentials, job_templates, jobs.

use crate::db::{utc_now_iso, DbPool};
use crate::schemas::*;
use rusqlite::params;
use std::sync::MutexGuard;

fn conn(db: &DbPool) -> MutexGuard<'_, rusqlite::Connection> {
    db.lock().unwrap_or_else(|e| e.into_inner())
}

// --- Projects ---
pub fn get_project(db: &DbPool, id: i64) -> Option<(i64, String, String, Option<String>, Option<String>, Option<i64>, String, String)> {
    let c = conn(db);
    let mut stmt = c.prepare(
        "SELECT id, name, description, git_url, git_branch, git_credential_id, created_at, updated_at FROM projects WHERE id = ?1",
    ).ok()?;
    let row = stmt.query_row(params![id], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<i64>>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, String>(7)?,
        ))
    }).ok()?;
    Some(row)
}

pub fn get_projects(db: &DbPool) -> Vec<(i64, String, String, Option<String>, Option<String>, Option<i64>, String, String)> {
    let c = conn(db);
    let mut stmt = match c.prepare(
        "SELECT id, name, description, git_url, git_branch, git_credential_id, created_at, updated_at FROM projects ORDER BY name",
    ) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let rows = match stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<i64>>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, String>(7)?,
        ))
    }) {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    rows.filter_map(Result::ok).collect()
}

pub fn create_project(db: &DbPool, data: &ProjectCreate) -> Result<ProjectRead, rusqlite::Error> {
    let now = utc_now_iso();
    let c = conn(db);
    c.execute(
        "INSERT INTO projects (name, description, git_url, git_branch, git_credential_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            data.name,
            data.description.as_deref().unwrap_or(""),
            data.git_url,
            data.git_branch.as_deref().unwrap_or("main"),
            data.git_credential_id,
            now,
            now,
        ],
    )?;
    let id = c.last_insert_rowid();
    Ok(ProjectRead {
        id,
        name: data.name.clone(),
        description: data.description.clone().unwrap_or_default(),
        git_url: data.git_url.clone(),
        git_branch: data.git_branch.clone().or_else(|| Some("main".into())),
        git_credential_id: data.git_credential_id,
        created_at: now.clone(),
        updated_at: now,
    })
}

pub fn update_project(db: &DbPool, id: i64, data: &ProjectUpdate) -> Option<ProjectRead> {
    let p = get_project(db, id)?;
    let name = data.name.as_deref().unwrap_or(&p.1);
    let description = data.description.as_deref().unwrap_or(&p.2).to_string();
    let git_url = data.git_url.as_ref().or(p.3.as_ref()).cloned();
    let git_branch = data.git_branch.as_deref().or(p.4.as_deref()).map(|s| s.to_string()).unwrap_or_else(|| "main".into());
    let git_credential_id = data.git_credential_id.or(p.5);
    let now = utc_now_iso();
    let c = conn(db);
    c.execute(
        "UPDATE projects SET name=?1, description=?2, git_url=?3, git_branch=?4, git_credential_id=?5, updated_at=?6 WHERE id=?7",
        params![name, description, git_url, git_branch, git_credential_id, now, id],
    ).ok()?;
    Some(ProjectRead {
        id,
        name: name.to_string(),
        description,
        git_url,
        git_branch: Some(git_branch),
        git_credential_id,
        created_at: p.6,
        updated_at: now,
    })
}

pub fn delete_project(db: &DbPool, id: i64) -> bool {
    conn(db).execute("DELETE FROM projects WHERE id = ?1", params![id]).ok().map(|n| n > 0).unwrap_or(false)
}

// --- Inventories ---
pub fn get_inventory(db: &DbPool, id: i64) -> Option<InventoryRead> {
    let c = conn(db);
    let mut stmt = c.prepare("SELECT id, project_id, name, description, content, created_at, updated_at FROM inventories WHERE id = ?1").ok()?;
    stmt.query_row(params![id], |r| {
        Ok(InventoryRead {
            id: r.get(0)?,
            project_id: r.get(1)?,
            name: r.get(2)?,
            description: r.get(3)?,
            content: r.get(4)?,
            created_at: r.get(5)?,
            updated_at: r.get(6)?,
        })
    }).ok()
}

pub fn get_inventories_by_project(db: &DbPool, project_id: i64) -> Vec<InventoryRead> {
    let c = conn(db);
    let mut stmt = match c.prepare("SELECT id, project_id, name, description, content, created_at, updated_at FROM inventories WHERE project_id = ?1 ORDER BY name") {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let rows = match stmt.query_map(params![project_id], |r| {
        Ok(InventoryRead {
            id: r.get(0)?,
            project_id: r.get(1)?,
            name: r.get(2)?,
            description: r.get(3)?,
            content: r.get(4)?,
            created_at: r.get(5)?,
            updated_at: r.get(6)?,
        })
    }) {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    rows.filter_map(Result::ok).collect()
}

pub fn create_inventory(db: &DbPool, data: &InventoryCreate) -> Result<InventoryRead, rusqlite::Error> {
    let now = utc_now_iso();
    let c = conn(db);
    c.execute(
        "INSERT INTO inventories (project_id, name, description, content, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            data.project_id,
            data.name,
            data.description.as_deref().unwrap_or(""),
            data.content.as_deref().unwrap_or(""),
            now,
            now,
        ],
    )?;
    let id = c.last_insert_rowid();
    Ok(InventoryRead {
        id,
        project_id: data.project_id,
        name: data.name.clone(),
        description: data.description.clone().unwrap_or_default(),
        content: data.content.clone().unwrap_or_default(),
        created_at: now.clone(),
        updated_at: now,
    })
}

pub fn update_inventory(db: &DbPool, id: i64, data: &InventoryUpdate) -> Option<InventoryRead> {
    let inv = get_inventory(db, id)?;
    let name = data.name.as_deref().unwrap_or(&inv.name);
    let description = data.description.as_deref().unwrap_or(&inv.description).to_string();
    let content = data.content.as_deref().unwrap_or(&inv.content).to_string();
    let now = utc_now_iso();
    conn(db).execute(
        "UPDATE inventories SET name=?1, description=?2, content=?3, updated_at=?4 WHERE id=?5",
        params![name, description, content, now, id],
    ).ok()?;
    Some(InventoryRead {
        id,
        project_id: inv.project_id,
        name: name.to_string(),
        description,
        content,
        created_at: inv.created_at,
        updated_at: now,
    })
}

pub fn delete_inventory(db: &DbPool, id: i64) -> bool {
    conn(db).execute("DELETE FROM inventories WHERE id = ?1", params![id]).ok().map(|n| n > 0).unwrap_or(false)
}

// --- Credentials ---
pub fn get_credential(db: &DbPool, id: i64) -> Option<CredentialRead> {
    let c = conn(db);
    let mut stmt = c.prepare("SELECT id, project_id, name, kind, extra, created_at, updated_at FROM credentials WHERE id = ?1").ok()?;
    stmt.query_row(params![id], |r| {
        Ok(CredentialRead {
            id: r.get(0)?,
            project_id: r.get(1)?,
            name: r.get(2)?,
            kind: r.get(3)?,
            extra: r.get(4)?,
            created_at: r.get(5)?,
            updated_at: r.get(6)?,
        })
    }).ok()
}

pub fn get_credentials_by_project(db: &DbPool, project_id: i64) -> Vec<CredentialRead> {
    let c = conn(db);
    let mut stmt = match c.prepare("SELECT id, project_id, name, kind, extra, created_at, updated_at FROM credentials WHERE project_id = ?1 ORDER BY name") {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let rows = match stmt.query_map(params![project_id], |r| {
        Ok(CredentialRead {
            id: r.get(0)?,
            project_id: r.get(1)?,
            name: r.get(2)?,
            kind: r.get(3)?,
            extra: r.get(4)?,
            created_at: r.get(5)?,
            updated_at: r.get(6)?,
        })
    }) {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    rows.filter_map(Result::ok).collect()
}

pub fn create_credential(db: &DbPool, data: &CredentialCreate, secret_encrypted: &str) -> Result<CredentialRead, rusqlite::Error> {
    let now = utc_now_iso();
    let kind = data.kind.as_deref().unwrap_or("ssh");
    let c = conn(db);
    c.execute(
        "INSERT INTO credentials (project_id, name, kind, secret_encrypted, extra, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![data.project_id, data.name, kind, secret_encrypted, data.extra.as_deref().unwrap_or(""), now, now],
    )?;
    let id = c.last_insert_rowid();
    Ok(CredentialRead {
        id,
        project_id: data.project_id,
        name: data.name.clone(),
        kind: kind.to_string(),
        extra: data.extra.clone().unwrap_or_default(),
        created_at: now.clone(),
        updated_at: now,
    })
}

pub fn update_credential(db: &DbPool, id: i64, data: &CredentialUpdate, secret_encrypted: Option<&str>) -> Option<CredentialRead> {
    let c = get_credential(db, id)?;
    let name = data.name.as_deref().unwrap_or(&c.name);
    let kind = data.kind.as_deref().unwrap_or(&c.kind);
    let extra = data.extra.as_deref().unwrap_or(&c.extra);
    let now = utc_now_iso();
    let conn = conn(db);
    if let Some(sec) = secret_encrypted {
        conn.execute(
            "UPDATE credentials SET name=?1, kind=?2, extra=?3, secret_encrypted=?4, updated_at=?5 WHERE id=?6",
            params![name, kind, extra, sec, now, id],
        ).ok()?;
    } else {
        conn.execute(
            "UPDATE credentials SET name=?1, kind=?2, extra=?3, updated_at=?4 WHERE id=?5",
            params![name, kind, extra, now, id],
        ).ok()?;
    }
    Some(CredentialRead {
        id,
        project_id: c.project_id,
        name: name.to_string(),
        kind: kind.to_string(),
        extra: extra.to_string(),
        created_at: c.created_at,
        updated_at: now,
    })
}

pub fn get_credential_secret(db: &DbPool, id: i64) -> Option<String> {
    let c = conn(db);
    let enc: String = c.query_row("SELECT secret_encrypted FROM credentials WHERE id = ?1", params![id], |r| r.get(0)).ok()?;
    if enc.is_empty() {
        return Some(String::new());
    }
    crate::secrets::decrypt_secret(&enc).ok()
}

pub fn delete_credential(db: &DbPool, id: i64) -> bool {
    conn(db).execute("DELETE FROM credentials WHERE id = ?1", params![id]).ok().map(|n| n > 0).unwrap_or(false)
}

// --- Job templates ---
pub fn get_job_template(db: &DbPool, id: i64) -> Option<JobTemplateRead> {
    let c = conn(db);
    let mut stmt = c.prepare(
        "SELECT id, project_id, name, description, playbook_path, inventory_id, credential_id, extra_vars, schedule_enabled, schedule_cron, schedule_tz, created_at, updated_at, schedule_last_fire_utc FROM job_templates WHERE id = ?1",
    ).ok()?;
    stmt.query_row(params![id], |r| {
        Ok(JobTemplateRead {
            id: r.get(0)?,
            project_id: r.get(1)?,
            name: r.get(2)?,
            description: r.get(3)?,
            playbook_path: r.get(4)?,
            inventory_id: r.get(5)?,
            credential_id: r.get(6)?,
            extra_vars: r.get(7)?,
            schedule_enabled: r.get::<_, i64>(8)? != 0,
            schedule_cron: r.get(9)?,
            schedule_tz: r.get(10)?,
            schedule_last_fire_utc: r.get(13)?,
            created_at: r.get(11)?,
            updated_at: r.get(12)?,
        })
    }).ok()
}

pub fn get_job_templates_by_project(db: &DbPool, project_id: i64) -> Vec<JobTemplateRead> {
    let c = conn(db);
    let mut stmt = match c.prepare(
        "SELECT id, project_id, name, description, playbook_path, inventory_id, credential_id, extra_vars, schedule_enabled, schedule_cron, schedule_tz, created_at, updated_at, schedule_last_fire_utc FROM job_templates WHERE project_id = ?1 ORDER BY name",
    ) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let rows = match stmt.query_map(params![project_id], |r| {
        Ok(JobTemplateRead {
            id: r.get(0)?,
            project_id: r.get(1)?,
            name: r.get(2)?,
            description: r.get(3)?,
            playbook_path: r.get(4)?,
            inventory_id: r.get(5)?,
            credential_id: r.get(6)?,
            extra_vars: r.get(7)?,
            schedule_enabled: r.get::<_, i64>(8)? != 0,
            schedule_cron: r.get(9)?,
            schedule_tz: r.get(10)?,
            schedule_last_fire_utc: r.get(13)?,
            created_at: r.get(11)?,
            updated_at: r.get(12)?,
        })
    }) {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    rows.filter_map(Result::ok).collect()
}

pub fn get_scheduled_job_templates(db: &DbPool) -> Vec<JobTemplateRead> {
    let c = conn(db);
    let mut stmt = match c.prepare(
        "SELECT id, project_id, name, description, playbook_path, inventory_id, credential_id, extra_vars, schedule_enabled, schedule_cron, schedule_tz, created_at, updated_at, schedule_last_fire_utc FROM job_templates WHERE schedule_enabled = 1 AND schedule_cron IS NOT NULL AND schedule_cron != ''",
    ) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let rows = match stmt.query_map([], |r| {
        Ok(JobTemplateRead {
            id: r.get(0)?,
            project_id: r.get(1)?,
            name: r.get(2)?,
            description: r.get(3)?,
            playbook_path: r.get(4)?,
            inventory_id: r.get(5)?,
            credential_id: r.get(6)?,
            extra_vars: r.get(7)?,
            schedule_enabled: true,
            schedule_cron: r.get(9)?,
            schedule_tz: r.get(10)?,
            schedule_last_fire_utc: r.get(13)?,
            created_at: r.get(11)?,
            updated_at: r.get(12)?,
        })
    }) {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    rows.filter_map(Result::ok).collect()
}

pub fn create_job_template(db: &DbPool, data: &JobTemplateCreate) -> Result<JobTemplateRead, rusqlite::Error> {
    let now = utc_now_iso();
    let c = conn(db);
    c.execute(
        "INSERT INTO job_templates (project_id, name, description, playbook_path, inventory_id, credential_id, extra_vars, schedule_enabled, schedule_cron, schedule_tz, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            data.project_id,
            data.name,
            data.description.as_deref().unwrap_or(""),
            data.playbook_path,
            data.inventory_id,
            data.credential_id,
            data.extra_vars.as_deref().unwrap_or(""),
            if data.schedule_enabled.unwrap_or(false) { 1i64 } else { 0 },
            data.schedule_cron,
            data.schedule_tz.as_deref().unwrap_or("UTC"),
            now,
            now,
        ],
    )?;
    let id = c.last_insert_rowid();
    Ok(JobTemplateRead {
        id,
        project_id: data.project_id,
        name: data.name.clone(),
        description: data.description.clone().unwrap_or_default(),
        playbook_path: data.playbook_path.clone(),
        inventory_id: data.inventory_id,
        credential_id: data.credential_id,
        extra_vars: data.extra_vars.clone().unwrap_or_default(),
        schedule_enabled: data.schedule_enabled.unwrap_or(false),
        schedule_cron: data.schedule_cron.clone(),
        schedule_tz: data.schedule_tz.clone().or_else(|| Some("UTC".into())),
        schedule_last_fire_utc: None,
        created_at: now.clone(),
        updated_at: now,
    })
}

pub fn update_job_template(db: &DbPool, id: i64, data: &JobTemplateUpdate) -> Option<JobTemplateRead> {
    let jt = get_job_template(db, id)?;
    let name = data.name.as_deref().unwrap_or(&jt.name);
    let description = data.description.as_deref().unwrap_or(&jt.description).to_string();
    let playbook_path = data.playbook_path.as_deref().unwrap_or(&jt.playbook_path).to_string();
    let inventory_id = data.inventory_id.or(jt.inventory_id);
    let credential_id = data.credential_id.or(jt.credential_id);
    let extra_vars = data.extra_vars.as_deref().unwrap_or(&jt.extra_vars).to_string();
    let schedule_enabled = data.schedule_enabled.unwrap_or(jt.schedule_enabled);
    let schedule_cron = data.schedule_cron.clone().or(jt.schedule_cron.clone());
    let schedule_tz = data.schedule_tz.clone().or(jt.schedule_tz.clone());
    let cron_changed = data
        .schedule_cron
        .as_ref()
        .is_some_and(|c| jt.schedule_cron.as_ref() != Some(c));
    let tz_changed = data
        .schedule_tz
        .as_ref()
        .is_some_and(|t| jt.schedule_tz.as_ref() != Some(t));
    let reset_last_fire = !schedule_enabled || cron_changed || tz_changed;
    let now = utc_now_iso();
    conn(db).execute(
        "UPDATE job_templates SET name=?1, description=?2, playbook_path=?3, inventory_id=?4, credential_id=?5, extra_vars=?6, schedule_enabled=?7, schedule_cron=?8, schedule_tz=?9, updated_at=?10 WHERE id=?11",
        params![name, description, playbook_path, inventory_id, credential_id, extra_vars, if schedule_enabled { 1i64 } else { 0 }, schedule_cron, schedule_tz.as_deref().unwrap_or("UTC"), now, id],
    ).ok()?;
    if reset_last_fire {
        conn(db)
            .execute(
                "UPDATE job_templates SET schedule_last_fire_utc = NULL WHERE id = ?1",
                params![id],
            )
            .ok();
    }
    let last_fire = if reset_last_fire {
        None
    } else {
        jt.schedule_last_fire_utc.clone()
    };
    Some(JobTemplateRead {
        id,
        project_id: jt.project_id,
        name: name.to_string(),
        description,
        playbook_path,
        inventory_id,
        credential_id,
        extra_vars,
        schedule_enabled,
        schedule_cron,
        schedule_tz,
        schedule_last_fire_utc: last_fire,
        created_at: jt.created_at,
        updated_at: now,
    })
}

/// Persist the schedule slot (RFC3339 UTC) that was just fired, so restarts and multi-instance avoid duplicate runs.
pub fn set_job_template_schedule_last_fire(
    db: &DbPool,
    id: i64,
    fire_rfc3339: Option<&str>,
) -> Result<(), rusqlite::Error> {
    conn(db).execute(
        "UPDATE job_templates SET schedule_last_fire_utc = ?1 WHERE id = ?2",
        params![fire_rfc3339, id],
    )?;
    Ok(())
}

pub fn delete_job_template(db: &DbPool, id: i64) -> bool {
    conn(db).execute("DELETE FROM job_templates WHERE id = ?1", params![id]).ok().map(|n| n > 0).unwrap_or(false)
}

// --- Jobs ---
pub fn get_job(db: &DbPool, id: i64) -> Option<JobRead> {
    let c = conn(db);
    let mut stmt = c.prepare(
        "SELECT id, project_id, job_template_id, status, playbook_path, extra_vars, output_log, started_at, finished_at, created_at FROM jobs WHERE id = ?1",
    ).ok()?;
    stmt.query_row(params![id], |r| {
        Ok(JobRead {
            id: r.get(0)?,
            project_id: r.get(1)?,
            job_template_id: r.get(2)?,
            status: r.get(3)?,
            playbook_path: r.get(4)?,
            extra_vars: r.get(5)?,
            output_log: r.get(6)?,
            started_at: r.get(7)?,
            finished_at: r.get(8)?,
            created_at: r.get(9)?,
        })
    }).ok()
}

pub fn get_jobs_by_project(db: &DbPool, project_id: i64, limit: i64) -> Vec<JobListSummary> {
    let c = conn(db);
    let mut stmt = match c.prepare(
        "SELECT id, project_id, job_template_id, status, playbook_path, started_at, finished_at, created_at FROM jobs WHERE project_id = ?1 ORDER BY created_at DESC LIMIT ?2",
    ) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let rows = match stmt.query_map(params![project_id, limit], |r| {
        Ok(JobListSummary {
            id: r.get(0)?,
            project_id: r.get(1)?,
            job_template_id: r.get(2)?,
            status: r.get(3)?,
            playbook_path: r.get(4)?,
            started_at: r.get(5)?,
            finished_at: r.get(6)?,
            created_at: r.get(7)?,
        })
    }) {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    rows.filter_map(Result::ok).collect()
}

pub fn get_recent_jobs(db: &DbPool, limit: i64) -> Vec<JobListSummary> {
    let c = conn(db);
    let mut stmt = match c.prepare(
        "SELECT id, project_id, job_template_id, status, playbook_path, started_at, finished_at, created_at FROM jobs ORDER BY created_at DESC LIMIT ?1",
    ) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let rows = match stmt.query_map(params![limit], |r| {
        Ok(JobListSummary {
            id: r.get(0)?,
            project_id: r.get(1)?,
            job_template_id: r.get(2)?,
            status: r.get(3)?,
            playbook_path: r.get(4)?,
            started_at: r.get(5)?,
            finished_at: r.get(6)?,
            created_at: r.get(7)?,
        })
    }) {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    rows.filter_map(Result::ok).collect()
}

pub fn create_job(
    db: &DbPool,
    project_id: i64,
    job_template_id: Option<i64>,
    playbook_path: &str,
    inventory_content: &str,
    extra_vars: &str,
    status: &str,
) -> Result<JobRead, rusqlite::Error> {
    let now = utc_now_iso();
    let c = conn(db);
    c.execute(
        "INSERT INTO jobs (project_id, job_template_id, status, playbook_path, inventory_content, extra_vars, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![project_id, job_template_id, status, playbook_path, inventory_content, extra_vars, now],
    )?;
    let id = c.last_insert_rowid();
    Ok(JobRead {
        id,
        project_id,
        job_template_id,
        status: status.to_string(),
        playbook_path: playbook_path.to_string(),
        extra_vars: extra_vars.to_string(),
        output_log: String::new(),
        started_at: None,
        finished_at: None,
        created_at: now,
    })
}

pub fn update_job_status(db: &DbPool, id: i64, status: &str, output_log: &str) -> Option<()> {
    let now = utc_now_iso();
    let c = conn(db);
    if status == "running" {
        c.execute("UPDATE jobs SET status=?1, output_log=?2, started_at=?3 WHERE id=?4", params![status, output_log, now, id]).ok()?;
    } else if status == "success" || status == "failed" {
        c.execute("UPDATE jobs SET status=?1, output_log=?2, finished_at=?3 WHERE id=?4", params![status, output_log, now, id]).ok()?;
    } else {
        c.execute("UPDATE jobs SET status=?1, output_log=?2 WHERE id=?3", params![status, output_log, id]).ok()?;
    }
    Some(())
}

pub fn delete_job(db: &DbPool, id: i64) -> bool {
    conn(db).execute("DELETE FROM jobs WHERE id = ?1", params![id]).ok().map(|n| n > 0).unwrap_or(false)
}

#[allow(dead_code)]
pub fn delete_all_jobs(db: &DbPool) -> u64 {
    conn(db).execute("DELETE FROM jobs", []).ok().unwrap_or(0) as u64
}
