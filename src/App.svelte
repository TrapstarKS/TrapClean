<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import Icon from './components/Icon.svelte';
  import Modal from './components/Modal.svelte';
  import Treemap from './components/Treemap.svelte';
  import Settings from './components/Settings.svelte';
  import type {
    CleanupPlan,
    CleanupResult,
    DuplicateGroup,
    Entry,
    EntryPage,
    ProviderReport,
    ProviderSettings,
    ScanSummary,
    Sort,
    View,
  } from './types';
  import { bytes, date, disposition, message, shortRoot } from './utils';

  let view = $state<View>('storage');
  let available = $state(false);
  let initialized = $state(false);
  let scans = $state<ScanSummary[]>([]);
  let scan = $state<ScanSummary | null>(null);
  let provider = $state<ProviderSettings>({
    mode: 'disabled',
    model: '',
    endpoint: '',
    keyStatus: 'missing',
    status: '',
  });
  let page = $state<EntryPage>({
    items: [],
    total: 0,
    totalBytes: 0,
    offset: 0,
  });
  let mapPage = $state<EntryPage>({
    items: [],
    total: 0,
    totalBytes: 0,
    offset: 0,
  });
  let crumbs = $state<Entry[]>([]);
  let parentId = $state<number | null>(null);
  let offset = $state(0);
  let sort = $state<Sort>('size');
  let search = $state('');
  let loading = $state(false);
  let busy = $state('');
  let error = $state('');
  let notice = $state('');
  let detail = $state<Entry | null>(null);
  let selection = $state<Entry[]>([]);
  let duplicates = $state<DuplicateGroup[] | null>(null);
  let duplicateOpen = $state(false);
  let cleanupMode = $state<'trash' | 'permanent'>('trash');
  let plan = $state<CleanupPlan | null>(null);
  let confirmation = $state('');
  let result = $state<CleanupResult | null>(null);
  let aiConfirm = $state(false);
  let aiReport = $state<ProviderReport | null>(null);
  let removeId = $state<number | null>(null);
  let operation = $state<{
    kind: string;
    processed: number;
    message: string;
  } | null>(null);
  let searchTimer: ReturnType<typeof setTimeout>;
  let requestId = 0;
  let detailRequestId = 0;
  let lastEntryRefresh = 0;
  let recentElement = $state<HTMLDetailsElement>();
  const scanRunning = $derived(
    scan?.status === 'queued' ||
      scan?.status === 'running' ||
      scan?.status === 'scanning',
  );
  const detailDecision = $derived(
    aiReport?.decisions.find((decision) => decision.id === detail?.id),
  );
  const selectedBytes = $derived(
    selection.reduce((total, entry) => total + entry.logicalBytes, 0),
  );
  const selectedIds = $derived(selection.map((entry) => entry.id));
  const expectedConfirmation = $derived(
    plan?.mode === 'permanent' ? 'DELETE PERMANENTLY' : 'MOVE TO TRASH',
  );
  const aiReady = $derived(
    provider.mode !== 'disabled' &&
      selection.length > 0 &&
      selection.length <= 20 &&
      !scanRunning &&
      !busy,
  );
  const coverage = $derived(
    !scan
      ? 'No scan'
      : scanRunning
        ? 'In progress'
        : scan.status === 'cancelled' || scan.status === 'canceled'
          ? 'Stopped · incomplete'
          : scan.status === 'failed'
            ? 'Interrupted'
            : scan.status === 'stale'
              ? 'Changed · rescan needed'
              : scan.status === 'interrupted'
                ? 'Interrupted · incomplete'
                : scan.status === 'partial' ||
                    scan.errors > 0 ||
                    scan.skipped > 0
                  ? 'Partial · exclusions reported'
                  : scan.status === 'complete'
                    ? 'Completed'
                    : 'Incomplete',
  );

  function updateScan(value: ScanSummary): ScanSummary {
    const previous = scans.find((item) => item.id === value.id);
    const running = ['queued', 'scanning', 'running'];
    if (previous && running.includes(value.status)) {
      if (!running.includes(previous.status)) value = previous;
      else
        value = {
          ...value,
          entries: Math.max(previous.entries, value.entries),
          logicalBytes: Math.max(previous.logicalBytes, value.logicalBytes),
          allocatedBytes: Math.max(
            previous.allocatedBytes,
            value.allocatedBytes,
          ),
          candidateBytes: Math.max(
            previous.candidateBytes,
            value.candidateBytes,
          ),
          errors: Math.max(previous.errors, value.errors),
          skipped: Math.max(previous.skipped, value.skipped),
        };
    }
    scans = [value, ...scans.filter((item) => item.id !== value.id)].sort(
      (a, b) => b.startedAt - a.startedAt,
    );
    if (scan?.id === value.id) scan = value;
    return value;
  }
  onMount(() => {
    let disposed = false;
    const unlisten: UnlistenFn[] = [];
    available = '__TAURI_INTERNALS__' in window;
    if (!available) {
      initialized = true;
      return;
    }
    async function initialize() {
      try {
        const registrations = await Promise.all([
          listen<ScanSummary>('scan-progress', (event) => {
            updateScan(event.payload);
            if (
              scan?.id === event.payload.id &&
              performance.now() - lastEntryRefresh > 1200 &&
              !loading
            ) {
              lastEntryRefresh = performance.now();
              void loadEntries();
            }
          }),
          listen<ScanSummary>('scan-finished', (event) => {
            updateScan(event.payload);
            if (scan?.id === event.payload.id) {
              void loadEntries();
              void refreshDetail();
            }
          }),
          listen<{ kind: string; processed: number; message: string }>(
            'operation-progress',
            (event) => (operation = event.payload),
          ),
          listen<string>(
            'operation-error',
            (event) => void operationFailed(event.payload),
          ),
        ]);
        if (disposed) {
          registrations.forEach((stop) => stop());
          return;
        }
        unlisten.push(...registrations);
        const [history, settings] = await Promise.all([
          invoke<ScanSummary[]>('list_scans'),
          invoke<ProviderSettings>('get_provider_settings'),
        ]);
        if (disposed) return;
        history.forEach(updateScan);
        provider = settings;
        if (scans.length) await openScan(scans[0]);
      } catch (cause) {
        if (!disposed) error = message(cause);
      } finally {
        if (!disposed) {
          initialized = true;
          await tick();
          await new Promise<void>((resolve) =>
            requestAnimationFrame(() => resolve()),
          );
          if (!disposed) void invoke('record_frontend_ready').catch(() => {});
        }
      }
    }
    void initialize();
    return () => {
      disposed = true;
      unlisten.forEach((stop) => stop());
      clearTimeout(searchTimer);
    };
  });

  async function operationFailed(value: string) {
    error = value;
    busy = '';
    operation = null;
    if (scan) {
      try {
        updateScan(await invoke<ScanSummary>('get_scan', { scanId: scan.id }));
        await loadEntries();
      } catch {
        notice =
          'The operation stopped. Rescan this folder to refresh its index.';
      }
    }
  }
  async function openScan(value: ScanSummary) {
    if (busy) return;
    value = updateScan(value);
    scan = value;
    parentId = value.rootId;
    offset = 0;
    search = '';
    sort = 'size';
    crumbs = [];
    detail = null;
    detailRequestId++;
    selection = [];
    duplicates = null;
    aiReport = null;
    result = null;
    error = '';
    notice = '';
    if (recentElement) recentElement.open = false;
    await Promise.all([loadEntries(), loadCrumbs(value.rootId)]);
  }
  async function loadEntries() {
    if (!scan || view === 'settings') return;
    const id = ++requestId;
    loading = true;
    const currentScanId = scan.id;
    const cleanup = view === 'cleanup';
    try {
      const query = {
        scanId: currentScanId,
        parentId: cleanup ? null : parentId,
        offset,
        limit: 100,
        sort,
        search: search.trim(),
        candidatesOnly: cleanup,
      };
      const [entries, map] = await Promise.all([
        invoke<EntryPage>('get_entries', query),
        cleanup
          ? Promise.resolve(null)
          : invoke<EntryPage>('get_entries', {
              scanId: currentScanId,
              parentId,
              offset: 0,
              limit: 18,
              sort: 'size',
              search: '',
              candidatesOnly: false,
            }),
      ]);
      if (id !== requestId) return;
      page = entries;
      if (map) mapPage = map;
      if (offset > 0 && !entries.items.length) {
        offset = 0;
        void loadEntries();
      }
    } catch (cause) {
      if (id === requestId) error = message(cause);
    } finally {
      if (id === requestId) loading = false;
    }
  }
  async function loadCrumbs(entryId: number) {
    if (!scan) return;
    const scanId = scan.id;
    try {
      const value = await invoke<Entry[]>('get_breadcrumbs', {
        scanId,
        entryId,
      });
      if (scan?.id === scanId && parentId === entryId) crumbs = value;
    } catch (cause) {
      error = message(cause);
    }
  }
  async function changeView(value: View) {
    view = value;
    offset = 0;
    search = '';
    error = '';
    if (value !== 'settings') await loadEntries();
  }
  async function chooseRoot() {
    if (busy || scanRunning) return;
    busy = 'choose';
    error = '';
    notice = '';
    try {
      const root = await invoke<string | null>('choose_root');
      if (!root) return;
      view = 'storage';
      const id = await invoke<number>('start_scan', { root });
      const value = await invoke<ScanSummary>('get_scan', { scanId: id });
      busy = '';
      await openScan(updateScan(value));
    } catch (cause) {
      error = message(cause);
    } finally {
      busy = '';
    }
  }
  async function rescan() {
    if (!scan || busy || scanRunning) return;
    const root = scan.root;
    const scanId = scan.id;
    busy = 'scan';
    error = '';
    notice = '';
    try {
      const id = await invoke<number>('start_scan', { root, scanId });
      const value = await invoke<ScanSummary>('get_scan', { scanId: id });
      busy = '';
      await openScan(updateScan(value));
    } catch (cause) {
      error = message(cause);
    } finally {
      busy = '';
    }
  }
  async function cancel() {
    try {
      await invoke('cancel_operation');
      notice =
        'Cancellation requested. The current filesystem operation will finish first.';
    } catch (cause) {
      error = message(cause);
    }
  }
  async function navigate(entry: Entry) {
    if (entry.kind !== 'directory' && entry.kind !== 'dir') {
      await inspect(entry);
      return;
    }
    parentId = entry.id;
    offset = 0;
    search = '';
    detail = null;
    detailRequestId++;
    await Promise.all([loadEntries(), loadCrumbs(entry.id)]);
  }
  async function inspect(entry: Entry) {
    if (!scan) return;
    const id = ++detailRequestId;
    const scanId = scan.id;
    try {
      const value = await invoke<Entry>('get_entry', {
        scanId,
        entryId: entry.id,
      });
      if (id === detailRequestId && scan?.id === scanId) detail = value;
    } catch (cause) {
      error = message(cause);
    }
  }
  async function refreshDetail() {
    if (detail) await inspect(detail);
  }
  function toggle(entry: Entry) {
    if (!entry.removable) return;
    selection = selectedIds.includes(entry.id)
      ? selection.filter((item) => item.id !== entry.id)
      : [...selection, entry];
    aiReport = null;
  }
  function searchChanged(value: string) {
    search = value;
    offset = 0;
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => void loadEntries(), 220);
  }
  async function changeSort(value: Sort) {
    sort = value;
    offset = 0;
    await loadEntries();
  }
  async function changePage(value: number) {
    offset = Math.max(0, value);
    await loadEntries();
  }
  async function preview() {
    if (!scan || !selection.length || busy || scanRunning) return;
    busy = 'preview';
    error = '';
    confirmation = '';
    try {
      plan = await invoke<CleanupPlan>('create_plan', {
        scanId: scan.id,
        entryIds: selectedIds,
        mode: cleanupMode,
      });
    } catch (cause) {
      error = message(cause);
    } finally {
      busy = '';
    }
  }
  async function execute() {
    if (!plan || confirmation !== expectedConfirmation) return;
    const token = plan.token;
    const typedConfirmation = confirmation;
    busy = 'cleanup';
    error = '';
    result = null;
    try {
      result = await invoke<CleanupResult>('execute_plan', {
        token,
        confirmation: typedConfirmation,
      });
      plan = null;
      selection = [];
      detail = null;
      duplicates = null;
      aiReport = null;
      if (scan) {
        updateScan(await invoke<ScanSummary>('get_scan', { scanId: scan.id }));
        await loadEntries();
        const id = await invoke<number>('start_scan', {
          root: scan.root,
          scanId: scan.id,
        });
        const refreshed = updateScan(
          await invoke<ScanSummary>('get_scan', { scanId: id }),
        );
        scan = refreshed;
        parentId = refreshed.rootId;
        offset = 0;
        search = '';
        crumbs = [];
        updateScan(refreshed);
        await Promise.all([loadEntries(), loadCrumbs(refreshed.rootId)]);
      }
    } catch (cause) {
      plan = null;
      error = message(cause);
    } finally {
      busy = '';
    }
  }
  async function findDuplicates() {
    if (!scan || busy || scanRunning) return;
    busy = 'duplicates';
    error = '';
    operation = null;
    duplicateOpen = true;
    try {
      duplicates = await invoke<DuplicateGroup[]>('find_duplicates', {
        scanId: scan.id,
      });
    } catch (cause) {
      error = message(cause);
    } finally {
      busy = '';
      operation = null;
    }
  }
  async function analyze() {
    if (!scan || !aiReady) return;
    aiConfirm = false;
    busy = 'ai';
    error = '';
    operation = null;
    aiReport = null;
    try {
      aiReport = await invoke<ProviderReport>('analyze_candidates', {
        scanId: scan.id,
        entryIds: selectedIds,
      });
    } catch (cause) {
      error = message(cause);
    } finally {
      busy = '';
      operation = null;
    }
  }
  async function removeScan() {
    if (removeId === null) return;
    const id = removeId;
    removeId = null;
    busy = 'remove';
    error = '';
    try {
      await invoke('remove_scan', { scanId: id });
      scans = scans.filter((item) => item.id !== id);
      if (scan?.id === id) {
        scan = null;
        detail = null;
        selection = [];
        page = { items: [], total: 0, totalBytes: 0, offset: 0 };
        busy = '';
        if (scans.length) await openScan(scans[0]);
      }
      notice = 'Scan history removed. Files on disk were not changed.';
    } catch (cause) {
      error = message(cause);
    } finally {
      busy = '';
    }
  }
</script>

<svelte:head
  ><title>TrapClean</title><meta
    name="description"
    content="Understand your storage. Review the evidence. Make room."
  /></svelte:head
>
<div class="app-shell">
  <aside class="sidebar">
    <a
      class="brand"
      href="#storage"
      onclick={(event) => {
        event.preventDefault();
        void changeView('storage');
      }}
      aria-label="TrapClean storage"
      ><span class="brand-mark"
        ><svg viewBox="0 0 32 32" aria-hidden="true"
          ><rect
            x="5"
            y="5"
            width="20"
            height="7"
            rx="2"
            fill="currentColor"
          /><rect
            x="5"
            y="16"
            width="13"
            height="10"
            rx="2"
            fill="currentColor"
            opacity=".55"
          /><path
            d="m23 17 2 3 3 1-3 2-2 4-2-4-3-2 3-1z"
            fill="currentColor"
          /></svg
        ></span
      ><span>TrapClean<span class="brand-version">0.1</span></span></a
    >
    <nav aria-label="Main navigation">
      <button
        class:active={view === 'storage'}
        aria-current={view === 'storage' ? 'page' : undefined}
        onclick={() => changeView('storage')}
        ><Icon name="storage" /><span>Storage</span></button
      ><button
        class:active={view === 'cleanup'}
        aria-current={view === 'cleanup' ? 'page' : undefined}
        onclick={() => changeView('cleanup')}
        ><Icon name="clean" /><span>Cleanup</span>{#if selection.length}<span
            class="nav-count">{selection.length}</span
          >{/if}</button
      ><button
        class:active={view === 'settings'}
        aria-current={view === 'settings' ? 'page' : undefined}
        onclick={() => changeView('settings')}
        ><Icon name="settings" /><span>Settings</span></button
      >
    </nav>
    <div class="sidebar-bottom">
      <span class="status-dot success"></span><span
        >Local first<small>Your storage, your decision.</small></span
      >
    </div>
  </aside>
  <main id="main-content">
    <header class="topbar">
      <div>
        <span class="eyebrow"
          >{view === 'settings'
            ? 'YOUR PREFERENCES'
            : view === 'cleanup'
              ? 'REVIEW BEFORE REMOVING'
              : 'A CLEARER VIEW OF YOUR DISK'}</span
        >
        <h1>
          {view === 'settings'
            ? 'Settings'
            : view === 'cleanup'
              ? 'Cleanup'
              : 'Storage'}
        </h1>
      </div>
      <div class="topbar-actions">
        {#if view !== 'settings'}<details
            class="recent-scans"
            bind:this={recentElement}
          >
            <summary class="button"
              ><Icon name="history" /><span>Recent scans</span><Icon
                name="down"
                size={14}
              /></summary
            >
            <div class="history-menu">
              <div class="menu-title">
                Recent scans <span>{scans.length}</span>
              </div>
              {#each scans as item}<div class="history-item">
                  <button onclick={() => openScan(item)} disabled={!!busy}
                    ><strong>{shortRoot(item.root)}</strong><span
                      >{date(item.startedAt)} · {bytes(item.logicalBytes)}</span
                    ></button
                  ><button
                    class="icon-button"
                    aria-label={`Remove scan of ${shortRoot(item.root)} from history`}
                    disabled={!!busy ||
                      ['queued', 'scanning', 'running'].includes(item.status)}
                    onclick={() => {
                      removeId = item.id;
                      if (recentElement) recentElement.open = false;
                    }}><Icon name="close" size={15} /></button
                  >
                </div>{:else}<p class="menu-empty">
                  Your completed scans appear here.
                </p>{/each}
            </div>
          </details>
          <button
            class="primary"
            onclick={chooseRoot}
            disabled={!available || !!busy || scanRunning}
            ><Icon name="folder" /><span>Choose folder</span></button
          >{/if}
      </div>
    </header>

    {#if error}<div class="alert error global-alert" role="alert">
        <Icon name="warning" />
        <p>{error}</p>
        <button
          class="icon-button"
          aria-label="Dismiss error"
          onclick={() => (error = '')}><Icon name="close" size={16} /></button
        >
      </div>{/if}
    {#if notice}<div class="alert global-alert" role="status">
        <Icon name="info" />
        <p>{notice}</p>
        <button
          class="icon-button"
          aria-label="Dismiss notice"
          onclick={() => (notice = '')}><Icon name="close" size={16} /></button
        >
      </div>{/if}
    {#if scanRunning || busy === 'duplicates' || busy === 'ai'}<div
        class="activity-strip"
        role="status"
      >
        <span class="spinner"></span>
        <div>
          <strong
            >{scanRunning
              ? 'Scanning your folder'
              : busy === 'duplicates'
                ? 'Verifying duplicate contents'
                : 'Waiting for the decision provider'}</strong
          ><span
            >{scanRunning
              ? `${scan?.entries.toLocaleString()} entries indexed · ${bytes(scan?.logicalBytes ?? 0)} found`
              : operation?.message ||
                'Working in the background. You can keep browsing.'}</span
          >
        </div>
        <button onclick={cancel}>Cancel</button>
      </div>{/if}

    {#if view === 'settings'}
      <Settings
        settings={provider}
        onsaved={(value) => (provider = value)}
        {available}
        locked={busy === 'ai'}
      />
    {:else if !scan}
      <section class="welcome" aria-labelledby="welcome-heading">
        <span class="welcome-symbol"><Icon name="storage" size={44} /></span
        ><span class="eyebrow">SPACE FOR WHAT MATTERS</span>
        <h2 id="welcome-heading">Know what’s taking up space.</h2>
        <p>
          Choose a folder to see its largest files, discover rebuildable caches,
          and review what you can clear.
        </p>
        <button
          class="primary"
          onclick={chooseRoot}
          disabled={!available || !!busy}
          ><Icon name="folder" />Choose a folder to scan</button
        >
        <div class="welcome-details">
          <span
            ><Icon name="shield" size={16} /> No files removed during scanning</span
          ><span><Icon name="spark" size={16} /> AI is optional</span>
        </div>
        {#if !available}<p class="desktop-note">
            Open the installed desktop app to access your filesystem.
          </p>{:else if !initialized}<p class="muted">
            Opening your local index…
          </p>{/if}
      </section>
    {:else}
      <div class="scan-heading">
        <div class="scan-location">
          <span class="location-icon"><Icon name="folder" size={22} /></span>
          <div>
            <strong>{shortRoot(scan.root)}</strong><span title={scan.root}
              >{scan.root}</span
            >
          </div>
        </div>
        <div class="scan-meta">
          <span class="badge" class:warning={coverage !== 'Completed'}
            ><span class="status-dot" class:success={coverage === 'Completed'}
            ></span>{coverage}</span
          ><button
            class="icon-button"
            aria-label="Scan this folder again"
            title="Scan this folder again"
            onclick={rescan}
            disabled={!!busy || scanRunning}><Icon name="refresh" /></button
          >
        </div>
      </div>
      <section class="metrics" aria-label="Scan summary">
        <div>
          <span>Logical size</span><strong>{bytes(scan.logicalBytes)}</strong
          ><small>{scan.entries.toLocaleString()} entries indexed</small>
        </div>
        <div>
          <span>Allocated space</span><strong
            >{bytes(scan.allocatedBytes)}{#if scan.allocatedEstimated}<span
                class="estimate">≈</span
              >{/if}</strong
          ><small
            >{scan.allocatedEstimated
              ? 'Includes estimates where unavailable'
              : 'Filesystem-reported allocation'}</small
          >
        </div>
        <div class="metric-accent">
          <span>Cleanup candidates</span><strong
            >{bytes(scan.candidateBytes)}</strong
          ><small>Review required before removal</small>
        </div>
        <div>
          <span>Scan coverage</span><strong class="coverage-value"
            >{coverage === 'Partial · exclusions reported'
              ? 'Partial coverage'
              : coverage === 'Changed · rescan needed'
                ? 'Needs rescan'
                : coverage}</strong
          ><small
            >{scan.errors.toLocaleString()} errors · {scan.skipped.toLocaleString()}
            skipped</small
          >
        </div>
      </section>
      {#if coverage !== 'Completed' && !scanRunning}<div
          class="inline-note coverage-note"
        >
          <Icon name="info" />
          <p>
            This scan has incomplete, changed or excluded coverage. Review item
            details; partially inspected folders cannot be recommended for
            cleanup.
          </p>
        </div>{/if}

      {#if view === 'cleanup'}
        <div class="cleanup-heading">
          <div>
            <h2>Review your candidates</h2>
            <p>
              Evidence comes first. Only items you select enter a cleanup plan.
            </p>
          </div>
          <button onclick={findDuplicates} disabled={!!busy || scanRunning}
            ><Icon name="copy" />{duplicates === null
              ? 'Find duplicates'
              : 'Check duplicates again'}</button
          >
        </div>
        {#if aiReport}<div class="alert ai-report" role="status">
            <Icon name="spark" />
            <div>
              <strong
                >{aiReport.status === 'ok' || aiReport.status === 'success'
                  ? 'AI analysis returned'
                  : 'Local recommendations retained'}</strong
              >
              <p>{aiReport.message}</p>
              {#if aiReport.decisions.length}<p>
                  {aiReport.decisions.length} decisions · {aiReport.cached
                    ? 'Previously cached results'
                    : 'Provider response'}. No items were automatically
                  selected.
                </p>{/if}
            </div>
            <button
              class="icon-button"
              aria-label="Dismiss AI report"
              onclick={() => (aiReport = null)}><Icon name="close" /></button
            >
          </div>{/if}
        {#if result}<section
            class="panel cleanup-result"
            aria-label="Cleanup result"
          >
            <div class="section-heading">
              <div>
                <h2>
                  {result.interrupted
                    ? 'Cleanup interrupted'
                    : 'Cleanup finished'}
                </h2>
                <p>
                  {bytes(result.movedBytes)} moved to trash · {bytes(
                    result.deletedBytes,
                  )} permanently deleted
                </p>
              </div>
              <button
                class="icon-button"
                aria-label="Dismiss cleanup result"
                onclick={() => (result = null)}><Icon name="close" /></button
              >
            </div>
            <div class="result-free-space">
              <Icon name="storage" /><span
                >Measured free-space change: <strong
                  >{result.freeSpaceChange === null
                    ? 'Unavailable'
                    : `${result.freeSpaceChange > 0 ? '+' : ''}${bytes(result.freeSpaceChange)}`}</strong
                ></span
              >
            </div>
            <p class="muted">
              Trash normally retains disk space until emptied. Snapshots, open
              files and concurrent disk activity can affect this measurement.
              Restore trashed items using your system’s trash.
            </p>
            <ul class="result-list">
              {#each result.items as item}<li>
                  <Icon
                    name={item.status === 'success' ||
                    item.status === 'moved' ||
                    item.status === 'deleted'
                      ? 'check'
                      : 'warning'}
                    size={16}
                  />
                  <div>
                    <strong>{item.relativePath}</strong><span
                      >{item.message}</span
                    >
                  </div>
                  <span class="badge">{item.status}</span>
                </li>{/each}
            </ul>
          </section>{/if}
      {/if}

      <div class="analysis-layout" class:has-detail={!!detail}>
        <div class="analysis-primary">
          {#if view === 'storage'}<section class="panel map-panel">
              <div class="section-heading">
                <div>
                  <h2>Where your space goes</h2>
                  <span class="muted"
                    >Box area represents logical size. Open a folder to explore.</span
                  >
                </div>
                <span class="map-total">{bytes(mapPage.totalBytes)}</span>
              </div>
              <Treemap
                page={mapPage}
                onopen={navigate}
                onother={() => {
                  sort = 'size';
                  search = '';
                  void changePage(18);
                }}
              />
              <div class="map-caption">
                <span
                  ><span class="legend-dot"></span>Largest 18 items + all
                  remaining space as Other</span
                ><span>Allocation and recoverable space can differ.</span>
              </div>
            </section>{/if}
          <section
            class="panel entries-panel"
            aria-labelledby="entries-heading"
            aria-busy={loading}
          >
            <div class="entries-toolbar">
              <div>
                {#if view === 'storage'}<nav
                    class="breadcrumbs"
                    aria-label="Folder breadcrumbs"
                  >
                    {#if crumbs.length}{#each crumbs as crumb, index}{#if index}<Icon
                            name="chevron"
                            size={12}
                          />{/if}<button
                          onclick={() => navigate(crumb)}
                          aria-current={index === crumbs.length - 1
                            ? 'location'
                            : undefined}
                          >{index === 0
                            ? shortRoot(scan.root)
                            : crumb.name}</button
                        >{/each}{:else}<span>{shortRoot(scan.root)}</span>{/if}
                  </nav>{:else}<h2 id="entries-heading">
                    Local recommendations <span class="muted"
                      >({page.total.toLocaleString()})</span
                    >
                  </h2>{/if}
              </div>
              <label class="search-field"
                ><Icon name="search" size={16} /><input
                  aria-label={view === 'cleanup'
                    ? 'Search cleanup candidates'
                    : 'Search this folder'}
                  value={search}
                  oninput={(event) => searchChanged(event.currentTarget.value)}
                  placeholder={view === 'cleanup'
                    ? 'Search candidates'
                    : 'Search this folder'}
                />{#if search}<button
                    class="icon-button"
                    aria-label="Clear search"
                    onclick={() => {
                      search = '';
                      offset = 0;
                      void loadEntries();
                    }}><Icon name="close" size={14} /></button
                  >{/if}</label
              >
            </div>
            <div class="table-scroll">
              <table>
                <thead
                  ><tr
                    ><th class="select-column"
                      ><span class="sr-only">Select</span></th
                    ><th aria-sort={sort === 'name' ? 'ascending' : 'none'}
                      ><button onclick={() => changeSort('name')}
                        >Name {#if sort === 'name'}<Icon
                            name="down"
                            size={12}
                          />{/if}</button
                      ></th
                    ><th
                      aria-sort={sort === 'size' ? 'descending' : 'none'}
                      class="number-column"
                      ><button onclick={() => changeSort('size')}
                        >Size {#if sort === 'size'}<Icon
                            name="down"
                            size={12}
                          />{/if}</button
                      ></th
                    ><th class="recommendation-column">Recommendation</th><th
                      aria-sort={sort === 'modified' ? 'descending' : 'none'}
                      class="modified-column"
                      ><button onclick={() => changeSort('modified')}
                        >Last modified {#if sort === 'modified'}<Icon
                            name="down"
                            size={12}
                          />{/if}</button
                      ></th
                    ><th class="inspect-column"
                      ><span class="sr-only">Details</span></th
                    ></tr
                  ></thead
                ><tbody
                  >{#each page.items as entry}<tr
                      class:row-selected={selectedIds.includes(entry.id)}
                      class:row-inspected={detail?.id === entry.id}
                      ><td
                        ><input
                          type="checkbox"
                          checked={selectedIds.includes(entry.id)}
                          disabled={!entry.removable || busy === 'cleanup'}
                          onchange={() => toggle(entry)}
                          aria-label={`Select ${entry.name} for cleanup`}
                          title={entry.removable
                            ? 'Add to your selection'
                            : 'Protected or requires more evidence; see details'}
                        /></td
                      ><td
                        ><div class="entry-name">
                          <Icon
                            name={entry.kind === 'directory' ||
                            entry.kind === 'dir'
                              ? 'folder'
                              : 'file'}
                            size={18}
                          /><button
                            class="entry-link"
                            title={entry.relativePath}
                            onclick={() =>
                              view === 'storage'
                                ? navigate(entry)
                                : inspect(entry)}
                            >{entry.name}{#if entry.kind === 'directory' || entry.kind === 'dir'}<Icon
                                name="chevron"
                                size={13}
                              />{/if}</button
                          >
                        </div>
                        {#if view === 'cleanup'}<span
                            class="entry-path"
                            title={entry.relativePath}
                            >{entry.relativePath}</span
                          >{/if}</td
                      ><td class="number-column">{bytes(entry.logicalBytes)}</td
                      ><td class="recommendation-column"
                        ><span
                          class="disposition"
                          class:candidate={disposition(entry.disposition) ===
                            'Candidate for cleanup'}
                          class:keep={disposition(entry.disposition) === 'Keep'}
                          >{disposition(entry.disposition)}</span
                        ></td
                      ><td class="modified-column"
                        >{entry.modifiedAt === null
                          ? 'Unknown'
                          : new Date(
                              entry.modifiedAt * 1000,
                            ).toLocaleDateString()}</td
                      ><td
                        ><button
                          class="icon-button"
                          aria-label={`Inspect ${entry.name}`}
                          onclick={() => inspect(entry)}
                          ><Icon name="info" size={17} /></button
                        ></td
                      ></tr
                    >{:else}<tr
                      ><td colspan="6" class="table-empty"
                        >{loading
                          ? 'Loading entries…'
                          : search
                            ? 'No entries match your search.'
                            : view === 'cleanup'
                              ? 'No local cleanup candidates in this scan. You can inspect items in Storage or check duplicates.'
                              : 'No entries in this folder yet.'}</td
                      ></tr
                    >{/each}</tbody
                >
              </table>
            </div>
            <footer class="table-footer">
              <span
                >{page.total
                  ? `${(offset + 1).toLocaleString()}–${Math.min(offset + page.items.length, page.total).toLocaleString()} of ${page.total.toLocaleString()}`
                  : '0 entries'}{#if loading}<span class="muted">
                    · Updating…</span
                  >{/if}</span
              >
              <div>
                <button
                  class="icon-button"
                  aria-label="Previous page"
                  disabled={offset === 0 || loading}
                  onclick={() => changePage(offset - 100)}
                  ><Icon name="back" size={16} /></button
                ><button
                  class="icon-button"
                  aria-label="Next page"
                  disabled={offset + page.items.length >= page.total || loading}
                  onclick={() => changePage(offset + 100)}
                  ><Icon name="chevron" size={16} /></button
                >
              </div>
            </footer>
          </section>
          {#if view === 'cleanup' && duplicateOpen}<section
              class="panel duplicates-panel"
            >
              <div class="section-heading">
                <div>
                  <h2>Content-verified duplicates</h2>
                  <p>
                    {busy === 'duplicates'
                      ? 'Comparing content with streaming hashes…'
                      : duplicates === null
                        ? 'Duplicate analysis did not finish.'
                        : `${duplicates.length} groups found. Choose copies yourself; nothing is selected automatically.`}
                  </p>
                </div>
                <button
                  class="icon-button"
                  aria-label="Hide duplicate groups"
                  onclick={() => (duplicateOpen = false)}
                  ><Icon name="close" /></button
                >
              </div>
              <p class="muted">
                Up to 200 groups are shown. Hardlinks are not independent
                copies. A verified match establishes equal content, not that
                either copy is disposable.
              </p>
              {#if duplicates?.length}{#each duplicates as group}<div
                    class="duplicate-group"
                  >
                    <div class="duplicate-group-heading">
                      <Icon name="copy" size={16} /><strong
                        >{group.items.length} identical files</strong
                      ><span>{bytes(group.bytesEach)} each</span>
                    </div>
                    {#each group.items as entry}<div class="duplicate-item">
                        <input
                          type="checkbox"
                          checked={selectedIds.includes(entry.id)}
                          disabled={!entry.removable}
                          onchange={() => toggle(entry)}
                          aria-label={`Select duplicate ${entry.relativePath}`}
                        /><button
                          class="entry-link"
                          onclick={() => inspect(entry)}
                          >{entry.relativePath}</button
                        ><button
                          class="icon-button"
                          aria-label={`Inspect duplicate ${entry.relativePath}`}
                          onclick={() => inspect(entry)}
                          ><Icon name="info" size={16} /></button
                        >
                      </div>{/each}
                  </div>{/each}{:else if duplicates}<div class="compact-empty">
                  <Icon name="check" />
                  <p>
                    No verified duplicate groups found in the completed scan.
                  </p>
                </div>{/if}
            </section>{/if}
        </div>
        {#if detail}<aside
            class="panel details-panel"
            aria-labelledby="detail-heading"
          >
            <div class="detail-heading">
              <span class="detail-icon"
                ><Icon
                  name={detail.kind === 'directory' || detail.kind === 'dir'
                    ? 'folder'
                    : 'file'}
                  size={24}
                /></span
              ><button
                class="icon-button"
                aria-label="Close item details"
                onclick={() => {
                  detail = null;
                  detailRequestId++;
                }}><Icon name="close" /></button
              >
            </div>
            <span class="eyebrow">ITEM DETAILS</span>
            <h2 id="detail-heading">{detail.name}</h2>
            <p class="detail-path">{detail.relativePath || scan.root}</p>
            <span
              class="disposition"
              class:candidate={disposition(detail.disposition) ===
                'Candidate for cleanup'}
              class:keep={disposition(detail.disposition) === 'Keep'}
              >{disposition(detail.disposition)}</span
            >
            <dl class="detail-facts">
              <div>
                <dt>Logical size</dt>
                <dd>{bytes(detail.logicalBytes)}</dd>
              </div>
              <div>
                <dt>Allocated</dt>
                <dd>{bytes(detail.allocatedBytes)}</dd>
              </div>
              <div>
                <dt>Category</dt>
                <dd>{detail.category.replaceAll('_', ' ')}</dd>
              </div>
              <div>
                <dt>Coverage</dt>
                <dd>{detail.complete ? 'Inspected' : 'Incomplete'}</dd>
              </div>
              {#if detail.project}<div>
                  <dt>Related project</dt>
                  <dd>{detail.project}</dd>
                </div>{/if}
            </dl>
            <div class="detail-section">
              <h3>Why this recommendation</h3>
              <p>
                {detail.reason || 'No reconstruction evidence was recognized.'}
              </p>
            </div>
            {#if detail.evidence.length}<div class="detail-section">
                <h3>Observed evidence</h3>
                <ul>
                  {#each detail.evidence as evidence}<li>{evidence}</li>{/each}
                </ul>
              </div>{/if}
            <div class="detail-section">
              <h3>What removal means</h3>
              <p>
                {detail.consequence ||
                  'Review the item and its purpose before choosing to remove it.'}
              </p>
            </div>
            <div class="detail-section uncertainty">
              <h3>What remains uncertain</h3>
              <p>
                {detail.uncertainty ||
                  'Future use is unknown. A recommendation is not a guarantee of safe removal.'}
              </p>
            </div>
            <div class="detail-section">
              <h3>Space limits</h3>
              <p>
                Logical size measures file length. Allocation may be estimated.
                Hardlinks, sparse files, compression, clones and snapshots can
                make recovered space differ. Trash retains allocation until
                emptied.
              </p>
            </div>
            <div class="detail-section">
              <h3>Filesystem dates</h3>
              <dl class="date-facts">
                <dt>Last modified</dt>
                <dd>{date(detail.modifiedAt)}</dd>
                <dt>Access time reported by filesystem</dt>
                <dd>{date(detail.accessedAt)}</dd>
              </dl>
              <p class="muted">
                Access time may be disabled or delayed. It is not proof of when
                an item was last used. Usage is unknown.
              </p>
            </div>
            {#if detail.error}<div class="alert error">
                <Icon name="warning" />
                <p>{detail.error}</p>
              </div>{/if}{#if detailDecision}{@const decision = detailDecision}
              <div class="detail-section">
                <h3>Provider recommendation</h3>
                <p>
                  {decision.refused
                    ? 'Insufficient evidence or provider refusal. Local recommendation retained.'
                    : disposition(decision.disposition)}
                </p>
                {#if decision.probability !== null}<p class="muted">
                    Class probability: {(decision.probability * 100).toFixed(
                      1,
                    )}%. This does not establish removal safety.
                  </p>{/if}{#if decision.confidence !== null}<p class="muted">
                    Provider-reported confidence: {decision.confidence.toLocaleString()}.
                  </p>{/if}
              </div>{/if}<button
              class={selectedIds.includes(detail.id) ? '' : 'primary'}
              disabled={!detail.removable || busy === 'cleanup'}
              onclick={() => detail && toggle(detail)}
              ><Icon
                name={selectedIds.includes(detail.id) ? 'minus' : 'check'}
              />{selectedIds.includes(detail.id)
                ? 'Remove from selection'
                : detail.removable
                  ? 'Select for review'
                  : 'Protected from cleanup'}</button
            >
          </aside>{/if}
      </div>
      {#if selection.length || view === 'cleanup'}<section
          class="selection-bar"
          aria-label="Cleanup selection"
        >
          <div class="selection-summary">
            <span class="selection-icon"><Icon name="check" /></span>
            <div>
              <strong
                >{selection.length}
                {selection.length === 1 ? 'item' : 'items'} selected
                <span>{bytes(selectedBytes)}</span></strong
              ><small
                >{selection.length
                  ? 'Review the exact plan before making changes.'
                  : 'Select candidates or verified duplicates to begin.'}</small
              >
            </div>
            {#if selection.length}<button
                class="text-button"
                onclick={() => {
                  selection = [];
                  aiReport = null;
                }}
                disabled={busy === 'cleanup'}>Clear</button
              >{/if}
          </div>
          <div class="selection-actions">
            <button
              onclick={() => (aiConfirm = true)}
              disabled={!aiReady}
              title={provider.mode === 'disabled'
                ? 'Enable a decision provider in Settings'
                : selection.length > 20
                  ? 'Select at most 20 candidates per AI request'
                  : 'Preview metadata before sending'}
              ><Icon name="spark" />Analyze with AI</button
            ><label class="mode-select"
              ><span class="sr-only">Cleanup action</span><select
                bind:value={cleanupMode}
                disabled={busy === 'cleanup'}
                ><option value="trash">Move to trash</option><option
                  value="permanent">Delete permanently</option
                ></select
              ></label
            ><button
              class:danger={cleanupMode === 'permanent'}
              class:primary={cleanupMode === 'trash'}
              onclick={preview}
              disabled={!selection.length || !!busy || scanRunning}
              ><Icon name="trash" />{busy === 'preview'
                ? 'Preparing…'
                : 'Preview cleanup'}</button
            >
          </div>
        </section>{/if}
    {/if}
    <footer class="app-footer">
      <span
        ><Icon name="shield" size={13} />{provider.mode === 'disabled'
          ? 'AI disabled · local recommendations'
          : `${provider.mode === 'typesafe' ? 'JEV / TypeSafe' : provider.mode === 'openai' ? 'OpenAI' : 'Custom provider'} · requests only on your action`}</span
      ><span>Space estimates are not guaranteed recovery.</span>
    </footer>
  </main>
</div>

{#if plan}<Modal
    title={plan.mode === 'permanent'
      ? 'Review permanent deletion'
      : 'Review move to trash'}
    busy={busy === 'cleanup'}
    onclose={() => (plan = null)}
    ><div class="modal-body">
      <div class="plan-summary">
        <strong
          >{plan.items.length}
          {plan.items.length === 1 ? 'item' : 'items'}</strong
        ><span>{bytes(plan.estimatedBytes)} logical size</span>
      </div>
      <div class="inline-note" class:danger-note={plan.mode === 'permanent'}>
        <Icon name={plan.mode === 'permanent' ? 'warning' : 'info'} />
        <p>
          {plan.mode === 'permanent'
            ? 'Permanent deletion cannot be undone. TrapClean rechecks each item before removal and refuses items that changed or fall outside the plan.'
            : 'Items go to your system’s trash. This usually does not free disk space until you empty it. Restore items using your system’s trash; TrapClean does not offer an Undo.'}
        </p>
      </div>
      <ul class="plan-items">
        {#each plan.items as item}<li>
            <div>
              <strong>{item.relativePath}</strong><span>{item.reason}</span>
              <p>{item.consequence}</p>
            </div>
            <span>{bytes(item.logicalBytes)}</span>
          </li>{/each}
      </ul>
      <p class="muted">
        This reviewed plan is fixed and expires at {date(plan.expiresAt)}. A
        failed item will not fall back to a different removal method.
      </p>
      <label class="confirmation-field"
        >Type <strong>{expectedConfirmation}</strong> to confirm<input
          bind:value={confirmation}
          placeholder={expectedConfirmation}
          autocomplete="off"
          spellcheck="false"
          disabled={busy === 'cleanup'}
        /></label
      >
    </div>
    <footer class="modal-actions">
      {#if busy === 'cleanup'}<button onclick={cancel}
          >Stop remaining items</button
        >{:else}<button onclick={() => (plan = null)}>Cancel</button
        >{/if}<button
        class:danger={plan.mode === 'permanent'}
        class:primary={plan.mode === 'trash'}
        disabled={confirmation !== expectedConfirmation || busy === 'cleanup'}
        onclick={execute}
        >{busy === 'cleanup'
          ? 'Applying reviewed plan…'
          : plan.mode === 'permanent'
            ? 'Delete selected items permanently'
            : 'Move selected items to trash'}</button
      >
    </footer></Modal
  >{/if}
{#if aiConfirm}<Modal
    title="Review what AI receives"
    onclose={() => (aiConfirm = false)}
    ><div class="modal-body">
      <div class="inline-note">
        <Icon name="spark" />
        <p>
          This sends limited metadata for {selection.length} selected candidates to
          {provider.mode === 'typesafe'
            ? 'JEV / TypeSafe'
            : provider.mode === 'openai'
              ? 'OpenAI'
              : 'your custom endpoint'}. It may consume provider credits. Your
          selection and cleanup protections stay under your control.
        </p>
      </div>
      <dl class="privacy-preview">
        <div>
          <dt>Included</dt>
          <dd>
            Candidate IDs; redacted root-relative paths with only known tool
            components; category; logical and allocated size; filesystem dates;
            coverage; local recommendation and protection flags; structured
            reconstruction observations; project-present flag; usage marked
            unknown.
          </dd>
        </div>
        <div>
          <dt>Stays on this device</dt>
          <dd>
            File and manifest contents, absolute paths, personal and project
            names, arbitrary filenames, freeform evidence, usernames and
            credentials.
          </dd>
        </div>
        <div>
          <dt>Limits</dt>
          <dd>
            At most 20 candidates. Oversized requests are refused, rather than
            sent with incomplete metadata.
          </dd>
        </div>
      </dl>
      <div class="ai-selection-preview">
        <strong>Selected on this device</strong>
        <ul>
          {#each selection as entry}<li>
              <span>{entry.relativePath}</span><span
                >{bytes(entry.logicalBytes)}</span
              >
            </li>{/each}
        </ul>
        <p class="muted">
          These displayed paths are for your review; the provider receives
          redacted paths.
        </p>
      </div>
    </div>
    <footer class="modal-actions">
      <button onclick={() => (aiConfirm = false)}>Cancel</button><button
        class="primary"
        onclick={analyze}>Send metadata for analysis</button
      >
    </footer></Modal
  >{/if}
{#if removeId !== null}<Modal
    title="Remove this scan from history?"
    onclose={() => (removeId = null)}
    ><div class="modal-body">
      <p>
        This removes the local scan index and its recommendations. Files in the
        scanned folder are unchanged.
      </p>
    </div>
    <footer class="modal-actions">
      <button onclick={() => (removeId = null)}>Cancel</button><button
        onclick={removeScan}>Remove scan history</button
      >
    </footer></Modal
  >{/if}
