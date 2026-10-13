use serde::Serialize;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

pub struct Metrics {
    started: Instant,
    ready_ms: AtomicU64,
    events: AtomicU64,
    event_bytes: AtomicU64,
    pages: AtomicU64,
    page_bytes: AtomicU64,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            ready_ms: AtomicU64::new(0),
            events: AtomicU64::new(0),
            event_bytes: AtomicU64::new(0),
            pages: AtomicU64::new(0),
            page_bytes: AtomicU64::new(0),
        }
    }
    pub fn ready(&self) {
        let _ = self.ready_ms.compare_exchange(
            0,
            self.started.elapsed().as_millis().max(1) as u64,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
    }
    pub fn event(&self, value: &impl Serialize) {
        self.events.fetch_add(1, Ordering::Relaxed);
        if let Ok(value) = serde_json::to_vec(value) {
            self.event_bytes
                .fetch_add(value.len() as u64, Ordering::Relaxed);
        }
    }
    pub fn page(&self, value: &impl Serialize) {
        self.pages.fetch_add(1, Ordering::Relaxed);
        if let Ok(value) = serde_json::to_vec(value) {
            self.page_bytes
                .fetch_add(value.len() as u64, Ordering::Relaxed);
        }
    }
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let index = crate::index::Index::open(path)?;
        index.conn.execute_batch("CREATE TABLE IF NOT EXISTS runtime_metrics(id INTEGER PRIMARY KEY CHECK(id=1),value TEXT NOT NULL)").map_err(|e|e.to_string())?;
        self.save_connection(&index.conn)
    }
    pub fn save_connection(&self, connection: &rusqlite::Connection) -> Result<(), String> {
        let value=serde_json::json!({"pid":std::process::id(),"frontendReadyMs":self.ready_ms.load(Ordering::Relaxed),"progressEvents":self.events.load(Ordering::Relaxed),"progressPayloadBytes":self.event_bytes.load(Ordering::Relaxed),"pageReplies":self.pages.load(Ordering::Relaxed),"pagePayloadBytes":self.page_bytes.load(Ordering::Relaxed)}).to_string();
        connection.execute("INSERT INTO runtime_metrics VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET value=excluded.value",[value]).map_err(|e|e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_never_wait_for_the_scanner_writer_on_a_page_reply() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("fixture.sqlite");
        let metrics = Metrics::new();
        metrics.save(&path).unwrap();
        let writer = crate::index::Index::open(&path).unwrap();
        writer.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        let reader = crate::index::Index::open(&path).unwrap();
        reader.conn.busy_timeout(std::time::Duration::ZERO).unwrap();
        metrics.page(&serde_json::json!({"fixture":true}));
        let started = Instant::now();
        assert!(metrics.save_connection(&reader.conn).is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!(
            reader
                .conn
                .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        writer.conn.execute_batch("ROLLBACK").unwrap();
        metrics.save_connection(&reader.conn).unwrap();
        let value: String = reader
            .conn
            .query_row("SELECT value FROM runtime_metrics", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&value).unwrap()["pageReplies"],
            1
        );
    }
}
