<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { _ } from 'svelte-i18n';
  import { internetStatus, type InternetState } from '../stores/internetStatus';
  import { internetMark } from '../status';
  import StateMark from './StateMark.svelte';

  const { variant = 'chip', onClick }: { variant?: 'chip' | 'block'; onClick?: () => void } = $props();

  let showPopover = $state(false);
  interface PublicIpInfo { ip: string; country: string | null; city: string | null; isp: string | null; }
  let publicIp = $state<PublicIpInfo | null>(null);

  onMount(() => {
    internetStatus.init();
    loadPublicIp();
  });

  async function loadPublicIp() {
    try { publicIp = await invoke<PublicIpInfo>('cmd_get_public_ip'); }
    catch { /* hors ligne — on laisse vide */ }
  }

  const stateClass = $derived<InternetState>($internetStatus?.state ?? 'offline');
  $effect(() => {
    if (stateClass === 'online' && !publicIp) loadPublicIp();
  });
  const icmpMs = $derived($internetStatus?.icmp_ms ?? null);
  const httpMs = $derived($internetStatus?.http_ms ?? null);
  const icmpOk = $derived($internetStatus?.icmp_ok ?? false);
  const httpOk = $derived($internetStatus?.http_ok ?? false);
  const uptime = $derived($internetStatus?.uptime_pct ?? 0);
  const ageSec = $derived.by(() => {
    const ts = $internetStatus?.timestamp;
    if (!ts) return null;
    return Math.max(0, Math.floor(Date.now() / 1000 - ts));
  });
  const mark = $derived(internetMark($internetStatus ? stateClass : null));
  const ms = $derived(stateClass === 'online' ? (icmpMs ?? httpMs) : null);
</script>

<div class="ii-wrap ii-{variant}">
  <button
    class="ii"
    onmouseenter={() => (showPopover = true)}
    onmouseleave={() => (showPopover = false)}
    onfocus={() => (showPopover = true)}
    onblur={() => (showPopover = false)}
    onkeydown={(event) => { if (event.key === 'Escape') showPopover = false; }}
    onclick={onClick}
    type="button"
    aria-describedby="ii-tooltip"
  >
    <StateMark {mark} label={$_('topbar.internet.label')} />
    {#if ms != null}<span class="ms num">{ms}<span class="u">ms</span></span>{/if}
  </button>

  {#if showPopover && $internetStatus}
    <div class="pop" role="tooltip" id="ii-tooltip">
      <dl class="kv">
        <dt>{$_('topbar.internet.state')}</dt>
        <dd><StateMark {mark} /></dd>
        <dt>{$_('topbar.internet.icmp')}</dt>
        <dd>
          <span class="m">{$internetStatus.icmp_target}</span>
          <span class="v" class:bad={!icmpOk && stateClass !== 'online'}>
            {icmpOk && icmpMs != null ? `${icmpMs} ms` : stateClass === 'online' ? '—' : $_('topbar.internet.timeout')}
          </span>
        </dd>
        <dt>{$_('topbar.internet.http')}</dt>
        <dd>
          <span class="m">{$internetStatus.http_target.replace(/^https?:\/\//, '')}</span>
          <span class="v" class:bad={!httpOk}>{httpOk && httpMs != null ? `${httpMs} ms` : $_('topbar.internet.timeout')}</span>
        </dd>
        {#if publicIp}
          <dt>{$_('topbar.internet.public_ip')}</dt>
          <dd>
            <span class="m">{publicIp.ip}</span>
            <span class="muted">{[publicIp.city, publicIp.country].filter(Boolean).join(', ') || publicIp.isp || ''}</span>
          </dd>
        {/if}
        <dt>{$_('topbar.internet.uptime')}</dt>
        <dd class="num">{uptime.toFixed(2)} %{#if ageSec != null}<span class="muted"> · {$_('topbar.internet.last_tick', { values: { n: ageSec } })}</span>{/if}</dd>
      </dl>
    </div>
  {/if}
</div>

<style>
  .ii-wrap { position: relative; display: inline-block; }
  .ii {
    display: inline-flex; align-items: center; gap: var(--space-xs);
    min-height: 30px; padding: 0 var(--space-sm);
    border: 1px solid var(--color-rule-2); border-radius: var(--radius-pill);
    background: var(--color-paper-2); cursor: default;
  }
  .ii:hover { border-color: var(--color-field); }
  .ii-block { display: block; }
  .ii-block .ii { border: 0; padding: 0; min-height: 0; background: none; }
  .ms { font-size: var(--text-sm); color: var(--color-ink-2); }
  .pop {
    position: absolute; z-index: var(--z-dropdown);
    min-width: 300px; padding: var(--space-sm) var(--space-md);
    background: var(--color-paper-4); border: 1px solid var(--color-rule-2);
    border-radius: var(--radius-ctl); box-shadow: var(--shadow-pop);
    font-size: var(--text-sm); text-align: left;
  }
  .ii-chip .pop { top: calc(100% + 8px); right: 0; }
  .ii-block .pop { left: calc(100% + 16px); bottom: 0; }
  .pop dd { display: flex; flex-wrap: wrap; gap: 0 var(--space-xs); align-items: baseline; }
  .v { margin-left: auto; font-variant-numeric: tabular-nums; }
  .v.bad { color: var(--st-bad); }
</style>
