<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { _ } from 'svelte-i18n';
  import { settings } from '../stores/settings';
  import { scheduler } from '../stores/scheduler';
  import SchedulerControl from './SchedulerControl.svelte';

  function openUrl(url: string) {
    invoke('cmd_open_url', { url }).catch((e) => console.error('open_url', e));
  }

  interface SpeedResult {
    download_mbps: number;
    upload_mbps: number;
    latency_ms: number;
    jitter_ms: number | null;
    server_name: string;
    result_url: string | null;
    timestamp: number;
  }

  let running = $state(false);
  let current = $state<SpeedResult | null>(null);
  let history = $state<SpeedResult[]>([]);
  let error = $state('');

  // Hydrate + écoute des events du backend : le speedtest partage son
  // dernier résultat entre desktop et clients web via `speedtest:result`.
  // Un test lancé depuis le desktop s'affiche en direct sur le web
  // (progress → result final) et réciproquement.
  let cancelling = $state(false);

  onMount(() => {
    scheduler.init();
    (async () => {
      try {
        const snap = await invoke<{ latest: SpeedResult | null; running: boolean }>('cmd_get_speedtest_snapshot');
        if (snap?.latest) {
          current = snap.latest;
          if (history.length === 0) history = [snap.latest];
        }
        if (snap?.running) running = true;
      } catch {}
    })();
    const unlistens: Array<Promise<() => void>> = [
      listen<SpeedResult>('speedtest:result', ({ payload }) => {
        if (!payload) return;
        current = payload;
        history = [payload, ...history.filter(h => h.timestamp !== payload.timestamp)].slice(0, 10);
        running = false;
      }),
      listen<{ running: boolean }>('speedtest:running', ({ payload }) => {
        if (payload && typeof payload.running === 'boolean') running = payload.running;
      }),
    ];
    return () => { unlistens.forEach(p => p.then(fn => fn()).catch(() => {})); };
  });

  async function cancelTest() {
    cancelling = true;
    try { await invoke('cmd_cancel_speedtest'); } catch (e) { console.error(e); }
    running = false;
    cancelling = false;
  }

  async function runTest() {
    running = true; error = ''; cancelling = false;
    try {
      // On laisse le listener `speedtest:result` peupler current/history
      // (source unique de vérité, déclenché par le backend). Sans ça, on
      // prepend deux fois : une via invoke→result et une via l'event.
      if ($settings.speedtestEngine === 'iperf3') {
        const server = $settings.iperfServer.trim();
        if (!server) {
          throw new Error($_('speedtest.iperf_no_server'));
        }
        await invoke('cmd_run_iperf3', { server });
      } else {
        await invoke('cmd_run_speedtest');
      }
    } catch (e) {
      // Annulation volontaire → pas de message d'erreur rouge.
      error = String(e).includes('cancelled') ? '' : formatBackendError(e);
      running = false;
    }
  }

  // Le backend renvoie soit un message brut, soit une clé i18n au format
  // `speedtest.errors.<code>[|k=v[|k=v]...]`. On détecte le préfixe connu
  // et on parse les arguments pour l'interpolation svelte-i18n.
  function formatBackendError(e: unknown): string {
    const raw = String(e);
    if (!raw.startsWith('speedtest.errors.')) return raw;
    const [key, ...argParts] = raw.split('|');
    const values: Record<string, string> = {};
    for (const p of argParts) {
      const idx = p.indexOf('=');
      if (idx > 0) values[p.slice(0, idx)] = p.slice(idx + 1);
    }
    return $_(key, { values });
  }

  function fmt(ts: number) {
    return new Date(ts * 1000).toLocaleString();
  }

  // Au-delà de 1000 Mbps (test iperf3 10GbE typique) les chiffres deviennent
  // illisibles en Mbps, on passe en Gbps avec 2 décimales.
  function fmtSpeed(mbps: number): { val: string; unit: string } {
    if (mbps >= 1000) return { val: (mbps / 1000).toFixed(2), unit: 'Gbps' };
    return { val: mbps.toFixed(2), unit: 'Mbps' };
  }
</script>

<div class="page">
  <div class="header">
    <h1>{$_('speedtest.title')}</h1>
    <div class="header-actions">
      <SchedulerControl field="speedtest_interval_min" />
      {#if running}
        <button class="danger" onclick={cancelTest} disabled={cancelling}>
          {cancelling ? $_('speedtest.cancelling') : $_('speedtest.cancel')}
        </button>
      {:else}
        <button class="primary" onclick={runTest}>{$_('speedtest.run')}</button>
      {/if}
    </div>
  </div>
  {#if error}<div class="error">{error}</div>{/if}

  {#if running}
    <div class="placeholder-card">
      <div class="spinner"></div>
      <div style="margin-top: 12px;">{$_('speedtest.measuring')}</div>
      <button class="danger cancel-inline" onclick={cancelTest} disabled={cancelling}>
        {cancelling ? $_('speedtest.cancelling') : $_('speedtest.cancel')}
      </button>
    </div>
  {:else if current}
    <div class="result-card">
      <div class="metric">
        <div class="metric-value" style="color: var(--ep-accent)">↓ {fmtSpeed(current.download_mbps).val}</div>
        <div class="metric-label">{fmtSpeed(current.download_mbps).unit} {$_('speedtest.down_suffix')}</div>
      </div>
      <div class="metric">
        <div class="metric-value" style="color: var(--ep-success)">↑ {fmtSpeed(current.upload_mbps).val}</div>
        <div class="metric-label">{fmtSpeed(current.upload_mbps).unit} {$_('speedtest.up_suffix')}</div>
      </div>
      <div class="metric">
        <div class="metric-value">{current.latency_ms}</div>
        <div class="metric-label">{$_('speedtest.latency_unit')}{current.jitter_ms != null ? ` / ±${current.jitter_ms.toFixed(1)}ms ${$_('speedtest.jitter_suffix')}` : ''}</div>
      </div>
    </div>
    <div class="server-row">
      <span class="server-name">🖥 {current.server_name}</span>
      {#if current.result_url}
        <button class="share-btn" type="button" onclick={() => openUrl(current!.result_url!)}>
          {$_('speedtest.view_result')}
        </button>
      {/if}
    </div>
    {#if current.result_url}
      <div class="url-box">
        <span class="url-label">{$_('speedtest.share_link')}</span>
        <code class="url-text">{current.result_url}</code>
        <button class="copy-btn" onclick={() => navigator.clipboard.writeText(current!.result_url!)}>{$_('speedtest.copy')}</button>
      </div>
    {/if}
  {:else}
    <div class="placeholder-card">{$_('speedtest.placeholder')}</div>
  {/if}

  {#if history.length > 1}
    <h2>{$_('speedtest.history')}</h2>
    <table>
      <thead><tr><th>{$_('speedtest.col_date')}</th><th>{$_('speedtest.col_down')}</th><th>{$_('speedtest.col_up')}</th><th>{$_('speedtest.col_latency')}</th><th>{$_('speedtest.col_link')}</th></tr></thead>
      <tbody>
        {#each history as r}
          <tr>
            <td class="secondary">{fmt(r.timestamp)}</td>
            <td class="mono">{fmtSpeed(r.download_mbps).val} {fmtSpeed(r.download_mbps).unit}</td>
            <td class="mono">{fmtSpeed(r.upload_mbps).val} {fmtSpeed(r.upload_mbps).unit}</td>
            <td class="mono">{r.latency_ms} ms</td>
            <td>{#if r.result_url}<button type="button" class="link link-btn" onclick={() => openUrl(r.result_url!)}>↗</button>{:else}—{/if}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>

<style>
  .page { padding: 24px; }
  .header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 24px; }
  h1 { font-size: 20px; font-weight: 700; }
  h2 { font-size: 16px; font-weight: 600; margin: 24px 0 12px; }
  .header-actions { display: flex; align-items: center; gap: 10px; }
  button.primary { padding: 8px 18px; border-radius: 6px; background: var(--ep-accent); border: none; color: #fff; cursor: pointer; font-size: 14px; font-weight: 600; }
  button.danger { padding: 8px 18px; border-radius: 6px; background: var(--ep-danger); border: none; color: #fff; cursor: pointer; font-size: 14px; font-weight: 600; }
  .cancel-inline { margin-top: 16px; }
  button:disabled { opacity: 0.5; cursor: not-allowed; }
  .error { color: var(--ep-danger); font-size: 13px; margin-bottom: 12px; }
  .result-card {
    display: flex; gap: 24px;
    background: var(--ep-glass-bg);
    border: 1px solid var(--ep-glass-border);
    border-radius: var(--ep-radius-lg);
    padding: 32px; margin-bottom: 12px;
    transition: border-color 0.15s;
  }
  .result-card:hover { border-color: var(--ep-glass-border-strong); }
  .metric { flex: 1; text-align: center; }
  .metric-value { font-size: 48px; font-weight: 800; line-height: 1; font-family: var(--ep-font-mono); }
  .metric-label { font-size: 12px; color: var(--ep-text-secondary); margin-top: 6px; }
  .server-row { display: flex; align-items: center; justify-content: space-between; margin-bottom: 12px; padding: 0 4px; }
  .server-name { font-size: 12px; color: var(--ep-text-muted); }
  .share-btn { font-size: 12px; color: var(--ep-accent); text-decoration: none; font-weight: 600; background: none; border: none; padding: 0; cursor: pointer; }
  .share-btn:hover { text-decoration: underline; }
  .link-btn { background: none; border: none; padding: 0; cursor: pointer; }
  .url-box { background: var(--ep-glass-bg); border: 1px solid var(--ep-glass-border); border-radius: var(--ep-radius-md); padding: 10px 14px; display: flex; align-items: center; gap: 10px; margin-bottom: 24px; }
  .url-label { font-size: 11px; color: var(--ep-text-muted); text-transform: uppercase; letter-spacing: .5px; white-space: nowrap; }
  .url-text { font-family: var(--ep-font-mono); font-size: 12px; color: var(--ep-accent); flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .copy-btn { padding: 4px 10px; border-radius: 5px; border: 1px solid var(--ep-border); background: var(--ep-bg-tertiary); color: var(--ep-text-primary); cursor: pointer; font-size: 11px; white-space: nowrap; }
  .copy-btn:hover { background: var(--ep-border); }
  .placeholder-card { background: var(--ep-glass-bg); border: 1px dashed var(--ep-glass-border); border-radius: var(--ep-radius-lg); padding: 48px; text-align: center; color: var(--ep-text-muted); font-size: 14px; margin-bottom: 24px; }
  .spinner { width: 32px; height: 32px; border: 3px solid var(--ep-border); border-top-color: var(--ep-accent); border-radius: 50%; animation: spin 0.8s linear infinite; margin: 0 auto; }
  @keyframes spin { to { transform: rotate(360deg); } }
  table { width: 100%; border-collapse: collapse; font-size: 13px; }
  th { text-align: left; padding: 8px 12px; border-bottom: 1px solid var(--ep-glass-border); color: var(--ep-text-secondary); font-size: 11px; text-transform: uppercase; letter-spacing: .5px; }
  td { padding: 8px 12px; border-bottom: 1px solid var(--ep-glass-border); }
  .mono { font-family: var(--ep-font-mono); }
  .secondary { color: var(--ep-text-secondary); }
  .link { color: var(--ep-accent); text-decoration: none; font-size: 14px; }
</style>
