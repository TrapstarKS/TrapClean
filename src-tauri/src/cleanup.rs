use crate::index::{self, Index, IndexedEntry};
use crate::model::{CleanupItemResult, CleanupPlan, CleanupResult, PlanItem};
use crate::scanner;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

pub struct PreparedPlan {
    pub visible: CleanupPlan,
    root: PathBuf,
    root_identity: scanner::Fingerprint,
    records: Vec<IndexedEntry>,
}

#[derive(Default)]
pub struct PlanStore {
    plans: HashMap<String, PreparedPlan>,
}
impl PlanStore {
    pub fn insert(&mut self, plan: PreparedPlan) {
        self.plans.clear();
        self.plans.insert(plan.visible.token.clone(), plan);
    }
    pub fn clear(&mut self) {
        self.plans.clear();
    }
    pub fn take(&mut self, token: &str) -> Result<PreparedPlan, String> {
        self.plans
            .remove(token)
            .ok_or_else(|| "Preview expired or already used. Create a new preview.".into())
    }
}

pub fn prepare(
    index: &Index,
    scan_id: i64,
    ids: &[i64],
    mode: &str,
) -> Result<PreparedPlan, String> {
    if !matches!(mode, "trash" | "permanent") {
        return Err("Unsupported cleanup action".into());
    }
    if ids.is_empty() || ids.len() > 100 {
        return Err("Select between 1 and 100 items to review.".into());
    }
    let scan = index.get_scan(scan_id)?;
    if !matches!(scan.status.as_str(), "complete" | "partial") {
        return Err("Finish a scan before preparing cleanup.".into());
    }
    let root = index.root_path(scan_id)?;
    let root_identity = index.get_record(scan_id, scan.root_id)?.fingerprint;
    crate::duplicates::verify_selection(index, scan_id, ids, &AtomicBool::new(false))?;
    let root_metadata = std::fs::symlink_metadata(&root).map_err(|e| e.to_string())?;
    let actual = scanner::path_fingerprint(&root, &root_metadata)?;
    if actual.kind != "directory"
        || actual.device != root_identity.device
        || actual.inode != root_identity.inode
    {
        return Err("The selected root changed identity; refused.".into());
    }
    let mut records: Vec<IndexedEntry> = Vec::new();
    for id in ids {
        if records.iter().any(|r| r.row.id == *id) {
            continue;
        }
        let record = index.get_record(scan_id, *id)?;
        if record.path == root
            || !record.path.starts_with(&root)
            || record.protected
            || !record.row.removable
            || !record.row.complete
            || !matches!(record.row.kind.as_str(), "file" | "directory")
        {
            return Err(format!(
                "{} is protected, incompletely inspected, or outside this scan.",
                record.row.relative_path
            ));
        }
        if records
            .iter()
            .any(|r| r.path.starts_with(&record.path) || record.path.starts_with(&r.path))
        {
            return Err(
                "Select the folder or its children, without overlapping selections.".into(),
            );
        }
        validate_original(index, scan_id, &record)?;
        records.push(record);
    }
    let items = records
        .iter()
        .map(|r| PlanItem {
            id: r.row.id,
            relative_path: r.row.relative_path.clone(),
            logical_bytes: r.row.logical_bytes,
            reason: r.row.reason.clone(),
            consequence: r.row.consequence.clone(),
        })
        .collect();
    let estimated_bytes = records.iter().map(|r| r.row.logical_bytes).sum();
    let visible = CleanupPlan {
        token: uuid::Uuid::new_v4().to_string(),
        scan_id,
        mode: mode.to_string(),
        items,
        estimated_bytes,
        expires_at: index::now() + 300,
    };
    Ok(PreparedPlan {
        visible,
        root,
        root_identity,
        records,
    })
}

fn validate_original(index: &Index, scan_id: i64, record: &IndexedEntry) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(&record.path)
        .map_err(|_| "Item disappeared. Scan again.".to_string())?;
    if scanner::is_reparse(&metadata)
        || metadata.file_type().is_symlink()
        || scanner::path_fingerprint(&record.path, &metadata)? != record.fingerprint
    {
        return Err("Changed since scan. Scan this folder again.".into());
    }
    let bad: i64 = index.conn.query_row("WITH RECURSIVE subtree(id) AS (SELECT ?1 UNION ALL SELECT e.id FROM entries e JOIN subtree s ON e.parent_id=s.id WHERE e.scan_id=?2) SELECT count(*) FROM entries WHERE id IN subtree AND (protected=1 OR complete=0 OR kind NOT IN ('file','directory'))", rusqlite::params![record.row.id,scan_id], |r|r.get(0)).map_err(|e|e.to_string())?;
    if bad != 0 {
        return Err(
            "This folder contains protected, skipped, or incompletely inspected items.".into(),
        );
    }
    if matches!(
        record.row.category.as_str(),
        "cargo-build" | "javascript-dependencies"
    ) {
        let parent = record.path.parent().ok_or("Missing project root")?;
        let mut evidence = if record.row.category == "cargo-build" {
            vec![parent.join("Cargo.toml"), parent.join("src")]
        } else {
            vec![parent.join("package.json")]
        };
        let possible = if record.row.category == "cargo-build" {
            vec![parent.join("src/main.rs"), parent.join("src/lib.rs")]
        } else {
            vec![
                parent.join("package-lock.json"),
                parent.join("npm-shrinkwrap.json"),
                parent.join("pnpm-lock.yaml"),
                parent.join("yarn.lock"),
                parent.join("bun.lock"),
                parent.join("bun.lockb"),
            ]
        };
        let observed: Vec<_> = possible
            .into_iter()
            .filter(|path| index.record_by_path(scan_id, path).is_ok())
            .collect();
        if observed.is_empty() {
            return Err("Reconstruction evidence is missing. Scan again.".into());
        }
        evidence.extend(observed);
        for path in evidence {
            let observed = index.record_by_path(scan_id, &path)?;
            let metadata = std::fs::symlink_metadata(&observed.path)
                .map_err(|_| "Preserved project evidence disappeared; refused.")?;
            if scanner::path_fingerprint(&observed.path, &metadata)? != observed.fingerprint {
                return Err(
                    "Preserved sources, manifest or lockfile changed. Scan again before cleanup."
                        .into(),
                );
            }
        }
    }
    Ok(())
}

fn compare(
    index: &Index,
    scan_id: i64,
    original: &Path,
    current: &Path,
    metadata: &std::fs::Metadata,
    top: bool,
) -> Result<IndexedEntry, String> {
    let record = index.record_by_path(scan_id, original)?;
    if !record.row.complete
        || record.protected
        || scanner::is_reparse(metadata)
        || metadata.file_type().is_symlink()
    {
        return Err("Protected or uninspected contents detected; refused.".into());
    }
    let mut actual = scanner::path_fingerprint(current, metadata)?;
    let mut expected = record.fingerprint.clone();
    // Atomic capture updates only the top directory entry's change timestamp.
    if top {
        actual.changed_ns = 0;
        expected.changed_ns = 0;
    }
    if actual != expected {
        return Err("Contents changed since scan; refused.".into());
    }
    Ok(record)
}

#[cfg(unix)]
fn validate_staged(
    index: &Index,
    scan_id: i64,
    staged: &crate::native::Staged,
    original: &Path,
    cancel: &AtomicBool,
) -> Result<(), String> {
    #[allow(clippy::too_many_arguments)]
    fn visit(
        index: &Index,
        scan_id: i64,
        file: &std::fs::File,
        original: &Path,
        relative: &Path,
        current: &Path,
        depth: usize,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        if cancel.load(Ordering::Acquire) {
            return Err("Cancelled before removal".into());
        }
        if depth > 256 {
            return Err("Cleanup depth limit reached".into());
        }
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        let original_path = if relative.as_os_str().is_empty() {
            original.to_path_buf()
        } else {
            original.join(relative)
        };
        let record = compare(
            index,
            scan_id,
            &original_path,
            current,
            &metadata,
            relative.as_os_str().is_empty(),
        )?;
        if metadata.is_dir() {
            let mut count = 0u64;
            crate::native::for_each_name(file, |name| {
                let child = crate::native::open_child(file, Path::new(&name))?;
                visit(
                    index,
                    scan_id,
                    &child,
                    original,
                    &relative.join(&name),
                    &current.join(&name),
                    depth + 1,
                    cancel,
                )?;
                count += 1;
                Ok(())
            })?;
            let expected: i64 = index
                .conn
                .query_row(
                    "SELECT count(*) FROM entries WHERE scan_id=?1 AND parent_id=?2",
                    rusqlite::params![scan_id, record.row.id],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if count != expected as u64 {
                return Err("Directory contents changed since scan; refused.".into());
            }
        }
        Ok(())
    }
    visit(
        index,
        scan_id,
        &staged.handle,
        original,
        Path::new(""),
        &staged.path,
        0,
        cancel,
    )
}

#[cfg(windows)]
fn validate_staged(
    index: &Index,
    scan_id: i64,
    staged: &crate::native::Staged,
    original: &Path,
    cancel: &AtomicBool,
) -> Result<(), String> {
    staged.validate(|relative, current, metadata| {
        if cancel.load(Ordering::Acquire) {
            return Err("Cancelled before removal".into());
        }
        let top = relative.as_os_str().is_empty();
        let original_path = if top {
            original.to_path_buf()
        } else {
            original.join(relative)
        };
        let record = compare(index, scan_id, &original_path, current, metadata, top)?;
        let expected: i64 = index
            .conn
            .query_row(
                "SELECT count(*) FROM entries WHERE scan_id=?1 AND parent_id=?2",
                rusqlite::params![scan_id, record.row.id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        Ok(expected as u64)
    })
}

pub fn execute(
    index: &Index,
    plan: PreparedPlan,
    confirmation: &str,
    cancel: Arc<AtomicBool>,
) -> Result<CleanupResult, String> {
    let expected = if plan.visible.mode == "trash" {
        "MOVE TO TRASH"
    } else {
        "DELETE PERMANENTLY"
    };
    if confirmation != expected {
        return Err(format!("Type {expected} to confirm this exact preview."));
    }
    if plan.visible.expires_at < index::now() {
        return Err("Preview expired. Create a new one.".into());
    }
    crate::duplicates::verify_selection(
        index,
        plan.visible.scan_id,
        &plan.records.iter().map(|r| r.row.id).collect::<Vec<_>>(),
        &cancel,
    )?;
    index.conn.execute_batch("CREATE TABLE IF NOT EXISTS cleanup_history(id INTEGER PRIMARY KEY,recorded_at INTEGER NOT NULL,mode TEXT NOT NULL,result TEXT NOT NULL)").map_err(|e|e.to_string())?;
    let before = fs2::available_space(&plan.root).ok();
    let mut result = CleanupResult {
        items: Vec::new(),
        moved_bytes: 0,
        deleted_bytes: 0,
        free_space_change: None,
        interrupted: false,
    };
    for record in &plan.records {
        if cancel.load(Ordering::Acquire) {
            result.interrupted = true;
            result.items.push(CleanupItemResult {
                id: record.row.id,
                relative_path: record.row.relative_path.clone(),
                status: "cancelled".into(),
                message: "Cancelled before this item was changed.".into(),
                bytes: 0,
            });
            continue;
        }
        let mut deleted_bytes = 0;
        let operation = (|| {
            validate_original(index, plan.visible.scan_id, record)?;
            let mut staged = crate::native::Staged::capture_scoped(
                &record.path,
                &record.fingerprint,
                &plan.root,
                &plan.root_identity,
            )?;
            if let Err(error) =
                validate_staged(index, plan.visible.scan_id, &staged, &record.path, &cancel)
            {
                return Err(match staged.restore() {
                    Ok(()) => error,
                    Err(recovery) => format!("{error}. {recovery}"),
                });
            }
            let staged_path = staged.path.clone();
            let operation = if plan.visible.mode == "trash" {
                staged.trash()
            } else {
                staged.remove(|relative,metadata| {
                    if cancel.load(Ordering::Acquire){ return Err("Interrupted during permanent deletion; some files may already be deleted.".into()); }
                    let original=if relative.as_os_str().is_empty(){record.path.clone()}else{record.path.join(relative)};
                    let known=index.record_by_path(plan.visible.scan_id,&original)?;
                    let current_path=if relative.as_os_str().is_empty(){staged_path.clone()}else{staged_path.join(relative)};
                    let current=scanner::path_fingerprint(&current_path,metadata)?;
                    if known.protected || !known.row.complete || current.device!=known.fingerprint.device || current.inode!=known.fingerprint.inode || current.kind!=known.fingerprint.kind || (current.kind=="file" && (current.len!=known.fingerprint.len || current.modified_ns!=known.fingerprint.modified_ns)) { return Err("Changed or protected contents detected during removal.".into()); }
                    Ok(())
                })
            };
            deleted_bytes = staged.deleted_bytes;
            if let Err(error) = operation {
                return Err(match staged.restore() {
                    Ok(()) => error,
                    Err(recovery) => format!("{error}. {recovery}"),
                });
            }
            Ok(())
        })();
        if operation.is_err() && cancel.load(Ordering::Acquire) {
            result.interrupted = true;
        }
        result.deleted_bytes = result.deleted_bytes.saturating_add(deleted_bytes);
        let (status, message, bytes) = match operation {
            Ok(()) => {
                if plan.visible.mode == "trash" {
                    result.moved_bytes += record.row.logical_bytes;
                }
                (
                    "success",
                    if plan.visible.mode == "trash" {
                        "Moved to system Trash/Recycle Bin. Restore with the system file manager; the original folder is shown here. Space is normally released only after emptying these items.".to_string()
                    } else {
                        "Permanently deleted. No recovery is provided by TrapClean.".to_string()
                    },
                    if plan.visible.mode == "trash" {
                        record.row.logical_bytes
                    } else {
                        deleted_bytes
                    },
                )
            }
            Err(message) => (
                "error",
                if deleted_bytes > 0 {
                    format!("{message} {deleted_bytes} logical bytes were permanently deleted before the interruption; remaining items were retained or restored.")
                } else {
                    message
                },
                deleted_bytes,
            ),
        };
        result.items.push(CleanupItemResult {
            id: record.row.id,
            relative_path: record.row.relative_path.clone(),
            status: status.into(),
            message,
            bytes,
        });
    }
    let after = fs2::available_space(&plan.root).ok();
    result.free_space_change = before
        .zip(after)
        .map(|(a, b)| (b as i128 - a as i128).clamp(i64::MIN as i128, i64::MAX as i128) as i64);
    // The old snapshot is historical. A new scan is required after every attempted cleanup.
    index
        .conn
        .execute(
            "UPDATE scans SET status='stale' WHERE id=?1",
            [plan.visible.scan_id],
        )
        .map_err(|e| e.to_string())?;
    index
        .conn
        .execute(
            "UPDATE entries SET removable=0 WHERE scan_id=?1",
            [plan.visible.scan_id],
        )
        .map_err(|e| e.to_string())?;
    let encoded = serde_json::to_string(&result).map_err(|e| e.to_string())?;
    index
        .conn
        .execute(
            "INSERT INTO cleanup_history(recorded_at,mode,result) VALUES(?1,?2,?3)",
            rusqlite::params![index::now(), plan.visible.mode, encoded],
        )
        .map_err(|e| e.to_string())?;
    index.conn.execute("DELETE FROM cleanup_history WHERE id NOT IN (SELECT id FROM cleanup_history ORDER BY id DESC LIMIT 20)",[]).map_err(|e|e.to_string())?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Index, i64, i64) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target/debug/deps")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname='fixture'\nversion='0.1.0'\n",
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn fixture() {}\n").unwrap();
        std::fs::write(root.join("target/.rustc_info.json"), "{}").unwrap();
        std::fs::write(
            root.join("target/debug/deps/fixture.rlib"),
            b"temporary build",
        )
        .unwrap();
        let db = temp.path().join("index.sqlite");
        let index = Index::open(&db).unwrap();
        let scan = index.begin_scan(&root).unwrap();
        scanner::scan(&db, scan, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        let target = index
            .record_by_path(scan, &root.join("target"))
            .unwrap()
            .row
            .id;
        (temp, index, scan, target)
    }
    #[test]
    fn immutable_plan_confirmation_and_temporary_removal() {
        let (temp, index, scan, target) = fixture();
        let plan = prepare(&index, scan, &[target], "permanent").unwrap();
        assert!(execute(&index, plan, "yes", Arc::new(AtomicBool::new(false))).is_err());
        assert!(temp.path().join("project/target").exists());
        let plan = prepare(&index, scan, &[target], "permanent").unwrap();
        let result = execute(
            &index,
            plan,
            "DELETE PERMANENTLY",
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert_eq!(
            result.items[0].status, "success",
            "{}",
            result.items[0].message
        );
        assert!(!temp.path().join("project/target").exists());
        assert!(temp.path().join("project/src/lib.rs").exists());
    }
    #[test]
    fn changed_contents_refuse_and_source_cannot_be_selected() {
        let (temp, index, scan, target) = fixture();
        let plan = prepare(&index, scan, &[target], "permanent").unwrap();
        std::fs::write(
            temp.path().join("project/target/debug/deps/fixture.rlib"),
            b"changed",
        )
        .unwrap();
        let result = execute(
            &index,
            plan,
            "DELETE PERMANENTLY",
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert_eq!(result.items[0].status, "error");
        assert!(temp.path().join("project/target").exists());
        let source = index
            .record_by_path(scan, &temp.path().join("project/src/lib.rs"))
            .unwrap()
            .row
            .id;
        assert!(prepare(&index, scan, &[source], "trash").is_err());
    }
    #[cfg(unix)]
    #[test]
    fn swapped_symlink_cannot_touch_outside() {
        let (temp, index, scan, target) = fixture();
        let plan = prepare(&index, scan, &[target], "permanent").unwrap();
        let root = temp.path().join("project");
        std::fs::rename(root.join("target"), root.join("saved-target")).unwrap();
        let outside = temp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("valuable"), b"preserve").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("target")).unwrap();
        let result = execute(
            &index,
            plan,
            "DELETE PERMANENTLY",
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert_eq!(result.items[0].status, "error");
        assert_eq!(
            std::fs::read(outside.join("valuable")).unwrap(),
            b"preserve"
        );
    }
    #[test]
    fn cancellation_never_deletes_selected_item() {
        let (temp, index, scan, target) = fixture();
        let plan = prepare(&index, scan, &[target], "permanent").unwrap();
        let result = execute(
            &index,
            plan,
            "DELETE PERMANENTLY",
            Arc::new(AtomicBool::new(true)),
        )
        .unwrap();
        assert!(result.interrupted);
        assert!(temp.path().join("project/target").exists());
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_after_a_file_is_deleted_reports_partial_bytes_and_stopped_state() {
        let (temp, index, _, _) = fixture();
        let directory = temp.path().join("project/target/debug/deps");
        for number in 0..500 {
            std::fs::write(
                directory.join(format!("artifact-{number}.rlib")),
                b"generated data",
            )
            .unwrap();
        }
        let scan = index.begin_scan(&temp.path().join("project")).unwrap();
        scanner::scan(
            &temp.path().join("index.sqlite"),
            scan,
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .unwrap();
        let target = index
            .record_by_path(scan, &temp.path().join("project/target"))
            .unwrap()
            .row
            .id;
        let plan = prepare(&index, scan, &[target], "permanent").unwrap();
        let held =
            crate::native::open_directory(&std::fs::canonicalize(&directory).unwrap()).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let requested = cancel.clone();
        let watcher = std::thread::spawn(move || {
            let started = std::time::Instant::now();
            while started.elapsed() < std::time::Duration::from_secs(5) {
                let current = crate::native::open_child(&held, std::path::Path::new(".")).unwrap();
                let mut names = 0;
                crate::native::for_each_name(&current, |_| {
                    names += 1;
                    Ok(())
                })
                .unwrap();
                if names < 501 {
                    requested.store(true, Ordering::Release);
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            panic!("The generated fixture never reached file deletion");
        });
        let result = execute(&index, plan, "DELETE PERMANENTLY", cancel).unwrap();
        watcher.join().unwrap();
        assert!(result.interrupted);
        assert_eq!(result.items[0].status, "error");
        assert!(result.deleted_bytes > 0);
        assert_eq!(result.deleted_bytes, result.items[0].bytes);
        let remaining = std::fs::read_dir(&directory).unwrap().count();
        assert!(remaining > 0 && remaining < 501);
        assert!(temp.path().join("project/src/lib.rs").exists());
    }
}
