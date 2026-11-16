<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { _ } from 'svelte-i18n';
  import { settings } from '../stores/settings';
  import { scheduler } from '../stores/scheduler';
  import SchedulerControl from './SchedulerControl.svelte';
  import ScreenHead from './ScreenHead.svelte';
  import { locale } from 'svelte-i18n';
  import { fmtNum } from '../format';
  import { screenKind } from '../screen';

  const isCell = screenKind() === 'cell';
  const lang = $derived($locale ?? 'en');
  let copied = $state(false);
  async function copyLink(url: string) {
    try { await navigator.clipboard.writeText(url); copied = true; setTimeout(() => (copied = false), 2000); } catch {}
  }

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
    return new Date(ts * 1000).toLocaleString(lang, { day: '2-digit', month: '2-digit', hour: '2-digit', minute: '2-digit' });
  }
  function hm(ts: number) {
    return new Date(ts * 1000).toLocaleTimeString(lang, { hour: '2-digit', minute: '2-digit' });
  }

  // Au-delà de 1000 Mbit/s (test iperf3 10GbE typique) les chiffres deviennent
  // illisibles en Mbit/s, on passe en Gbit/s avec 2 décimales.
  function fmtSpeed(mbps: number): { val: string; unit: string } {
    if (mbps >= 1000) return { val: fmtNum(mbps / 1000, lang, 2), unit: 'Gbit/s' };
    return { val: fmtNum(mbps, lang, mbps >= 100 ? 0 : 1), unit: 'Mbit/s' };
  }
</script>

<div class="screen">
  <ScreenHead title={$_('speedtest.title')}>
    {#snippet acts()}
      {#if running}
        <button class="btn" class:sm={isCell} type="button" onclick={cancelTest} disabled={cancelling} aria-busy={cancelling}>
          {cancelling ? $_('speedtest.cancelling') : $_('speedtest.cancel')}
        </button>
      {:else}
        <button class="btn primary" class:sm={isCell} type="button" onclick={runTest}>{isCell ? $_('speedtest.run_short') : $_('speedtest.run')}</button>
      {/if}
    {/snippet}
  </ScreenHead>

  <div class="screen-body">
    <div class="filters">
      <SchedulerControl field="speedtest_interval_min" />
      <span class="grow"></span>
      <span class="muted">{$settings.speedtestEngine === 'iperf3' ? `iperf3${$settings.iperfServer ? `, ${$settings.iperfServer}` : ''}` : 'Ookla'}</span>
    </div>
    {#if error}<div class="banner bad" role="alert"><span class="grow">{error}</span></div>{/if}

    {#if running}
      <div class="banner info" role="status">
        <span class="grow">{$_('speedtest.measuring')}</span>
        <span class="progress indet"><span></span></span>
      </div>
    {/if}

    {#if current}
      {@const d = fmtSpeed(current.download_mbps)}
      {@const u = fmtSpeed(current.upload_mbps)}
      <div class="panel ticks result" class:stale={running}>
        <div><div class="muted">{$_('speedtest.down')}</div><div class="big">{d.val}<span class="u">{d.unit}</span></div></div>
        <div><div class="muted">{$_('speedtest.up')}</div><div class="big">{u.val}<span class="u">{u.unit}</span></div></div>
        <div><div class="muted">{$_('speedtest.latency')}</div><div class="big">{fmtNum(current.latency_ms, lang)}<span class="u">ms</span></div></div>
        {#if current.jitter_ms != null}
          <div><div class="muted">{$_('speedtest.jitter')}</div><div class="big">{fmtNum(current.jitter_ms, lang)}<span class="u">ms</span></div></div>
        {/if}
        <div class="meta muted">
          {hm(current.timestamp)}, {current.server_name}
          {#if current.result_url}
            <span class="links">
              <button class="link" type="button" onclick={() => openUrl(current!.result_url!)}>{$_('speedtest.view_result')}</button>
              <button class="link" type="button" onclick={() => copyLink(current!.result_url!)}>{copied ? $_('speedtest.copied') : $_('speedtest.copy')}</button>
            </span>
          {/if}
        </div>
      </div>
    {:else if !running}
      <div class="tbl-wrap"><div class="state"><p>{$_('speedtest.placeholder')}</p></div></div>
    {/if}

    {#if history.length > 0}
      <div class="sec-head"><h2 class="sec">{$_('speedtest.history')}</h2></div>
      <div class="tbl-wrap">
        <table class="t">
          <thead>
            <tr>
              <th>{$_('speedtest.col_date')}</th>
              <th class="num">{$_('speedtest.col_down')}</th>
              <th class="num">{$_('speedtest.col_up')}</th>
              <th class="num opt">{$_('speedtest.col_latency')}</th>
              <th class="num opt">{$_('speedtest.col_jitter')}</th>
              <th class="opt">{$_('speedtest.col_server')}</th>
              <th class="opt"><span class="visually-hidden">{$_('speedtest.col_link')}</span></th>
            </tr>
          </thead>
          <tbody>
            {#each history as r (r.timestamp)}
              <tr>
                <td class="num">{fmt(r.timestamp)}</td>
                <td class="num">{fmtSpeed(r.download_mbps).val}<span class="u">{fmtSpeed(r.download_mbps).unit}</span></td>
                <td class="num">{fmtSpeed(r.upload_mbps).val}<span class="u">{fmtSpeed(r.upload_mbps).unit}</span></td>
                <td class="num opt">{fmtNum(r.latency_ms, lang)}</td>
                <td class="num opt">{fmtNum(r.jitter_ms, lang)}</td>
                <td class="opt">{r.server_name}</td>
                <td class="act opt">{#if r.result_url}<button type="button" class="link" onclick={() => openUrl(r.result_url!)}>{$_('speedtest.col_link')}</button>{/if}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </div>
</div>

<style>
  .result { display: flex; flex-wrap: wrap; gap: var(--space-lg) var(--space-2xl); padding: var(--space-lg); }
  .result.stale { opacity: 0.55; }
  .result .meta { align-self: end; margin-left: auto; display: grid; gap: var(--space-2xs); justify-items: end; text-align: right; }
  .links { display: inline-flex; gap: var(--space-sm); }
  :global(.cell) .result { gap: var(--space-md) var(--space-lg); padding: var(--space-md); }
</style>
