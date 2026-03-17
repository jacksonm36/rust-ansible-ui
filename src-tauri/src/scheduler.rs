//! Time-based scheduler: runs job templates on a cron-like schedule.

use crate::crud;
use crate::db::DbPool;
use chrono::{DateTime, Duration, Utc};
use cron::Schedule;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Duration as StdDuration;

static STOP: AtomicBool = AtomicBool::new(false);

lazy_static::lazy_static! {
    /// Last time we fired each template (to avoid double-run and to run after scheduled time).
    static ref LAST_RUN: Mutex<HashMap<i64, DateTime<Utc>>> = Mutex::new(HashMap::new());
}

pub fn start_scheduler(db: DbPool) {
    STOP.store(false, Ordering::SeqCst);
    thread::spawn(move || {
        tracing::info!("Job schedule checker started (every 60s)");
        while !STOP.load(Ordering::SeqCst) {
            if let Err(e) = tick(&db) {
                tracing::warn!("Scheduler tick error: {}", e);
            }
            thread::sleep(StdDuration::from_secs(60));
        }
    });
}

pub fn stop_scheduler() {
    STOP.store(true, Ordering::SeqCst);
}

fn next_run_utc(cron_expr: &str, _tz_name: &str) -> Option<DateTime<Utc>> {
    // Cron crate expects 6 fields (sec min hour day month dow). Our UI uses 5 (min hour day month dow).
    let expr = cron_expr.trim();
    let parts: Vec<&str> = expr.split_whitespace().collect();
    let expr = if parts.len() == 5 {
        format!("0 {}", expr)
    } else {
        expr.to_string()
    };
    let schedule = Schedule::from_str(&expr).ok()?;
    schedule.upcoming(Utc).next()
}

/// Run when we're within 0–90s *after* the scheduled time. Use "previous" occurrence (next - 1 min for minute cron) and cooldown.
fn tick(db: &DbPool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let templates = crud::get_scheduled_job_templates(db);
    let now = Utc::now();
    let mut last_run = LAST_RUN.lock().unwrap_or_else(|e| e.into_inner());

    for jt in templates {
        let cron_str = match jt.schedule_cron.as_ref().filter(|s| !s.is_empty()) {
            Some(s) => s,
            None => continue,
        };
        let _tz_name = jt.schedule_tz.as_deref().unwrap_or("UTC");
        let next = match next_run_utc(cron_str, _tz_name) {
            Some(t) => t,
            None => continue,
        };
        // Previous occurrence (for minute-level cron; approximation for daily/weekly).
        let prev = next - Duration::minutes(1);
        let window_end = prev + Duration::seconds(90);
        let in_window = now >= prev && now <= window_end;
        let cooldown_ok = last_run
            .get(&jt.id)
            .map(|&t| now.signed_duration_since(t).num_seconds() > 90)
            .unwrap_or(true);
        if in_window && cooldown_ok {
            tracing::info!("Scheduled run: template id={}", jt.id);
            last_run.insert(jt.id, now);
            drop(last_run);
            if let Some(launch) = crate::server::launch_job_template_by_id_impl(db, jt.id) {
                std::thread::spawn(move || launch());
            }
            last_run = LAST_RUN.lock().unwrap_or_else(|e| e.into_inner());
        }
    }
    Ok(())
}

pub fn next_run_iso(cron_expr: &str, tz_name: &str) -> Option<String> {
    next_run_utc(cron_expr, tz_name).map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}
