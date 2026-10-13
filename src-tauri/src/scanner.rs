use crate::index::{decode_path, encode_path, now, Index};
use crate::model::ScanSummary;
use crate::rules;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::fs::{self, Metadata};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub kind: String,
    pub len: u64,
    pub modified_ns: i64,
    pub changed_ns: i64,
    pub device: u64,
    pub inode: u64,
}

pub(crate) fn seconds(value: Option<SystemTime>) -> Option<i64> {
    value
        .and_then(|v| v.duration_since(UNIX_EPOCH).ok())
        .map(|v| v.as_secs().min(i64::MAX as u64) as i64)
}

fn nanos(value: Option<SystemTime>) -> i64 {
    value
        .and_then(|v| v.duration_since(UNIX_EPOCH).ok())
        .map(|v| v.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

pub fn is_reparse(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        let _ = metadata;
        false
    }
}

pub fn fingerprint(metadata: &Metadata) -> Fingerprint {
    let kind = if metadata.file_type().is_symlink() || is_reparse(metadata) {
        "link"
    } else if metadata.is_dir() {
        "directory"
    } else if metadata.is_file() {
        "file"
    } else {
        "special"
    };
    #[cfg(unix)]
    let (device, inode, changed_ns) = {
        use std::os::unix::fs::MetadataExt;
        (
            metadata.dev(),
            metadata.ino(),
            metadata
                .ctime()
                .saturating_mul(1_000_000_000)
                .saturating_add(metadata.ctime_nsec()),
        )
    };
    #[cfg(windows)]
    let (device, inode, changed_ns) = (0, 0, 0);
    Fingerprint {
        kind: kind.into(),
        len: metadata.len(),
        modified_ns: nanos(metadata.modified().ok()),
        changed_ns,
        device,
        inode,
    }
}

pub fn path_fingerprint(path: &Path, metadata: &Metadata) -> Result<Fingerprint, String> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            FileBasicInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
            BY_HANDLE_FILE_INFORMATION, FILE_BASIC_INFO,
        };
        let file = native_open(path)?;
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let current_metadata = file.metadata().map_err(|e| e.to_string())?;
        if fingerprint(&current_metadata) != fingerprint(metadata) {
            return Err("Filesystem metadata changed while its identity was read.".into());
        }
        let mut result = fingerprint(&current_metadata);
        result.device = info.dwVolumeSerialNumber as u64;
        result.inode = ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64;
        if result.inode == 0 {
            return Err("This filesystem did not provide a stable file identity; the item cannot be inspected safely.".into());
        }
        let mut basic: FILE_BASIC_INFO = unsafe { std::mem::zeroed() };
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileBasicInfo,
                (&mut basic as *mut FILE_BASIC_INFO).cast(),
                std::mem::size_of::<FILE_BASIC_INFO>() as u32,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        result.changed_ns = basic
            .ChangeTime
            .saturating_sub(116_444_736_000_000_000)
            .saturating_mul(100);
        Ok(result)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Ok(fingerprint(metadata))
    }
}

#[cfg(windows)]
fn native_open(path: &Path) -> Result<std::fs::File, String> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .map_err(|e| e.to_string())
}

pub fn allocated(path: &Path, metadata: &Metadata) -> (u64, bool) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = path;
        (metadata.blocks().saturating_mul(512), false)
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            FileStandardInfo, GetFileInformationByHandleEx, FILE_STANDARD_INFO,
        };
        if let Ok(file) = native_open(path) {
            let mut info: FILE_STANDARD_INFO = unsafe { std::mem::zeroed() };
            if unsafe {
                GetFileInformationByHandleEx(
                    file.as_raw_handle(),
                    FileStandardInfo,
                    (&mut info as *mut FILE_STANDARD_INFO).cast(),
                    std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
                )
            } != 0
            {
                // shortcut: primary stream allocation only, enumerate all streams before claiming exact Windows totals.
                return (info.AllocationSize.max(0) as u64, true);
            }
        }
        (
            if metadata.is_file() {
                metadata.len()
            } else {
                0
            },
            true,
        )
    }
}

fn visit_directory(
    path: &Path,
    expected: &Fingerprint,
    mut visit: impl FnMut(std::ffi::OsString, Result<Metadata, String>) -> Result<(), String>,
) -> Result<(), String> {
    #[cfg(unix)]
    {
        let directory = crate::native::open_directory(path)?;
        if fingerprint(&directory.metadata().map_err(|e| e.to_string())?) != *expected {
            return Err("Directory changed before its contents were scanned.".into());
        }
        crate::native::for_each_name(&directory, |name| {
            let metadata = crate::native::open_child(&directory, Path::new(&name))
                .and_then(|file| file.metadata().map_err(|e| e.to_string()));
            let metadata = metadata.or_else(|error| {
                let link = fs::symlink_metadata(path.join(&name));
                match link {
                    Ok(value) if value.file_type().is_symlink() => Ok(value),
                    _ => Err(error),
                }
            });
            visit(name, metadata)
        })?;
        if fingerprint(&directory.metadata().map_err(|e| e.to_string())?) != *expected {
            return Err("Directory changed while its contents were scanned.".into());
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        let _ancestors = crate::native::locked_path(path)?;
        if path_fingerprint(
            path,
            &fs::symlink_metadata(path).map_err(|e| e.to_string())?,
        )? != *expected
        {
            return Err("Directory changed before its contents were scanned.".into());
        }
        for child in fs::read_dir(path).map_err(|e| e.to_string())? {
            let child = child.map_err(|e| e.to_string())?;
            let name = child.file_name();
            visit(
                name,
                fs::symlink_metadata(child.path()).map_err(|e| e.to_string()),
            )?;
        }
        if path_fingerprint(
            path,
            &fs::symlink_metadata(path).map_err(|e| e.to_string())?,
        )? != *expected
        {
            return Err("Directory changed while its contents were scanned.".into());
        }
        Ok(())
    }
}

fn bounded(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

pub fn scan(
    db_path: &Path,
    scan_id: i64,
    cancel: Arc<AtomicBool>,
    mut progress: impl FnMut(ScanSummary),
) -> Result<ScanSummary, String> {
    let index = Index::open(db_path)?;
    let mut summary = index.get_scan(scan_id)?;
    if summary.status != "queued" {
        return Err("This scan has already started.".into());
    }
    let root = index.root_path(scan_id)?;
    let root_record = index.get_record(scan_id, summary.root_id)?;
    summary.status = "scanning".into();
    index
        .conn
        .execute("UPDATE scans SET status='scanning' WHERE id=?1", [scan_id])
        .map_err(|e| e.to_string())?;
    progress(summary.clone());
    let mut last_progress = Instant::now();
    index
        .conn
        .execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| e.to_string())?;
    let result = (|| {
        let mut batch = 0;
        loop {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let pending = index.conn.query_row("SELECT e.id,e.raw_path,e.depth,e.protected FROM pending_dirs p JOIN entries e ON e.id=p.entry_id WHERE e.scan_id=?1 ORDER BY p.entry_id LIMIT 1",[scan_id],|r| Ok((r.get::<_,i64>(0)?,decode_path(r.get(1)?),r.get::<_,i64>(2)?,r.get::<_,bool>(3)?))).optional().map_err(|e|e.to_string())?;
            let Some((parent_id, path, depth, parent_protected)) = pending else {
                break;
            };
            let mut directory_ok = true;
            let mut logical_delta = 0u64;
            let mut allocated_delta = 0u64;
            match fs::symlink_metadata(&path) {
                Ok(current)
                    if !current.file_type().is_symlink()
                        && !is_reparse(&current)
                        && current.is_dir() =>
                {
                    let current_fp = path_fingerprint(&path, &current)?;
                    let old = index.get_record(scan_id, parent_id)?.fingerprint;
                    if current_fp.device != old.device || current_fp.inode != old.inode {
                        directory_ok = false;
                        summary.errors += 1;
                        index.conn.execute("UPDATE entries SET error='Directory identity changed during scan.' WHERE id=?1",[parent_id]).map_err(|e|e.to_string())?;
                    }
                }
                _ => {
                    directory_ok = false;
                    summary.errors += 1;
                    index.conn.execute("UPDATE entries SET error='Directory disappeared or became a link during scan.' WHERE id=?1",[parent_id]).map_err(|e|e.to_string())?;
                }
            }
            if directory_ok {
                let expected = index.get_record(scan_id, parent_id)?.fingerprint;
                let visited = visit_directory(&path, &expected, |native_name, metadata| {
                    if cancel.load(Ordering::Relaxed) {
                        return Err("Scan cancelled".into());
                    }
                    let path = path.join(&native_name);
                    let name = native_name.to_string_lossy().into_owned();
                    let relative = path
                        .strip_prefix(&root)
                        .map_err(|_| "A directory entry escaped the selected root.")?
                        .to_string_lossy()
                        .into_owned();
                    let mut error = None;
                    let mut complete = true;
                    let mut logical = 0;
                    let mut physical = 0;
                    let mut fp = Fingerprint {
                        kind: "unknown".into(),
                        len: 0,
                        modified_ns: 0,
                        changed_ns: 0,
                        device: 0,
                        inode: 0,
                    };
                    let mut modified = None;
                    let mut accessed = None;
                    if let Ok(ref meta) = metadata {
                        fp = fingerprint(meta);
                        match path_fingerprint(&path, meta) {
                            Ok(value) => fp = value,
                            Err(value) => {
                                error = Some(value);
                                complete = false;
                                summary.errors += 1;
                            }
                        }
                        modified = seconds(meta.modified().ok());
                        accessed = seconds(meta.accessed().ok());
                        if fp.kind == "link" || fp.kind == "special" {
                            complete = false;
                            error=Some("Not followed: symbolic link, reparse point, or special filesystem object.".into());
                            summary.skipped += 1;
                        } else if error.is_none() && fp.device != root_record.fingerprint.device {
                            complete = false;
                            error = Some(
                                "Not followed: this item is on another mounted filesystem.".into(),
                            );
                            summary.skipped += 1;
                        } else if error.is_none() && depth >= 256 {
                            complete = false;
                            error = Some("Directory nesting exceeds the scan limit.".into());
                            summary.skipped += 1;
                        } else if error.is_none() {
                            logical = if meta.is_file() { meta.len() } else { 0 };
                            let (bytes, estimated) = allocated(&path, meta);
                            summary.allocated_estimated |= estimated;
                            let first = index
                                .conn
                                .execute(
                                    "INSERT OR IGNORE INTO physical_seen VALUES(?1,?2,?3)",
                                    params![scan_id, fp.device as i64, fp.inode as i64],
                                )
                                .map_err(|e| e.to_string())?
                                > 0;
                            physical = if first { bytes } else { 0 };
                            if fp.kind == "directory" {
                                complete = false;
                            }
                        }
                    } else if let Err(value) = metadata {
                        error = Some(value.to_string());
                        complete = false;
                        summary.errors += 1;
                    }
                    let protected = parent_protected || rules::protected(&path);
                    index.conn.execute("INSERT INTO entries(scan_id,parent_id,depth,raw_path,name,relative_path,kind,len,modified_ns,changed_ns,device,inode,own_logical,own_allocated,logical_bytes,allocated_bytes,modified_at,accessed_at,complete,error,protected) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?13,?14,?15,?16,?17,?18,?19)",params![scan_id,parent_id,depth+1,encode_path(&path),name,relative,fp.kind,bounded(fp.len),fp.modified_ns,fp.changed_ns,fp.device as i64,fp.inode as i64,bounded(logical),bounded(physical),modified,accessed,complete,error,protected]).map_err(|e|e.to_string())?;
                    let entry_id = index.conn.last_insert_rowid();
                    if fp.kind == "directory" && error.is_none() {
                        index
                            .conn
                            .execute("INSERT INTO pending_dirs(entry_id) VALUES(?1)", [entry_id])
                            .map_err(|e| e.to_string())?;
                    }
                    logical_delta = logical_delta.saturating_add(logical);
                    allocated_delta = allocated_delta.saturating_add(physical);
                    summary.entries += 1;
                    summary.logical_bytes = summary.logical_bytes.saturating_add(logical);
                    summary.allocated_bytes = summary.allocated_bytes.saturating_add(physical);
                    batch += 1;
                    if batch >= 256 || last_progress.elapsed() >= Duration::from_millis(250) {
                        add_directory_bytes(&index, parent_id, logical_delta, allocated_delta)?;
                        logical_delta = 0;
                        allocated_delta = 0;
                        save_summary(&index, &summary)?;
                        index
                            .conn
                            .execute_batch("COMMIT; BEGIN IMMEDIATE")
                            .map_err(|e| e.to_string())?;
                        batch = 0;
                        if last_progress.elapsed() >= Duration::from_millis(250) {
                            progress(summary.clone());
                            last_progress = Instant::now();
                        }
                    }
                    Ok(())
                });
                if let Err(error) = visited {
                    directory_ok = false;
                    if !cancel.load(Ordering::Relaxed) {
                        summary.errors += 1;
                    }
                    index
                        .conn
                        .execute(
                            "UPDATE entries SET error=?1 WHERE id=?2",
                            params![error, parent_id],
                        )
                        .map_err(|e| e.to_string())?;
                }
            }
            add_directory_bytes(&index, parent_id, logical_delta, allocated_delta)?;
            index
                .conn
                .execute(
                    "UPDATE entries SET inspected=?1 WHERE id=?2",
                    params![directory_ok, parent_id],
                )
                .map_err(|e| e.to_string())?;
            update_directory_coverage(&index, parent_id)?;
            index
                .conn
                .execute("DELETE FROM pending_dirs WHERE entry_id=?1", [parent_id])
                .map_err(|e| e.to_string())?;
        }
        save_summary(&index, &summary)?;
        Ok::<_, String>(())
    })();
    index
        .conn
        .execute_batch(if result.is_ok() { "COMMIT" } else { "ROLLBACK" })
        .map_err(|e| e.to_string())?;
    if let Err(error) = result {
        index
            .conn
            .execute(
                "UPDATE scans SET status='failed',finished_at=?1 WHERE id=?2",
                params![now(), scan_id],
            )
            .map_err(|e| e.to_string())?;
        return Err(error);
    }
    aggregate(
        &index,
        scan_id,
        &cancel,
        &mut progress,
        &mut summary,
        &mut last_progress,
    )?;
    summary.status = if cancel.load(Ordering::Relaxed) {
        "cancelled"
    } else if summary.errors > 0 || summary.skipped > 0 {
        "partial"
    } else {
        "complete"
    }
    .into();
    summary.finished_at = Some(now());
    rules::classify(&index, scan_id, &cancel)?;
    summary.candidate_bytes=index.conn.query_row("SELECT coalesce(sum(logical_bytes),0) FROM entries WHERE scan_id=?1 AND disposition='candidate' AND removable=1",[scan_id],|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())?.max(0) as u64;
    save_summary(&index, &summary)?;
    index
        .conn
        .execute(
            "DELETE FROM pending_dirs WHERE entry_id IN (SELECT id FROM entries WHERE scan_id=?1)",
            [scan_id],
        )
        .map_err(|e| e.to_string())?;
    loop {
        let removed=index.conn.execute("DELETE FROM physical_seen WHERE rowid IN (SELECT rowid FROM physical_seen WHERE scan_id=?1 LIMIT 4096)",[scan_id]).map_err(|e|e.to_string())?;
        if removed == 0 {
            break;
        }
        if last_progress.elapsed() >= Duration::from_millis(250) {
            progress(summary.clone());
            last_progress = Instant::now();
        }
    }
    progress(summary.clone());
    Ok(summary)
}

fn add_directory_bytes(
    index: &Index,
    entry_id: i64,
    logical: u64,
    allocated: u64,
) -> Result<(), String> {
    if logical == 0 && allocated == 0 {
        return Ok(());
    }
    let mut next = Some(entry_id);
    while let Some(id) = next {
        next = index
            .conn
            .query_row("SELECT parent_id FROM entries WHERE id=?1", [id], |row| {
                row.get::<_, Option<i64>>(0)
            })
            .map_err(|e| e.to_string())?;
        index.conn.execute("UPDATE entries SET logical_bytes=logical_bytes+?1,allocated_bytes=allocated_bytes+?2 WHERE id=?3",params![bounded(logical),bounded(allocated),id]).map_err(|e|e.to_string())?;
    }
    Ok(())
}

fn update_directory_coverage(index: &Index, entry_id: i64) -> Result<(), String> {
    let mut next = Some(entry_id);
    while let Some(id) = next {
        next = index
            .conn
            .query_row("SELECT parent_id FROM entries WHERE id=?1", [id], |row| {
                row.get::<_, Option<i64>>(0)
            })
            .map_err(|e| e.to_string())?;
        index.conn.execute("UPDATE entries AS p SET complete=inspected AND NOT EXISTS(SELECT 1 FROM entries c WHERE c.parent_id=p.id AND c.complete=0) WHERE id=?1",[id]).map_err(|e|e.to_string())?;
    }
    Ok(())
}

fn save_summary(index: &Index, summary: &ScanSummary) -> Result<(), String> {
    index.conn.execute("UPDATE scans SET status=?1,entries=?2,logical_bytes=?3,allocated_bytes=?4,allocated_estimated=?5,candidate_bytes=?6,errors=?7,skipped=?8,finished_at=?9 WHERE id=?10",params![summary.status,bounded(summary.entries),bounded(summary.logical_bytes),bounded(summary.allocated_bytes),summary.allocated_estimated,bounded(summary.candidate_bytes),bounded(summary.errors),bounded(summary.skipped),summary.finished_at,summary.id]).map_err(|e|e.to_string())?;
    Ok(())
}

fn aggregate(
    index: &Index,
    scan_id: i64,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(ScanSummary),
    summary: &mut ScanSummary,
    last: &mut Instant,
) -> Result<(), String> {
    let max_depth = index
        .conn
        .query_row(
            "SELECT coalesce(max(depth),0) FROM entries WHERE scan_id=?1",
            [scan_id],
            |r| r.get::<_, i64>(0),
        )
        .map_err(|e| e.to_string())?;
    // Aggregate one depth at a time; SQLite holds the tree, including cancelled scans.
    for depth in (0..=max_depth).rev() {
        index.conn.execute("UPDATE entries AS p SET logical_bytes=own_logical+coalesce((SELECT sum(c.logical_bytes) FROM entries c WHERE c.parent_id=p.id),0),allocated_bytes=own_allocated+coalesce((SELECT sum(c.allocated_bytes) FROM entries c WHERE c.parent_id=p.id),0),complete=inspected AND NOT EXISTS(SELECT 1 FROM entries c WHERE c.parent_id=p.id AND c.complete=0),protected=protected OR EXISTS(SELECT 1 FROM entries c WHERE c.parent_id=p.id AND c.protected=1) WHERE scan_id=?1 AND depth=?2 AND kind='directory'",params![scan_id,depth]).map_err(|e|e.to_string())?;
        if last.elapsed() >= Duration::from_millis(250) {
            progress(summary.clone());
            *last = Instant::now();
        }
    }
    if cancel.load(Ordering::Relaxed) {
        index
            .conn
            .execute(
                "UPDATE entries SET complete=0 WHERE id=?1",
                [summary.root_id],
            )
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    pub(crate) fn fixture() -> (TempDir, PathBuf, PathBuf) {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("root");
        fs::create_dir(&root).unwrap();
        let db = temp.path().join("index.sqlite");
        (temp, root, db)
    }

    pub(crate) fn run(root: &Path, db: &Path) -> ScanSummary {
        let index = Index::open(db).unwrap();
        let id = index.begin_scan(root).unwrap();
        scan(db, id, Arc::new(AtomicBool::new(false)), |_| {}).unwrap()
    }

    #[test]
    fn paginated_tree_and_aggregates_are_real() {
        let (_temp, root, db) = fixture();
        fs::create_dir(root.join("nested")).unwrap();
        fs::write(root.join("nested/a"), [1u8; 120]).unwrap();
        fs::write(root.join("b"), [2u8; 80]).unwrap();
        let summary = run(&root, &db);
        assert_eq!(summary.logical_bytes, 200);
        assert_eq!(summary.entries, 4);
        assert_eq!(summary.status, "complete");
        let index = Index::open(&db).unwrap();
        let page = index
            .get_entries(summary.id, Some(summary.root_id), 0, 1, "size", "", false)
            .unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(page.total_bytes, 200);
        assert_eq!(page.items[0].name, "nested");
        assert_eq!(page.items[0].logical_bytes, 120);
        let second = index
            .get_entries(summary.id, Some(summary.root_id), 1, 1, "size", "", false)
            .unwrap();
        assert_eq!(second.items[0].name, "b");
    }

    #[cfg(unix)]
    #[test]
    fn hardlinks_count_physical_once_and_links_are_not_followed() {
        use std::os::unix::fs::{symlink, MetadataExt};
        let (_temp, root, db) = fixture();
        fs::write(root.join("original"), [7u8; 8192]).unwrap();
        fs::hard_link(root.join("original"), root.join("alias")).unwrap();
        symlink("/", root.join("escape")).unwrap();
        let summary = run(&root, &db);
        assert_eq!(summary.logical_bytes, 16384);
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.status, "partial");
        let expected = fs::metadata(root.join("original")).unwrap().blocks() * 512
            + fs::metadata(&root).unwrap().blocks() * 512;
        assert_eq!(summary.allocated_bytes, expected);
        assert!(
            !Index::open(&db)
                .unwrap()
                .get_entry(summary.id, summary.root_id)
                .unwrap()
                .complete
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_hardlinks_count_primary_allocation_once() {
        let (_temp, root, db) = fixture();
        fs::write(root.join("original"), [7u8; 8192]).unwrap();
        fs::hard_link(root.join("original"), root.join("alias")).unwrap();
        let summary = run(&root, &db);
        assert_eq!(summary.logical_bytes, 16384);
        let expected = allocated(&root, &fs::metadata(&root).unwrap()).0
            + allocated(
                &root.join("original"),
                &fs::metadata(root.join("original")).unwrap(),
            )
            .0;
        assert_eq!(summary.allocated_bytes, expected);
        assert!(summary.allocated_estimated);
        assert!(
            crate::duplicates::find(&db, summary.id, Arc::new(AtomicBool::new(false)), |_| {})
                .unwrap()
                .is_empty()
        );
    }

    #[cfg(unix)]
    #[test]
    fn sparse_logical_size_does_not_become_allocated_size() {
        use std::os::unix::fs::MetadataExt;
        let (_temp, root, db) = fixture();
        let file = fs::File::create(root.join("sparse")).unwrap();
        file.set_len(8 * 1024 * 1024).unwrap();
        let metadata = file.metadata().unwrap();
        let summary = run(&root, &db);
        assert_eq!(summary.logical_bytes, 8 * 1024 * 1024);
        assert_eq!(
            summary.allocated_bytes,
            metadata.blocks() * 512 + fs::metadata(&root).unwrap().blocks() * 512
        );
        assert!(metadata.blocks() * 512 < metadata.len());
        assert!(!summary.allocated_estimated);
    }

    #[test]
    fn directory_changed_after_queueing_is_partial() {
        let (_temp, root, db) = fixture();
        let index = Index::open(&db).unwrap();
        let id = index.begin_scan(&root).unwrap();
        fs::write(root.join("arrived-after-queueing"), b"a").unwrap();
        let summary = scan(&db, id, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        assert_eq!(summary.status, "partial");
        assert!(summary.errors > 0);
        assert!(!index.get_entry(id, summary.root_id).unwrap().complete);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn native_non_utf8_paths_roundtrip() {
        use std::os::unix::ffi::OsStringExt;
        let (_temp, root, db) = fixture();
        let path = root.join(std::ffi::OsString::from_vec(vec![0xff, 0xfe]));
        fs::write(&path, b"hello").unwrap();
        let summary = run(&root, &db);
        let index = Index::open(&db).unwrap();
        let row = &index
            .get_entries(summary.id, Some(summary.root_id), 0, 10, "size", "", false)
            .unwrap()
            .items[0];
        assert_eq!(index.get_record(summary.id, row.id).unwrap().path, path);
    }

    #[cfg(unix)]
    #[test]
    fn native_path_codec_preserves_non_utf8_bytes() {
        use std::os::unix::ffi::OsStringExt;
        let path = PathBuf::from(std::ffi::OsString::from_vec(vec![b'/', 0xff, 0xfe]));
        assert_eq!(decode_path(encode_path(&path)), path);
    }

    #[test]
    fn cancellation_keeps_partial_coverage() {
        let (_temp, root, db) = fixture();
        for i in 0..1000 {
            fs::write(root.join(i.to_string()), b"a").unwrap();
        }
        let index = Index::open(&db).unwrap();
        let id = index.begin_scan(&root).unwrap();
        let summary = scan(&db, id, Arc::new(AtomicBool::new(true)), |_| {}).unwrap();
        assert_eq!(summary.status, "cancelled");
        assert_eq!(summary.entries, 1);
        assert!(!index.get_entry(id, summary.root_id).unwrap().complete);
    }

    #[test]
    fn growing_directory_totals_are_visible_before_scan_finishes() {
        let (_temp, root, db) = fixture();
        for folder in ["first", "second"] {
            fs::create_dir(root.join(folder)).unwrap();
            for i in 0..3000 {
                fs::write(root.join(folder).join(i.to_string()), [3u8; 32]).unwrap();
            }
        }
        let index = Index::open(&db).unwrap();
        let id = index.begin_scan(&root).unwrap();
        let mut observed_growing = false;
        let summary = scan(&db, id, Arc::new(AtomicBool::new(false)), |summary| {
            if summary.status == "scanning" && summary.entries > 3 && summary.entries < 6003 {
                let page = index
                    .get_entries(id, Some(summary.root_id), 0, 10, "size", "", false)
                    .unwrap();
                if page.total_bytes > 0 {
                    observed_growing = true;
                    assert_eq!(page.total_bytes, summary.logical_bytes);
                    assert!(!index.get_entry(id, summary.root_id).unwrap().complete);
                }
            }
        })
        .unwrap();
        assert!(
            observed_growing,
            "Expected a progress snapshot with real growing subtree sizes"
        );
        assert_eq!(summary.logical_bytes, 6000 * 32);
        assert_eq!(
            index.get_entry(id, summary.root_id).unwrap().logical_bytes,
            6000 * 32
        );
    }

    #[cfg(unix)]
    #[test]
    fn denied_directory_has_partial_coverage_and_cannot_be_cleaned() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let (_temp, root, db) = fixture();
        let denied = root.join("denied");
        fs::create_dir(&denied).unwrap();
        fs::write(denied.join("persistent.txt"), b"keep").unwrap();
        struct RestorePermissions {
            path: PathBuf,
            permissions: fs::Permissions,
        }
        impl Drop for RestorePermissions {
            fn drop(&mut self) {
                let _ = fs::set_permissions(&self.path, self.permissions.clone());
            }
        }
        let restore = RestorePermissions {
            path: denied.clone(),
            permissions: fs::metadata(&denied).unwrap().permissions(),
        };
        fs::set_permissions(&denied, fs::Permissions::from_mode(0o000)).unwrap();
        let index = Index::open(&db).unwrap();
        let id = index.begin_scan(&root).unwrap();
        let result = scan(&db, id, Arc::new(AtomicBool::new(false)), |_| {});
        fs::set_permissions(&denied, restore.permissions.clone()).unwrap();
        drop(restore);
        let summary = result.unwrap();
        assert_eq!(summary.status, "partial");
        assert!(summary.errors > 0);
        let entry = index.record_by_path(id, &denied).unwrap().row;
        assert!(!entry.complete);
        assert!(!entry.removable);
        assert!(entry.error.is_some());
        assert!(!index.get_entry(id, summary.root_id).unwrap().complete);
        assert_eq!(fs::read(denied.join("persistent.txt")).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn nesting_beyond_depth_limit_is_partial_and_uncleanable() {
        let (_temp, root, db) = fixture();
        let mut deepest = root.clone();
        for _ in 0..260 {
            deepest = deepest.join("d");
            fs::create_dir(&deepest).unwrap();
        }
        fs::write(deepest.join("persistent.txt"), b"keep").unwrap();
        let summary = run(&root, &db);
        let index = Index::open(&db).unwrap();
        assert_eq!(summary.status, "partial");
        assert!(summary.skipped > 0);
        let (depth, entry_id) = index
            .conn
            .query_row(
                "SELECT depth,id FROM entries WHERE scan_id=?1 ORDER BY depth DESC LIMIT 1",
                [summary.id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap();
        assert_eq!(depth, 257);
        let entry = index.get_entry(summary.id, entry_id).unwrap();
        assert!(!entry.complete);
        assert!(!entry.removable);
        assert!(entry.error.unwrap().contains("nesting"));
        assert!(
            !index
                .get_entry(summary.id, summary.root_id)
                .unwrap()
                .complete
        );
        assert!(index.candidates(summary.id, 100).unwrap().is_empty());
        assert_eq!(fs::read(deepest.join("persistent.txt")).unwrap(), b"keep");
    }

    #[test]
    fn unicode_and_long_file_names_preserve_native_identity() {
        let (_temp, root, db) = fixture();
        let name = format!("notes-{}-資料-🧼.txt", "x".repeat(210));
        let native_root = fs::canonicalize(&root).unwrap();
        fs::write(native_root.join(&name), b"hello").unwrap();
        let summary = run(&root, &db);
        let index = Index::open(&db).unwrap();
        let page = index
            .get_entries(summary.id, Some(summary.root_id), 0, 10, "name", "", false)
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].name, name);
        assert_eq!(page.items[0].relative_path, name);
        let record = index.get_record(summary.id, page.items[0].id).unwrap();
        assert_eq!(
            record.path.file_name().unwrap(),
            std::ffi::OsStr::new(&name)
        );
        assert_eq!(fs::read(record.path).unwrap(), b"hello");
        assert_eq!(summary.logical_bytes, 5);
    }

    #[test]
    fn keeps_only_three_recent_scans() {
        let (_temp, root, db) = fixture();
        for _ in 0..5 {
            run(&root, &db);
        }
        assert_eq!(Index::open(&db).unwrap().list_scans().unwrap().len(), 3);
    }
}
