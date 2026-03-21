//! Time-based scheduler: runs job templates on a cron-like schedule.
//!
//! Fires once per cron **slot** when the server notices within a grace window after that slot.
//! Last-fired slot is stored in SQLite (`schedule_last_fire_utc`) so restarts do not double-run.
//! **Limitation:** `schedule_cron` is evaluated in **UTC**; `schedule_tz` is used for API/display
//! and resets last-fire when changed — full timezone-aware cron is not implemented yet.

use crate::crud;
use crate::db::DbPool;
use chrono::{DateTime, Duration, Utc};
use cron::Schedule;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration as StdDuration;

static STOP: AtomicBool = AtomicBool::new(false);

/// How long after a scheduled instant we still accept firing (matches ~3× 60s tick interval).
const GRACE_AFTER_SLOT_SECS: i64 = 180;

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

fn parse_schedule(cron_expr: &str) -> Option<Schedule> {
    let expr = cron_expr.trim();
    let parts: Vec<&str> = expr.split_whitespace().collect();
    let expr = if parts.len() == 5 {
        format!("0 {}", expr)
    } else {
        expr.to_string()
    };
    Schedule::from_str(&expr).ok()
}

fn next_run_utc(cron_expr: &str, _tz_name: &str) -> Option<DateTime<Utc>> {
    let schedule = parse_schedule(cron_expr)?;
    schedule.upcoming(Utc).next()
}

/// Latest cron occurrence in `(now - grace, now]`, if any.
fn latest_due_slot(schedule: &Schedule, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let window_start = now - Duration::seconds(GRACE_AFTER_SLOT_SECS);
    let mut latest: Option<DateTime<Utc>> = None;
    for t in schedule.after(&window_start) {
        if t > now {
            break;
        }
        latest = Some(t);
    }
    latest
}

fn tick(db: &DbPool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let templates = crud::get_scheduled_job_templates(db);
    let now = Utc::now();

    for jt in templates {
        let cron_str = match jt.schedule_cron.as_ref().filter(|s| !s.is_empty()) {
            Some(s) => s,
            None => continue,
        };
        let _tz_name = jt.schedule_tz.as_deref().unwrap_or("UTC");
        let schedule = match parse_schedule(cron_str) {
            Some(s) => s,
            None => continue,
        };
        let slot = match latest_due_slot(&schedule, now) {
            Some(s) => s,
            None => continue,
        };

        let late_secs = now.signed_duration_since(slot).num_seconds();
        if late_secs > GRACE_AFTER_SLOT_SECS {
            continue;
        }

        let last = jt.schedule_last_fire_utc.as_deref().and_then(|s| {
            DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|d| d.with_timezone(&Utc))
        });
        if last.map(|l| l >= slot).unwrap_or(false) {
            continue;
        }

        tracing::info!(
            "Scheduled run: template id={} slot={}",
            jt.id,
            slot.to_rfc3339()
        );
        crud::set_job_template_schedule_last_fire(db, jt.id, Some(&slot.to_rfc3339()))?;

        if let Some(launch) = crate::server::launch_job_template_by_id_impl(db, jt.id) {
            std::thread::spawn(launch);
        }
    }
    Ok(())
}

pub fn next_run_iso(cron_expr: &str, tz_name: &str) -> Option<String> {
    next_run_utc(cron_expr, tz_name).map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}
