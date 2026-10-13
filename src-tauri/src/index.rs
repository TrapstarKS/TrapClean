use crate::model::{EntryPage, EntryRow, ScanSummary};
use crate::scanner::{self, Fingerprint};
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub struct Index {
    pub(crate) conn: Connection,
}

#[derive(Clone, Debug)]
pub struct IndexedEntry {
    pub row: EntryRow,
    pub path: PathBuf,
    pub fingerprint: Fingerprint,
    pub protected: bool,
}

const ENTRY_COLUMNS: &str = "id,parent_id,name,relative_path,kind,logical_bytes,allocated_bytes,modified_at,accessed_at,complete,error,category,disposition,reason,consequence,uncertainty,project,evidence,removable";

pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(unix)]
pub(crate) fn encode_path(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(unix)]
pub(crate) fn decode_path(bytes: Vec<u8>) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    std::ffi::OsString::from_vec(bytes).into()
}

#[cfg(windows)]
pub(crate) fn encode_path(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(windows)]
pub(crate) fn decode_path(bytes: Vec<u8>) -> PathBuf {
    use std::os::windows::ffi::OsStringExt;
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes(*b))
        .collect();
    std::ffi::OsString::from_wide(&units).into()
}

fn entry_from_row(row: &Row<'_>) -> rusqlite::Result<EntryRow> {
    Ok(EntryRow {
        id: row.get(0)?,
        parent_id: row.get(1)?,
        name: row.get(2)?,
        relative_path: row.get(3)?,
        kind: row.get(4)?,
        logical_bytes: row.get::<_, i64>(5)?.max(0) as u64,
        allocated_bytes: row.get::<_, i64>(6)?.max(0) as u64,
        modified_at: row.get(7)?,
        accessed_at: row.get(8)?,
        complete: row.get(9)?,
        error: row.get(10)?,
        category: row.get(11)?,
        disposition: row.get(12)?,
        reason: row.get(13)?,
        consequence: row.get(14)?,
        uncertainty: row.get(15)?,
        project: row.get(16)?,
        evidence: serde_json::from_str(&row.get::<_, String>(17)?).unwrap_or_default(),
        removable: row.get(18)?,
    })
}

fn scan_from_row(row: &Row<'_>) -> rusqlite::Result<ScanSummary> {
    Ok(ScanSummary {
        id: row.get(0)?,
        root: row.get(1)?,
        root_id: row.get(2)?,
        status: row.get(3)?,
        entries: row.get::<_, i64>(4)?.max(0) as u64,
        logical_bytes: row.get::<_, i64>(5)?.max(0) as u64,
        allocated_bytes: row.get::<_, i64>(6)?.max(0) as u64,
        allocated_estimated: row.get(7)?,
        candidate_bytes: row.get::<_, i64>(8)?.max(0) as u64,
        errors: row.get::<_, i64>(9)?.max(0) as u64,
        skipped: row.get::<_, i64>(10)?.max(0) as u64,
        started_at: row.get(11)?,
        finished_at: row.get(12)?,
    })
}

impl Index {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA temp_store=FILE; PRAGMA cache_size=-8192;
            CREATE TABLE IF NOT EXISTS scans (
                id INTEGER PRIMARY KEY, root TEXT NOT NULL, raw_root BLOB NOT NULL, root_id INTEGER NOT NULL DEFAULT 0,
                status TEXT NOT NULL, entries INTEGER NOT NULL DEFAULT 0, logical_bytes INTEGER NOT NULL DEFAULT 0,
                allocated_bytes INTEGER NOT NULL DEFAULT 0, allocated_estimated INTEGER NOT NULL DEFAULT 0,
                candidate_bytes INTEGER NOT NULL DEFAULT 0, errors INTEGER NOT NULL DEFAULT 0, skipped INTEGER NOT NULL DEFAULT 0,
                started_at INTEGER NOT NULL, finished_at INTEGER);
            CREATE TABLE IF NOT EXISTS entries (
                id INTEGER PRIMARY KEY, scan_id INTEGER NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
                parent_id INTEGER REFERENCES entries(id) ON DELETE CASCADE, depth INTEGER NOT NULL, raw_path BLOB NOT NULL,
                name TEXT NOT NULL, relative_path TEXT NOT NULL, kind TEXT NOT NULL, len INTEGER NOT NULL,
                modified_ns INTEGER NOT NULL, changed_ns INTEGER NOT NULL, device INTEGER NOT NULL, inode INTEGER NOT NULL,
                own_logical INTEGER NOT NULL DEFAULT 0, own_allocated INTEGER NOT NULL DEFAULT 0,
                logical_bytes INTEGER NOT NULL DEFAULT 0, allocated_bytes INTEGER NOT NULL DEFAULT 0,
                modified_at INTEGER, accessed_at INTEGER, complete INTEGER NOT NULL DEFAULT 0, inspected INTEGER NOT NULL DEFAULT 0, error TEXT,
                protected INTEGER NOT NULL DEFAULT 0, category TEXT NOT NULL DEFAULT 'storage',
                disposition TEXT NOT NULL DEFAULT 'keep', reason TEXT NOT NULL DEFAULT 'No rebuildable storage evidence was found.',
                consequence TEXT NOT NULL DEFAULT 'Keep this item unless you independently verify its purpose.',
                uncertainty TEXT NOT NULL DEFAULT 'Usage is unknown. Allocation may be shared by clones or snapshots.',
                project TEXT, evidence TEXT NOT NULL DEFAULT '[]', removable INTEGER NOT NULL DEFAULT 0);
            CREATE INDEX IF NOT EXISTS entry_parent ON entries(parent_id);
            CREATE INDEX IF NOT EXISTS entry_candidates ON entries(scan_id,disposition);
            CREATE INDEX IF NOT EXISTS entry_native_path ON entries(scan_id,raw_path);
            CREATE INDEX IF NOT EXISTS entry_children ON entries(scan_id,parent_id);
            CREATE INDEX IF NOT EXISTS entry_depth ON entries(scan_id,depth);
            CREATE INDEX IF NOT EXISTS entry_identity ON entries(scan_id,device,inode);
            CREATE INDEX IF NOT EXISTS entry_sizes ON entries(scan_id,kind,len);
            CREATE TABLE IF NOT EXISTS pending_dirs (entry_id INTEGER PRIMARY KEY REFERENCES entries(id) ON DELETE CASCADE);
            CREATE TABLE IF NOT EXISTS duplicate_members (scan_id INTEGER NOT NULL REFERENCES scans(id) ON DELETE CASCADE, entry_id INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE, hash TEXT NOT NULL, PRIMARY KEY(scan_id,entry_id));
            CREATE INDEX IF NOT EXISTS duplicate_groups ON duplicate_members(scan_id,hash);
            CREATE TABLE IF NOT EXISTS physical_seen (scan_id INTEGER NOT NULL REFERENCES scans(id) ON DELETE CASCADE, device INTEGER NOT NULL, inode INTEGER NOT NULL, PRIMARY KEY(scan_id,device,inode));").map_err(|e| e.to_string())?;
        let inspected_column = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('entries') WHERE name='inspected'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|e| e.to_string())?;
        if inspected_column == 0 {
            conn.execute_batch("ALTER TABLE entries ADD COLUMN inspected INTEGER NOT NULL DEFAULT 0; UPDATE entries SET inspected=complete WHERE kind='directory';").map_err(|e|e.to_string())?;
        }
        Ok(Self { conn })
    }

    pub fn begin_scan(&self, root: &Path) -> Result<i64, String> {
        let metadata = std::fs::symlink_metadata(root).map_err(|e| e.to_string())?;
        if !metadata.is_dir() || scanner::is_reparse(&metadata) || metadata.file_type().is_symlink()
        {
            return Err(
                "Choose an ordinary directory; symbolic links and reparse points are not followed."
                    .into(),
            );
        }
        let selected_identity = scanner::path_fingerprint(root, &metadata)?;
        let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        let metadata = crate::native::open_directory(&root)?
            .metadata()
            .map_err(|e| e.to_string())?;
        #[cfg(windows)]
        let metadata = {
            let _guards = crate::native::locked_path(&root)?;
            std::fs::symlink_metadata(&root).map_err(|e| e.to_string())?
        };
        let fp = scanner::path_fingerprint(&root, &metadata)?;
        if fp.kind != "directory"
            || fp.device != selected_identity.device
            || fp.inode != selected_identity.inode
        {
            return Err(
                "The selected directory changed while it was being opened. Choose it again.".into(),
            );
        }
        let (allocated, estimated) = scanner::allocated(&root, &metadata);
        self.conn
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| e.to_string())?;
        let result = (|| {
            self.conn.execute("INSERT INTO scans(root,raw_root,status,started_at,allocated_estimated) VALUES(?1,?2,'queued',?3,?4)",
                params![root.to_string_lossy(), encode_path(&root), now(), estimated]).map_err(|e|e.to_string())?;
            let scan_id = self.conn.last_insert_rowid();
            self.conn.execute("INSERT INTO entries(scan_id,depth,raw_path,name,relative_path,kind,len,modified_ns,changed_ns,device,inode,own_allocated,allocated_bytes,modified_at,accessed_at) VALUES(?1,0,?2,?3,'','directory',?4,?5,?6,?7,?8,?9,?9,?10,?11)",
                params![scan_id, encode_path(&root), root.file_name().unwrap_or(root.as_os_str()).to_string_lossy(), fp.len as i64, fp.modified_ns, fp.changed_ns, fp.device as i64, fp.inode as i64, allocated as i64, scanner::seconds(metadata.modified().ok()), scanner::seconds(metadata.accessed().ok())]).map_err(|e|e.to_string())?;
            let root_id = self.conn.last_insert_rowid();
            self.conn
                .execute(
                    "UPDATE scans SET root_id=?1,entries=1,allocated_bytes=?2 WHERE id=?3",
                    params![root_id, allocated as i64, scan_id],
                )
                .map_err(|e| e.to_string())?;
            self.conn
                .execute("INSERT INTO pending_dirs(entry_id) VALUES(?1)", [root_id])
                .map_err(|e| e.to_string())?;
            self.conn
                .execute(
                    "INSERT OR IGNORE INTO physical_seen VALUES(?1,?2,?3)",
                    params![scan_id, fp.device as i64, fp.inode as i64],
                )
                .map_err(|e| e.to_string())?;
            self.conn.execute("DELETE FROM scans WHERE id IN (SELECT id FROM scans WHERE status NOT IN ('queued','scanning') ORDER BY id DESC LIMIT -1 OFFSET 2)", []).map_err(|e| e.to_string())?;
            Ok(scan_id)
        })();
        self.conn
            .execute_batch(if result.is_ok() { "COMMIT" } else { "ROLLBACK" })
            .map_err(|e| e.to_string())?;
        result
    }

    pub fn root_path(&self, scan_id: i64) -> Result<PathBuf, String> {
        self.conn
            .query_row("SELECT raw_root FROM scans WHERE id=?1", [scan_id], |row| {
                row.get::<_, Vec<u8>>(0)
            })
            .map(decode_path)
            .map_err(|e| e.to_string())
    }

    pub fn get_scan(&self, scan_id: i64) -> Result<ScanSummary, String> {
        self.conn.query_row("SELECT id,root,root_id,status,entries,logical_bytes,allocated_bytes,allocated_estimated,candidate_bytes,errors,skipped,started_at,finished_at FROM scans WHERE id=?1", [scan_id], scan_from_row).map_err(|e| e.to_string())
    }

    pub fn list_scans(&self) -> Result<Vec<ScanSummary>, String> {
        let mut query = self.conn.prepare("SELECT id,root,root_id,status,entries,logical_bytes,allocated_bytes,allocated_estimated,candidate_bytes,errors,skipped,started_at,finished_at FROM scans ORDER BY id DESC").map_err(|e| e.to_string())?;
        let rows = query
            .query_map([], scan_from_row)
            .map_err(|e| e.to_string())?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())
    }

    pub fn remove_scan(&self, scan_id: i64) -> Result<(), String> {
        if matches!(
            self.get_scan(scan_id)?.status.as_str(),
            "queued" | "scanning"
        ) {
            return Err("Cancel the active scan before removing it.".into());
        }
        self.conn
            .execute("DELETE FROM scans WHERE id=?1", [scan_id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn get_entry(&self, scan_id: i64, entry_id: i64) -> Result<EntryRow, String> {
        self.conn
            .query_row(
                &format!("SELECT {ENTRY_COLUMNS} FROM entries WHERE scan_id=?1 AND id=?2"),
                params![scan_id, entry_id],
                entry_from_row,
            )
            .map_err(|e| e.to_string())
    }

    pub fn record_by_path(&self, scan_id: i64, path: &Path) -> Result<IndexedEntry, String> {
        let lookup = |path: &Path| {
            self.conn
                .query_row(
                    "SELECT id FROM entries WHERE scan_id=?1 AND raw_path=?2",
                    params![scan_id, encode_path(path)],
                    |r| r.get::<_, i64>(0),
                )
                .optional()
                .map_err(|e| e.to_string())
        };
        let id = match lookup(path)? {
            Some(id) => id,
            None => {
                let canonical = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
                lookup(&canonical)?.ok_or("Path was not indexed in this scan")?
            }
        };
        self.get_record(scan_id, id)
    }

    pub fn get_record(&self, scan_id: i64, entry_id: i64) -> Result<IndexedEntry, String> {
        let row = self.get_entry(scan_id, entry_id)?;
        self.conn.query_row("SELECT raw_path,len,modified_ns,changed_ns,device,inode,protected FROM entries WHERE scan_id=?1 AND id=?2",params![scan_id,entry_id], |r| Ok(IndexedEntry {
            fingerprint: Fingerprint { kind: row.kind.clone(), len: r.get::<_,i64>(1)? as u64, modified_ns:r.get(2)?, changed_ns:r.get(3)?, device:r.get::<_,i64>(4)? as u64, inode:r.get::<_,i64>(5)? as u64 },
            row:row.clone(), path:decode_path(r.get(0)?), protected:r.get(6)?,
        })).map_err(|e|e.to_string())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn get_entries(
        &self,
        scan_id: i64,
        parent_id: Option<i64>,
        offset: u64,
        limit: u64,
        sort: &str,
        search: &str,
        candidates_only: bool,
    ) -> Result<EntryPage, String> {
        let order = match sort {
            "name" => "name COLLATE NOCASE,id",
            "modified" => "modified_at DESC,id",
            _ => "logical_bytes DESC,id",
        };
        let filter = if candidates_only {
            "scan_id=?1 AND disposition IN ('candidate','review') AND (?4='' OR instr(lower(relative_path),lower(?4))>0)"
        } else {
            "scan_id=?1 AND parent_id IS ?3 AND (?4='' OR instr(lower(relative_path),lower(?4))>0)"
        };
        let (total, total_bytes) = self
            .conn
            .query_row(
                &format!(
                    "SELECT count(*),coalesce(sum(logical_bytes),0) FROM entries WHERE {filter}"
                ),
                params![scan_id, candidates_only, parent_id, search],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
            )
            .map_err(|e| e.to_string())?;
        let mut query = self.conn.prepare(&format!("SELECT {ENTRY_COLUMNS} FROM entries WHERE {filter} ORDER BY {order} LIMIT ?5 OFFSET ?6")).map_err(|e|e.to_string())?;
        let items = query
            .query_map(
                params![
                    scan_id,
                    candidates_only,
                    parent_id,
                    search,
                    limit.clamp(1, 500) as i64,
                    offset.min(i64::MAX as u64) as i64
                ],
                entry_from_row,
            )
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?;
        Ok(EntryPage {
            items,
            total: total as u64,
            total_bytes: total_bytes.max(0) as u64,
            offset,
        })
    }

    pub fn candidates(&self, scan_id: i64, limit: u64) -> Result<Vec<EntryRow>, String> {
        Ok(self
            .get_entries(scan_id, None, 0, limit, "size", "", true)?
            .items)
    }

    pub fn breadcrumbs(&self, scan_id: i64, entry_id: i64) -> Result<Vec<EntryRow>, String> {
        let mut result = Vec::new();
        let mut next = Some(entry_id);
        while let Some(id) = next {
            if result.len() > 1024 {
                return Err("Directory nesting exceeds the display limit.".into());
            }
            let row = self.get_entry(scan_id, id)?;
            next = row.parent_id;
            result.push(row);
        }
        result.reverse();
        Ok(result)
    }

    pub(crate) fn optional_child(
        &self,
        scan_id: i64,
        parent_id: i64,
        name: &str,
    ) -> Result<Option<i64>, String> {
        self.conn
            .query_row(
                "SELECT id FROM entries WHERE scan_id=?1 AND parent_id=?2 AND name=?3 LIMIT 1",
                params![scan_id, parent_id, name],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())
    }
}
