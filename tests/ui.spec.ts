import { test, expect, type Page } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const mockSource = readFileSync(
  require.resolve('@tauri-apps/api/mocks').replace(/\.cjs$/, '.js'),
  'utf8',
).replace(/export \{[^}]*\};\s*$/, '');

async function setup(
  page: Page,
  options: {
    empty?: boolean;
    scanError?: boolean;
    providerError?: boolean;
    finishBeforeGetScan?: boolean;
  } = {},
) {
  await page.addInitScript({
    content: `${mockSource}\n(${initialize.toString()})(${JSON.stringify(options)});`,
  });
  await page.goto('/');
}

function initialize(options: {
  empty?: boolean;
  scanError?: boolean;
  providerError?: boolean;
  finishBeforeGetScan?: boolean;
}) {
  const runtime = window as unknown as {
    __calls: { command: string; args: Record<string, unknown> }[];
    __emit: (event: string, payload: unknown) => Promise<unknown>;
    __TAURI_INTERNALS__: {
      invoke: (
        command: string,
        args: Record<string, unknown>,
      ) => Promise<unknown>;
    };
  };
  runtime.__calls = [];
  const entry = (
    id: number,
    name: string,
    size: number,
    extra: Record<string, unknown> = {},
  ) => ({
    id,
    parentId: 1,
    name,
    relativePath: name,
    kind: 'file',
    logicalBytes: size,
    allocatedBytes: size,
    modifiedAt: 1760000000,
    accessedAt: null,
    complete: true,
    error: null,
    category: 'unknown',
    disposition: 'review',
    reason: 'Purpose is unknown.',
    consequence: 'Review before removing.',
    uncertainty: 'Usage unknown.',
    project: null,
    evidence: [],
    removable: false,
    ...extra,
  });
  const root = entry(1, 'TrapClean fixture', 2_000_000_000, {
    kind: 'directory',
    parentId: null,
    relativePath: '',
  });
  const project = entry(2, 'sample-project', 1_200_000_000, {
    kind: 'directory',
    category: 'project',
    disposition: 'keep',
    removable: false,
  });
  const target = entry(3, 'target', 900_000_000, {
    parentId: 2,
    relativePath: 'sample-project/target',
    kind: 'directory',
    category: 'cargo_build',
    disposition: 'candidate',
    removable: true,
    reason: 'Cargo build artifacts with sources preserved.',
    consequence: 'The next build recompiles artifacts.',
    uncertainty: 'Active processes and future use are unknown.',
    project: 'sample-project',
    evidence: [
      'Cargo.toml and source files found',
      'Directory inspection completed',
    ],
  });
  const modules = entry(4, 'node_modules', 300_000_000, {
    parentId: 2,
    relativePath: 'sample-project/node_modules',
    kind: 'directory',
    category: 'node_dependencies',
    disposition: 'review',
    removable: true,
    reason: 'Manifest and lockfile found.',
    consequence: 'Dependencies need reinstalling.',
    uncertainty: 'Local dependency edits and network availability are unknown.',
    project: 'sample-project',
    evidence: ['package.json and lockfile found'],
  });
  const files = Array.from({ length: 124 }, (_, index) =>
    entry(
      100 + index,
      `record-${String(index).padStart(3, '0')}.bin`,
      (125 - index) * 100_000,
    ),
  );
  const duplicateA = entry(500, 'copy-a.bin', 2048, {
    relativePath: 'copy-a.bin',
    removable: true,
    category: 'verified_duplicate',
  });
  const duplicateB = entry(501, 'copy-b.bin', 2048, {
    relativePath: 'copy-b.bin',
    removable: true,
    category: 'verified_duplicate',
  });
  const all = [
    root,
    project,
    target,
    modules,
    ...files,
    duplicateA,
    duplicateB,
  ];
  const children = [project, ...files];
  const scan = {
    id: 1,
    root: '/tmp/TrapClean fixture',
    rootId: 1,
    status: 'complete',
    entries: 129,
    logicalBytes: 2_000_000_000,
    allocatedBytes: 1_980_000_000,
    allocatedEstimated: true,
    candidateBytes: 1_200_000_000,
    errors: 0,
    skipped: 1,
    startedAt: 1760000000,
    finishedAt: 1760000010,
  };
  let scans = options.empty ? [] : [scan];
  let provider = {
    mode: 'disabled',
    model: '',
    endpoint: '',
    keyStatus: 'missing',
    status: 'AI disabled.',
  };
  let planIds: number[] = [];
  let planMode = 'trash';
  // This official mock API is injected only into the test page before application startup.
  // @ts-expect-error mockIPC is supplied by the injected @tauri-apps/api/mocks source.
  mockIPC(
    async (command: string, args: Record<string, any> = {}) => {
      runtime.__calls.push({ command, args });
      switch (command) {
        case 'list_scans':
          return scans;
        case 'get_provider_settings':
          return provider;
        case 'record_frontend_ready':
          return null;
        case 'get_scan':
          if (options.finishBeforeGetScan && args.scanId === 10)
            return {
              ...scan,
              id: 10,
              status: 'scanning',
              entries: 1,
              logicalBytes: 0,
              allocatedBytes: 0,
              candidateBytes: 0,
              finishedAt: null,
            };
          return scans.find((value) => value.id === args.scanId) || scan;
        case 'choose_root':
          return '/tmp/TrapClean fixture';
        case 'start_scan':
          if (options.scanError)
            throw new Error(
              'Access denied: the selected folder cannot be read.',
            );
          scans = [
            {
              ...scan,
              id: 10,
              status: 'scanning',
              entries: 0,
              logicalBytes: 0,
            },
          ];
          if (options.finishBeforeGetScan) {
            scans = [{ ...scan, id: 10, skipped: 0 }];
            await runtime.__TAURI_INTERNALS__.invoke('plugin:event|emit', {
              event: 'scan-finished',
              payload: scans[0],
            });
          }
          return 10;
        case 'get_entries': {
          let values = args.candidatesOnly
            ? [target, modules]
            : args.parentId === 2
              ? [target, modules]
              : children;
          if (args.search)
            values = values.filter((value) => value.name.includes(args.search));
          values = [...values].sort(
            args.sort === 'name'
              ? (a, b) => a.name.localeCompare(b.name)
              : (a, b) => b.logicalBytes - a.logicalBytes,
          );
          return {
            items: values.slice(args.offset, args.offset + args.limit),
            total: values.length,
            totalBytes: values.reduce(
              (sum, value) => sum + value.logicalBytes,
              0,
            ),
            offset: args.offset,
          };
        }
        case 'get_entry':
          return all.find((value) => value.id === args.entryId);
        case 'get_breadcrumbs':
          return args.entryId === 1 ? [root] : [root, project];
        case 'cancel_operation':
          return null;
        case 'find_duplicates':
          return [
            {
              hash: 'fixture-full-content-hash',
              bytesEach: 2048,
              items: [duplicateA, duplicateB],
            },
          ];
        case 'create_plan': {
          planIds = args.entryIds;
          planMode = args.mode;
          const items = all
            .filter((value) => planIds.includes(value.id))
            .map((value) => ({
              id: value.id,
              relativePath: value.relativePath,
              logicalBytes: value.logicalBytes,
              reason: value.reason,
              consequence: value.consequence,
            }));
          return {
            token: 'one-use-test-token',
            scanId: scan.id,
            mode: planMode,
            items,
            estimatedBytes: items.reduce(
              (sum, value) => sum + value.logicalBytes,
              0,
            ),
            expiresAt: 2000000000,
          };
        }
        case 'execute_plan': {
          if (
            args.token !== 'one-use-test-token' ||
            args.confirmation !==
              (planMode === 'permanent'
                ? 'DELETE PERMANENTLY'
                : 'MOVE TO TRASH')
          )
            throw new Error('Unreviewed plan refused.');
          const values = all.filter((value) => planIds.includes(value.id));
          return {
            items: values.map((value) => ({
              id: value.id,
              relativePath: value.relativePath,
              status: 'success',
              message:
                planMode === 'trash'
                  ? 'Moved to system trash.'
                  : 'Permanently deleted.',
              bytes: value.logicalBytes,
            })),
            movedBytes: planMode === 'trash' ? 900_000_000 : 0,
            deletedBytes: planMode === 'permanent' ? 900_000_000 : 0,
            freeSpaceChange: planMode === 'trash' ? 0 : 900_000_000,
            interrupted: false,
          };
        }
        case 'save_provider_settings':
          provider = {
            ...args.settings,
            keyStatus: args.key
              ? args.sessionOnly
                ? 'session'
                : 'saved'
              : 'missing',
            status: 'Settings saved.',
          };
          return provider;
        case 'remove_provider_key':
          provider = { ...provider, keyStatus: 'missing' };
          return provider;
        case 'test_provider':
          return options.providerError
            ? {
                status: 'auth',
                message: 'Authentication failed. Check your API key.',
                decisions: [],
                cached: false,
              }
            : {
                status: 'ok',
                message: 'Connection and decision contract verified.',
                decisions: [],
                cached: false,
              };
        case 'analyze_candidates':
          return {
            status: 'timeout',
            message: 'Provider timed out. Local recommendations retained.',
            decisions: [],
            cached: false,
          };
        case 'remove_scan':
          scans = scans.filter((value) => value.id !== args.scanId);
          return null;
        default:
          throw new Error(`Unexpected command in test: ${command}`);
      }
    },
    { shouldMockEvents: true },
  );
  runtime.__emit = (event, payload) =>
    runtime.__TAURI_INTERNALS__.invoke('plugin:event|emit', { event, payload });
}

async function calls(page: Page, command: string) {
  return page.evaluate(
    (name) =>
      (
        window as unknown as {
          __calls: { command: string; args: Record<string, unknown> }[];
        }
      ).__calls.filter((call) => call.command === name),
    command,
  );
}

test('storage has a bounded, sortable tree and truthful map; files expose evidence and dates', async ({
  page,
}) => {
  await setup(page);
  await expect(
    page.getByRole('heading', { name: 'Storage', exact: true }),
  ).toBeVisible();
  await expect(page.getByText('1–100 of 125', { exact: true })).toBeVisible();
  expect(await page.locator('tbody tr').count()).toBe(100);
  await page.screenshot({ path: 'test-results/storage-mock.png' });
  await expect(page.getByRole('button', { name: /^Other,/ })).toBeVisible();
  await page.getByRole('button', { name: 'Next page' }).click();
  await expect(page.getByText('101–125 of 125', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Previous page' }).click();
  await page
    .getByRole('textbox', { name: 'Search this folder' })
    .fill('record-023');
  await expect(
    page.getByRole('button', { name: 'record-023.bin', exact: true }),
  ).toBeVisible();
  await expect(page.locator('tbody tr')).toHaveCount(1);
  await page.getByRole('button', { name: 'Clear search' }).click();
  await page
    .getByRole('columnheader')
    .getByRole('button', { name: 'Name', exact: true })
    .click();
  await expect(
    page
      .locator('tbody tr')
      .first()
      .getByRole('button', { name: 'record-000.bin', exact: true }),
  ).toBeVisible();
  await page
    .getByRole('columnheader')
    .getByRole('button', { name: 'Size', exact: true })
    .click();
  await page
    .getByRole('button', { name: 'sample-project', exact: true })
    .click();
  await expect(
    page
      .getByRole('navigation', { name: 'Folder breadcrumbs' })
      .getByRole('button', { name: 'sample-project' }),
  ).toBeVisible();
  await page
    .getByRole('button', { name: 'Inspect target', exact: true })
    .click();
  await expect(
    page.getByRole('heading', { name: 'Observed evidence' }),
  ).toBeVisible();
  await expect(
    page.getByText('Cargo.toml and source files found'),
  ).toBeVisible();
  await expect(
    page.getByText('Access time reported by filesystem', { exact: true }),
  ).toBeVisible();
  await expect(
    page.locator('.details-panel').getByText('Not reported', { exact: true }),
  ).toBeVisible();
  await expect(
    page.locator('.details-panel').getByText('Last used', { exact: true }),
  ).toHaveCount(0);
  const queries = await calls(page, 'get_entries');
  expect(queries.every((query) => Number(query.args.limit) <= 100)).toBe(true);
});

test('cleanup requires a reviewed one-use plan and exact typed confirmation', async ({
  page,
}) => {
  await setup(page);
  await page.getByRole('button', { name: 'Cleanup', exact: true }).click();
  await expect(
    page.getByRole('button', { name: 'Preview cleanup', exact: true }),
  ).toBeDisabled();
  await page
    .getByRole('checkbox', { name: 'Select target for cleanup' })
    .check();
  await page
    .getByRole('button', { name: 'Preview cleanup', exact: true })
    .click();
  const modal = page.getByRole('dialog', { name: 'Review move to trash' });
  await expect(modal).toBeVisible();
  await expect(
    modal.getByText(/usually does not free disk space/),
  ).toBeVisible();
  await expect(
    modal.getByRole('button', { name: 'Move selected items to trash' }),
  ).toBeDisabled();
  expect(await calls(page, 'execute_plan')).toHaveLength(0);
  await modal.getByRole('textbox').fill('move to trash');
  await expect(
    modal.getByRole('button', { name: 'Move selected items to trash' }),
  ).toBeDisabled();
  await modal.getByRole('textbox').fill('MOVE TO TRASH');
  await modal
    .getByRole('button', { name: 'Move selected items to trash' })
    .click();
  await expect(
    page.getByRole('heading', { name: 'Cleanup finished' }),
  ).toBeVisible();
  await expect(
    page.getByText('Moved to system trash.', { exact: true }),
  ).toBeVisible();
  const executed = await calls(page, 'execute_plan');
  expect(executed).toHaveLength(1);
  expect(executed[0].args).toEqual({
    token: 'one-use-test-token',
    confirmation: 'MOVE TO TRASH',
  });
  expect(executed[0].args).not.toHaveProperty('paths');
});

test('permanent deletion is a distinct action and duplicate discovery never autoselects', async ({
  page,
}) => {
  await setup(page);
  await page.getByRole('button', { name: 'Cleanup', exact: true }).click();
  await page.getByRole('button', { name: 'Find duplicates' }).click();
  await expect(
    page.getByRole('checkbox', { name: 'Select duplicate copy-a.bin' }),
  ).not.toBeChecked();
  await expect(
    page.getByRole('checkbox', { name: 'Select duplicate copy-b.bin' }),
  ).not.toBeChecked();
  await expect(
    page.getByRole('button', { name: 'Preview cleanup', exact: true }),
  ).toBeDisabled();
  await page
    .getByRole('checkbox', { name: 'Select target for cleanup' })
    .check();
  await page
    .getByRole('combobox', { name: 'Cleanup action' })
    .selectOption('permanent');
  await page.getByRole('button', { name: 'Preview cleanup' }).click();
  const modal = page.getByRole('dialog', { name: 'Review permanent deletion' });
  await expect(
    modal.getByText(/Permanent deletion cannot be undone/),
  ).toBeVisible();
  await modal.getByRole('textbox').fill('MOVE TO TRASH');
  await expect(
    modal.getByRole('button', { name: 'Delete selected items permanently' }),
  ).toBeDisabled();
  await modal.getByRole('textbox').fill('DELETE PERMANENTLY');
  await modal
    .getByRole('button', { name: 'Delete selected items permanently' })
    .click();
  await expect(
    page.getByText('Permanently deleted.', { exact: true }),
  ).toBeVisible();
});

test('provider changes send no requests until explicit paid test confirmation; key/session/remove work', async ({
  page,
}) => {
  await setup(page, { providerError: true });
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('radio', { name: /OpenAI Decisions API/ }).check();
  await page
    .getByRole('textbox', { name: /^API key/ })
    .fill('fixture-not-a-real-key');
  await page
    .getByRole('checkbox', { name: /Keep a new key in memory/ })
    .check();
  expect(await calls(page, 'test_provider')).toHaveLength(0);
  await page.getByRole('button', { name: 'Save settings' }).click();
  await expect(
    page.getByText('Key held in this session', { exact: true }),
  ).toBeVisible();
  await page.getByRole('button', { name: 'Test connection' }).click();
  await expect(
    page.getByRole('dialog').getByText(/may consume provider credits/),
  ).toBeVisible();
  expect(await calls(page, 'test_provider')).toHaveLength(0);
  await page.getByRole('button', { name: 'Send test request' }).click();
  await expect(
    page.getByText('Authentication failed. Check your API key.', {
      exact: true,
    }),
  ).toBeVisible();
  expect(await calls(page, 'test_provider')).toHaveLength(1);
  await page.getByRole('button', { name: 'Remove key', exact: true }).click();
  await expect(page.getByText('No saved key', { exact: true })).toBeVisible();
  await page.getByRole('radio', { name: /JEV \/ TypeSafe/ }).check();
  await expect(
    page.getByRole('textbox', { name: 'Model or alias' }),
  ).toHaveValue('jev-latest');
  await page
    .getByRole('radio', { name: /Custom Decisions-compatible/ })
    .check();
  await page
    .getByRole('textbox', { name: 'Model or alias' })
    .fill('custom-test-model');
  await page
    .getByRole('textbox', { name: 'Endpoint URL' })
    .fill('http://127.0.0.1:4321/v1/decisions');
  await page.getByRole('button', { name: 'Save settings' }).click();
  expect(
    (await calls(page, 'save_provider_settings')).at(-1)?.args.settings,
  ).toMatchObject({
    mode: 'custom',
    endpoint: 'http://127.0.0.1:4321/v1/decisions',
  });
});

test('AI metadata preview is explicit and provider failure leaves local selections intact', async ({
  page,
}) => {
  await setup(page);
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page
    .getByRole('radio', { name: /Custom Decisions-compatible/ })
    .check();
  await page
    .getByRole('textbox', { name: 'Model or alias' })
    .fill('custom-test-model');
  await page
    .getByRole('textbox', { name: 'Endpoint URL' })
    .fill('http://127.0.0.1:4321/v1/decisions');
  await page.getByRole('button', { name: 'Save settings' }).click();
  await page.getByRole('button', { name: 'Cleanup', exact: true }).click();
  await page
    .getByRole('checkbox', { name: 'Select target for cleanup' })
    .check();
  await page
    .getByRole('button', { name: 'Analyze with AI', exact: true })
    .click();
  await expect(
    page.getByRole('dialog', { name: 'Review what AI receives' }),
  ).toBeVisible();
  await expect(
    page
      .getByRole('dialog')
      .getByText(/File and manifest contents, absolute paths/),
  ).toBeVisible();
  expect(await calls(page, 'analyze_candidates')).toHaveLength(0);
  await page
    .getByRole('button', { name: 'Send metadata for analysis' })
    .click();
  await expect(
    page.getByText('Provider timed out. Local recommendations retained.', {
      exact: true,
    }),
  ).toBeVisible();
  await expect(
    page.getByRole('checkbox', { name: 'Select target for cleanup' }),
  ).toBeChecked();
  expect((await calls(page, 'analyze_candidates'))[0].args).toEqual({
    scanId: 1,
    entryIds: [3],
  });
});

test('scan events aggregate progress, cancellation is available, and access errors are visible', async ({
  page,
}) => {
  await setup(page, { empty: true });
  await page.getByRole('button', { name: 'Choose a folder to scan' }).click();
  await expect(
    page.getByText('Scanning your folder', { exact: true }),
  ).toBeVisible();
  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  expect(await calls(page, 'cancel_operation')).toHaveLength(1);
  await page.evaluate(() =>
    (
      window as unknown as {
        __emit: (name: string, data: unknown) => Promise<unknown>;
      }
    ).__emit('scan-finished', {
      id: 10,
      root: '/tmp/TrapClean fixture',
      rootId: 1,
      status: 'cancelled',
      entries: 99,
      logicalBytes: 100000,
      allocatedBytes: 100000,
      allocatedEstimated: false,
      candidateBytes: 0,
      errors: 0,
      skipped: 0,
      startedAt: 1760000000,
      finishedAt: 1760000010,
    }),
  );
  await expect(page.getByText('Stopped · incomplete').first()).toBeVisible();
  await setup(page, { empty: true, scanError: true });
  await page.getByRole('button', { name: 'Choose a folder to scan' }).click();
  await expect(
    page.getByRole('alert').getByText(/Access denied/),
  ).toBeVisible();
});

test('keyboard modal focus, Escape restore, narrow 200% layout and light/dark remain usable', async ({
  page,
}) => {
  await setup(page);
  await page.getByRole('button', { name: 'Cleanup', exact: true }).click();
  await page
    .getByRole('checkbox', { name: 'Select target for cleanup' })
    .check();
  const preview = page.getByRole('button', {
    name: 'Preview cleanup',
    exact: true,
  });
  await preview.focus();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('dialog')).toBeVisible();
  expect(
    await page.evaluate(() => !!document.activeElement?.closest('dialog')),
  ).toBe(true);
  for (let index = 0; index < 8; index++) {
    await page.keyboard.press('Tab');
    expect(
      await page.evaluate(() => !!document.activeElement?.closest('dialog')),
    ).toBe(true);
  }
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog')).not.toBeVisible();
  await expect(preview).toBeFocused();
  await page.setViewportSize({ width: 640, height: 450 });
  await page.emulateMedia({ colorScheme: 'dark', reducedMotion: 'reduce' });
  await expect(
    page.getByRole('button', { name: 'Preview cleanup' }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  await page.emulateMedia({ colorScheme: 'light' });
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(
    page.getByRole('radio', { name: /Disabled Local rules/ }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
});

test('plain browser has no mock filesystem or fake production data', async ({
  page,
}) => {
  await page.goto('/');
  await expect(
    page.getByText('Open the installed desktop app to access your filesystem.'),
  ).toBeVisible();
  await expect(
    page.getByRole('button', { name: 'Choose a folder to scan' }),
  ).toBeDisabled();
  await expect(
    page.getByText('sample-project', { exact: true }),
  ).not.toBeVisible();
});

test('history removal changes only the scan index and stale coverage never claims completion', async ({
  page,
}) => {
  await setup(page);
  await page.evaluate(() =>
    (
      window as unknown as {
        __emit: (name: string, data: unknown) => Promise<unknown>;
      }
    ).__emit('scan-finished', {
      id: 1,
      root: '/tmp/TrapClean fixture',
      rootId: 1,
      status: 'stale',
      entries: 129,
      logicalBytes: 2000000000,
      allocatedBytes: 1980000000,
      allocatedEstimated: true,
      candidateBytes: 0,
      errors: 0,
      skipped: 0,
      startedAt: 1760000000,
      finishedAt: 1760000010,
    }),
  );
  await expect(page.getByText('Changed · rescan needed').first()).toBeVisible();
  await page.locator('summary', { hasText: 'Recent scans' }).click();
  await page
    .getByRole('button', {
      name: 'Remove scan of TrapClean fixture from history',
    })
    .click();
  await expect(
    page.getByRole('dialog', { name: 'Remove this scan from history?' }),
  ).toBeVisible();
  await page
    .getByRole('button', { name: 'Remove scan history', exact: true })
    .click();
  await expect(
    page.getByRole('heading', { name: 'Know what’s taking up space.' }),
  ).toBeVisible();
  expect(await calls(page, 'remove_scan')).toHaveLength(1);
  expect(await calls(page, 'execute_plan')).toHaveLength(0);
});

test('an immediate scan-finished event cannot be replaced by a stale scanning response', async ({
  page,
}) => {
  await setup(page, { empty: true, finishBeforeGetScan: true });
  await page.getByRole('button', { name: 'Choose a folder to scan' }).click();
  await expect(
    page.getByText('Completed', { exact: true }).first(),
  ).toBeVisible();
  await expect(
    page.getByText('129 entries indexed', { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText('Scanning your folder', { exact: true }),
  ).toHaveCount(0);
  await page.evaluate(() =>
    (
      window as unknown as {
        __emit: (name: string, data: unknown) => Promise<unknown>;
      }
    ).__emit('scan-progress', {
      id: 10,
      root: '/tmp/TrapClean fixture',
      rootId: 1,
      status: 'scanning',
      entries: 1,
      logicalBytes: 0,
      allocatedBytes: 0,
      allocatedEstimated: false,
      candidateBytes: 0,
      errors: 0,
      skipped: 0,
      startedAt: 1760000000,
      finishedAt: null,
    }),
  );
  await expect(
    page.getByText('Completed', { exact: true }).first(),
  ).toBeVisible();
  await expect(
    page.getByText('129 entries indexed', { exact: true }),
  ).toBeVisible();
});

test('a desktop-height window shows the summary, compact map and first three tree rows together', async ({
  page,
}) => {
  await page.setViewportSize({ width: 1180, height: 758 });
  await setup(page);
  await expect(page.locator('.metrics')).toBeInViewport({ ratio: 1 });
  await expect(page.locator('.treemap')).toBeInViewport({ ratio: 1 });
  await expect(page.locator('tbody tr').nth(2)).toBeInViewport({ ratio: 0.95 });
  await page.screenshot({ path: 'test-results/desktop-density-mock.png' });
});

test('startup readiness is acknowledged once locally without automatic paid requests', async ({
  page,
}) => {
  await setup(page);
  await expect
    .poll(async () => (await calls(page, 'record_frontend_ready')).length)
    .toBe(1);
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('button', { name: 'Cleanup', exact: true }).click();
  await page.getByRole('button', { name: 'Storage', exact: true }).click();
  expect(await calls(page, 'record_frontend_ready')).toHaveLength(1);
  expect(await calls(page, 'test_provider')).toHaveLength(0);
  expect(await calls(page, 'analyze_candidates')).toHaveLength(0);
});

test('provider session-key checkbox keeps its complete caption inside the form at desktop and scaled widths', async ({
  page,
}) => {
  await setup(page);
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('radio', { name: /OpenAI Decisions API/ }).check();
  const label = page.locator('.check-label');
  const caption = label.locator('span');
  for (const viewport of [
    { width: 1180, height: 758 },
    { width: 640, height: 450 },
    { width: 393, height: 700 },
  ]) {
    await page.setViewportSize(viewport);
    await caption.scrollIntoViewIfNeeded();
    await expect(caption).toBeInViewport({ ratio: 1 });
    const panelBox = await page.locator('.provider-panel').boundingBox();
    const labelBox = await label.boundingBox();
    const captionBox = await caption.boundingBox();
    const checkboxBox = await label.getByRole('checkbox').boundingBox();
    expect(panelBox && labelBox && captionBox && checkboxBox).toBeTruthy();
    expect(captionBox!.width).toBeGreaterThan(labelBox!.width * 0.7);
    expect(captionBox!.x).toBeGreaterThan(checkboxBox!.x + checkboxBox!.width);
    expect(captionBox!.x + captionBox!.width).toBeLessThanOrEqual(
      labelBox!.x + labelBox!.width + 1,
    );
    expect(labelBox!.x + labelBox!.width).toBeLessThanOrEqual(
      panelBox!.x + panelBox!.width,
    );
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth,
      ),
    ).toBe(true);
    if (viewport.width === 1180)
      await page.screenshot({
        path: 'test-results/provider-checkbox-layout-mock.png',
      });
  }
});
