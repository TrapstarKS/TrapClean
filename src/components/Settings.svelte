<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import Icon from './Icon.svelte';
  import Modal from './Modal.svelte';
  import type {
    ProviderMode,
    ProviderReport,
    ProviderSettings,
  } from '../types';
  import { message } from '../utils';
  let {
    settings,
    onsaved,
    available,
    locked = false,
  }: {
    settings: ProviderSettings;
    onsaved: (settings: ProviderSettings) => void;
    available: boolean;
    locked?: boolean;
  } = $props();
  let mode = $state<ProviderMode>('disabled');
  let model = $state('');
  let endpoint = $state('');
  let key = $state('');
  let sessionOnly = $state(false);
  let busy = $state(false);
  let testing = $state(false);
  let error = $state('');
  let notice = $state('');
  let testConfirm = $state(false);
  let report = $state<ProviderReport | null>(null);
  $effect(() => {
    mode = settings.mode;
    model = settings.model;
    endpoint = settings.endpoint;
  });
  const dirty = $derived(
    mode !== settings.mode ||
      model !== settings.model ||
      endpoint !== settings.endpoint ||
      !!key,
  );
  const providers: {
    mode: ProviderMode;
    title: string;
    description: string;
  }[] = [
    {
      mode: 'disabled',
      title: 'Disabled',
      description: 'Local rules · no account',
    },
    { mode: 'openai', title: 'OpenAI', description: 'Decisions API' },
    {
      mode: 'typesafe',
      title: 'JEV / TypeSafe',
      description: 'System One API',
    },
    {
      mode: 'custom',
      title: 'Custom',
      description: 'Decisions-compatible endpoint',
    },
  ];
  function choose(value: ProviderMode) {
    mode = value;
    key = '';
    error = '';
    notice = '';
    report = null;
    if (value === settings.mode) {
      model = settings.model;
      endpoint = settings.endpoint;
      return;
    }
    model =
      value === 'openai'
        ? 'gpt-6-luna'
        : value === 'typesafe'
          ? 'jev-latest'
          : '';
    endpoint =
      value === 'openai'
        ? 'https://api.openai.com/v1/decisions'
        : value === 'typesafe'
          ? 'https://api.typesafe.ai/v1/systemone'
          : '';
  }
  async function save(event: SubmitEvent) {
    event.preventDefault();
    if (busy || locked) return;
    busy = true;
    error = '';
    notice = '';
    try {
      const value = await invoke<ProviderSettings>('save_provider_settings', {
        settings: {
          mode,
          model: model.trim(),
          endpoint: endpoint.trim(),
          keyStatus: 'missing',
          status: '',
        },
        key: key.trim() || null,
        sessionOnly,
      });
      key = '';
      onsaved(value);
      notice =
        mode === 'disabled'
          ? 'AI is disabled. Local recommendations stay available.'
          : 'Provider settings saved. No request was sent.';
    } catch (cause) {
      error = message(cause);
    } finally {
      busy = false;
    }
  }
  async function removeKey() {
    if (busy || locked) return;
    busy = true;
    error = '';
    try {
      const value = await invoke<ProviderSettings>('remove_provider_key', {
        mode,
      });
      onsaved(value);
      key = '';
      notice = 'API key removed.';
    } catch (cause) {
      error = message(cause);
    } finally {
      busy = false;
    }
  }
  async function test() {
    if (busy || locked) return;
    testConfirm = false;
    testing = true;
    busy = true;
    error = '';
    report = null;
    try {
      report = await invoke<ProviderReport>('test_provider');
    } catch (cause) {
      error = message(cause);
    } finally {
      busy = false;
      testing = false;
    }
  }
  async function cancelTest() {
    try {
      await invoke('cancel_operation');
    } catch (cause) {
      error = message(cause);
    }
  }
</script>

<div class="settings-layout">
  <section class="panel provider-panel" aria-labelledby="provider-heading">
    <div class="section-heading">
      <div>
        <span class="eyebrow">OPTIONAL INTELLIGENCE</span>
        <h2 id="provider-heading">Decision provider</h2>
        <p>Storage analysis and local recommendations work offline.</p>
      </div>
      <span class="badge"><Icon name="shield" size={14} /> Your choice</span>
    </div>
    <fieldset class="provider-options">
      <legend class="sr-only">Decision provider</legend
      >{#each providers as provider}<label class:chosen={mode === provider.mode}
          ><input
            type="radio"
            disabled={busy || locked}
            name="provider"
            value={provider.mode}
            checked={mode === provider.mode}
            onchange={() => choose(provider.mode)}
          /><strong>{provider.title}</strong><small
            >{provider.description}</small
          ></label
        >{/each}
    </fieldset>
    <form onsubmit={save}>
      {#if mode !== 'disabled'}
        <div class="form-grid">
          <label
            >Model or alias<input
              required
              value={model}
              oninput={(event) => (model = event.currentTarget.value)}
              placeholder={mode === 'typesafe' ? 'jev-latest' : 'gpt-6-luna'}
              spellcheck="false"
              autocomplete="off"
            /><small
              >{mode === 'typesafe'
                ? 'Use an alias available to your TypeSafe account.'
                : 'Use a model supported by your provider.'}</small
            ></label
          >
          <label
            >Endpoint URL{#if mode === 'custom'}<input
                required
                type="url"
                bind:value={endpoint}
                placeholder="https://example.com/v1/decisions"
                spellcheck="false"
                autocomplete="off"
              />{:else}<input value={endpoint} readonly />{/if}<small
              >{mode === 'custom'
                ? 'Full endpoint, including /v1/decisions. HTTPS, or HTTP on loopback only.'
                : 'Official provider endpoint.'}</small
            ></label
          >
        </div>
        {#if mode === 'custom'}<div class="inline-note">
            <Icon name="info" />
            <p>
              Your endpoint must support the textual OpenAI Decisions contract.
              A chat-compatible endpoint is insufficient.
            </p>
          </div>{/if}
        <label class="key-field"
          >API key {#if mode === 'custom'}<span class="muted">(optional)</span
            >{/if}
          <div class="key-input">
            <Icon name="key" /><input
              type="password"
              bind:value={key}
              placeholder={mode === settings.mode &&
              settings.keyStatus !== 'missing'
                ? 'Key already stored · enter a replacement'
                : 'Enter your API key'}
              autocomplete="new-password"
              spellcheck="false"
            />
          </div>
          <small
            >Stored in the macOS Keychain or Windows Credential Manager. Saved
            keys are never returned to this screen.</small
          ></label
        >
        <label class="check-label"
          ><input type="checkbox" bind:checked={sessionOnly} /><span
            >Keep a new key in memory for this session only<small
              >Use this if your system vault is unavailable. The key is lost
              when you quit.</small
            ></span
          ></label
        >
        <div class="key-status">
          <span
            class="status-dot"
            class:success={mode === settings.mode &&
              settings.keyStatus !== 'missing'}
          ></span><span
            >{mode === settings.mode
              ? settings.keyStatus === 'saved'
                ? 'Key saved in the system vault'
                : settings.keyStatus === 'session'
                  ? 'Key held in this session'
                  : 'No saved key'
              : 'Save this provider to check its key status'}</span
          >{#if mode === settings.mode && settings.keyStatus !== 'missing'}<button
              type="button"
              class="text-button"
              disabled={busy || locked}
              onclick={removeKey}>Remove key</button
            >{/if}
        </div>
        {#if mode === settings.mode && settings.status}<p class="muted">
            {settings.status}
          </p>{/if}
      {:else}
        <div class="offline-state">
          <span class="offline-icon"><Icon name="shield" size={28} /></span>
          <div>
            <h3>Useful from the first scan</h3>
            <p>
              TrapClean finds large folders, recognized builds and caches using
              local evidence. Enable a provider only when you want a second
              opinion.
            </p>
          </div>
        </div>
      {/if}
      {#if error}<div class="alert error" role="alert">
          <Icon name="warning" />
          <p>{error}</p>
        </div>{/if}
      {#if notice}<div class="alert success" role="status">
          <Icon name="check" />
          <p>{notice}</p>
        </div>{/if}
      {#if report}<div
          class="alert"
          class:error={report.status !== 'ok' && report.status !== 'success'}
          role="status"
        >
          <Icon
            name={report.status === 'ok' || report.status === 'success'
              ? 'check'
              : 'warning'}
          />
          <div>
            <strong
              >{report.status === 'ok' || report.status === 'success'
                ? 'Connection tested'
                : 'Provider status'}</strong
            >
            <p>{report.message}</p>
          </div>
        </div>{/if}
      <div class="form-actions">
        {#if testing}<button type="button" onclick={cancelTest}
            >Cancel test</button
          >{/if}
        <button
          class="primary"
          disabled={busy || locked || !available}
          type="submit">{busy ? 'Working…' : 'Save settings'}</button
        >{#if mode !== 'disabled'}<button
            type="button"
            disabled={busy || locked || dirty || !available}
            onclick={() => (testConfirm = true)}>Test connection</button
          >{/if}{#if dirty && mode !== 'disabled'}<span class="muted"
            >Save changes before testing.</span
          >{/if}
      </div>
    </form>
  </section>
  <aside class="settings-explainer">
    <div class="section-heading">
      <Icon name="spark" />
      <h3>Advice, with boundaries</h3>
    </div>
    <p>
      AI runs only when you explicitly request it. Tests and analyses may
      consume provider credits.
    </p>
    <div class="explainer-rule">
      <strong>You review what leaves the device</strong>
      <p>
        At most 20 selected candidates per request. Only limited, redacted
        metadata is sent. File contents and absolute paths stay on your device.
      </p>
    </div>
    <div class="explainer-rule">
      <strong>You decide what is removed</strong>
      <p>
        A provider can suggest a recommendation. It cannot select files, bypass
        local protections, or authorize cleanup.
      </p>
    </div>
    <div class="explainer-rule">
      <strong>Local rules remain available</strong>
      <p>
        If a provider is offline, refuses, or returns an invalid result,
        TrapClean keeps the local recommendation and shows the reason.
      </p>
    </div>
  </aside>
</div>
{#if testConfirm}<Modal
    title="Test the saved provider?"
    onclose={() => (testConfirm = false)}
    ><div class="modal-body">
      <div class="inline-note">
        <Icon name="spark" />
        <p>
          This sends a small, synthetic request to {providers.find(
            (provider) => provider.mode === settings.mode,
          )?.title}. It may consume provider credits. No scanned file metadata
          is included.
        </p>
      </div>
    </div>
    <footer class="modal-actions">
      <button onclick={() => (testConfirm = false)}>Cancel</button><button
        class="primary"
        onclick={test}>Send test request</button
      >
    </footer></Modal
  >{/if}
