use std::fs;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Instant;
use trapclean_lib::{index::Index, scanner};

fn main() -> Result<(), String> {
    let arguments: Vec<String> = std::env::args().collect();
    let entries = arguments
        .get(1)
        .map(|v| v.parse::<usize>())
        .transpose()
        .map_err(|e| e.to_string())?
        .unwrap_or(100_000);
    let temp = tempfile::tempdir().map_err(|e| e.to_string())?;
    let supplied_root = arguments.get(2).map(PathBuf::from);
    let root: PathBuf = supplied_root
        .clone()
        .unwrap_or_else(|| temp.path().join("fixture"));
    if supplied_root.is_none() {
        fs::create_dir(&root).map_err(|e| e.to_string())?;
    }
    let generation = Instant::now();
    if supplied_root.is_none() {
        for batch in 0..entries.div_ceil(1000) {
            let dir = root.join(format!("group-{batch:04}"));
            fs::create_dir(&dir).map_err(|e| e.to_string())?;
            for i in 0..1000.min(entries - batch * 1000) {
                fs::write(dir.join(format!("entry-{i:04}.dat")), [0u8; 32])
                    .map_err(|e| e.to_string())?;
            }
        }
    }
    eprintln!(
        "fixture generation: {:.3}s",
        generation.elapsed().as_secs_f64()
    );
    let db = temp.path().join("scan.sqlite");
    let index = Index::open(&db)?;
    let id = index.begin_scan(&root)?;
    let started = Instant::now();
    let mut events = 0u64;
    let mut event_bytes = 0usize;
    let summary = scanner::scan(&db, id, Arc::new(AtomicBool::new(false)), |summary| {
        events += 1;
        event_bytes += serde_json::to_vec(&summary).unwrap().len();
    })?;
    let duration = started.elapsed().as_secs_f64();
    let page_start = Instant::now();
    let page = index.get_entries(id, Some(summary.root_id), 0, 50, "size", "", false)?;
    let page_ms = page_start.elapsed().as_secs_f64() * 1000.0;
    let cancel = Arc::new(AtomicBool::new(false));
    let cancelled = cancel.clone();
    let id = index.begin_scan(&root)?;
    let cancelled_at = Arc::new(std::sync::Mutex::new(None));
    let at = cancelled_at.clone();
    let cancel_result = scanner::scan(&db, id, cancel, |summary| {
        if summary.entries > 1 && at.lock().unwrap().is_none() {
            *at.lock().unwrap() = Some(Instant::now());
            cancelled.store(true, Ordering::Relaxed);
        }
    })?;
    let cancel_ms = cancelled_at
        .lock()
        .unwrap()
        .map(|value| value.elapsed().as_secs_f64() * 1000.0);
    #[cfg(unix)]
    let max_rss = {
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
        #[cfg(target_os = "macos")]
        let bytes = usage.ru_maxrss as u64;
        #[cfg(not(target_os = "macos"))]
        let bytes = usage.ru_maxrss as u64 * 1024;
        bytes
    };
    #[cfg(not(unix))]
    let max_rss: Option<u64> = None;
    println!(
        "{}",
        serde_json::json!({"fixtureFiles":entries,"indexedEntries":summary.entries,"status":summary.status,"seconds":duration,"entriesPerSecond":summary.entries as f64/duration,"progressEvents":events,"ipcSummaryBytes":event_bytes,"rootPageMs":page_ms,"rootPageItems":page.items.len(),"peakRssBytes":max_rss,"cancelStatus":cancel_result.status,"cancelMs":cancel_ms,"databaseBytes":fs::metadata(&db).map_err(|e|e.to_string())?.len()})
    );
    Ok(())
}
