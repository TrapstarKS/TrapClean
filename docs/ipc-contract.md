# Internal UI contract

All Tauri commands use camelCase arguments and serialize fields camelCase. Entry paths are display-only. Cleanup always uses indexed IDs and a backend-owned one-use plan token.

Commands:
- `choose_root() -> string | null` native folder picker; returns an opaque token for a backend-owned native PathBuf
- `start_scan({root: string, scanId?: number}) -> number` immediately returns scan ID; root token for first scan, known scanId for native-identity-preserving rescan
- `cancel_operation() -> void` cooperatively cancel scan, duplicates or AI
- `list_scans() -> ScanSummary[]`; `remove_scan({scanId})`
- `get_scan({scanId}) -> ScanSummary`
- `get_entries({scanId, parentId: number | null, offset, limit, sort: 'size'|'name'|'modified', search: string, candidatesOnly: boolean}) -> EntryPage`
- `get_entry({scanId, entryId}) -> EntryRow`
- `get_breadcrumbs({scanId, entryId}) -> EntryRow[]`
- `find_duplicates({scanId}) -> DuplicateGroup[]` async, cancellable. Maximum 200 groups returned; limitations explicit.
- `create_plan({scanId, entryIds: number[], mode: 'trash'|'permanent'}) -> CleanupPlan`
- `execute_plan({token, confirmation: string}) -> CleanupResult`, confirmation must equal `MOVE TO TRASH` or `DELETE PERMANENTLY`
- `get_provider_settings() -> ProviderSettings`
- `save_provider_settings({settings, key: string | null, sessionOnly: boolean}) -> ProviderSettings`
- `remove_provider_key({mode}) -> ProviderSettings`
- `test_provider() -> ProviderReport`
- `analyze_candidates({scanId, entryIds}) -> ProviderReport`

Events `scan-progress` and `scan-finished` carry ScanSummary. `operation-progress` carries `{kind, processed, message}`. Polling is unnecessary.

`record_frontend_ready()` records a single process-start-to-ready measurement locally. It has no network effects and accepts no client timings. Runtime counters in the local index record JSON payload lengths for progress and paginated replies; they do not log payload contents or secrets.

ProviderSettings: `{mode: 'disabled'|'openai'|'typesafe'|'custom', model: string, endpoint: string, keyStatus: 'missing'|'saved'|'session', status: string}`. No saved key returned. ProviderReport: `{status, message, decisions: [{id, disposition, probability: number|null, confidence: number|null, refused: boolean}], cached: boolean}`. AI cannot change removable or select items. Defaults disabled.

Data structs are authoritative in `src-tauri/src/model.rs`. Scanner/index agent can add backend-only structs and functions. Keep these fields stable or coordinate.
