<script lang="ts">
  import { _ } from 'svelte-i18n';
  import Sparkline from './Sparkline.svelte';

  interface Props {
    ip: string;
    hostname?: string | null;
    mac?: string | null;
    vendor?: string | null;
    latencyMs?: number | null;
    history?: number[];
    onPorts?: (ip: string) => void;
    onPing?: (ip: string) => void;
  }
  let { ip, hostname, mac, vendor, latencyMs, history = [], onPorts, onPing }: Props = $props();

  type State = 'alive' | 'warn' | 'down';
  const state: State = $derived(
    latencyMs == null ? 'down' : latencyMs > 20 ? 'warn' : 'alive'
  );
  const dotColor = $derived(
    state === 'alive' ? 'var(--ep-success)' :
    state === 'warn'  ? 'var(--ep-warning)' :
                        'var(--ep-danger)'
  );
</script>

<div class="row mono" role="row">
  <span class="col dot-col"><span class="dot" style:background={dotColor}></span></span>
  <span class="col ip">{ip}</span>
  <span class="col host">{hostname || $_('common.host.unknown_hostname')}</span>
  <span class="col mac">
    <span class="mac-addr">{mac || $_('common.host.no_mac')}</span>
    {#if vendor}<span class="mac-vendor">{vendor}</span>{/if}
  </span>
  <span class="col rtt" class:ok={state === 'alive'} class:warn={state === 'warn'} class:err={state === 'down'}>
    {latencyMs != null ? `${latencyMs} ms` : '—'}
  </span>
  <span class="col spark"><Sparkline values={history} color={dotColor} /></span>
  <span class="col actions">
    {#if onPorts}
      <button class="act" onclick={() => onPorts?.(ip)} title={$_('common.host.actions.ports')}>→</button>
    {/if}
    {#if onPing}
      <button class="act" onclick={() => onPing?.(ip)} title={$_('common.host.actions.ping')}>📡</button>
    {/if}
  </span>
</div>

<style>
  .row {
    display: grid;
    grid-template-columns: 24px 110px 1fr 130px 60px 50px 70px;
    padding: 7px 12px;
    font-size: 11px;
    align-items: center;
    border-bottom: 1px solid var(--ep-bg-secondary);
    font-family: var(--ep-font-mono);
  }
  .row:hover { background: var(--ep-bg-hover); }
  .row:hover .actions { opacity: 1; }
  .row:last-child { border-bottom: none; }
  .dot { display: inline-block; width: 6px; height: 6px; border-radius: 50%; }
  .ip { color: var(--ep-text-primary); }
  .host { color: var(--ep-text-secondary); }
  .mac { color: var(--ep-text-muted); font-size: 9px; display: flex; flex-direction: column; line-height: 1.2; }
  .mac-vendor { color: var(--ep-text-muted); opacity: .8; }
  .rtt.ok { color: var(--ep-success); }
  .rtt.warn { color: var(--ep-warning); }
  .rtt.err { color: var(--ep-danger); }
  .actions { opacity: 0; transition: opacity .15s; text-align: right; display: flex; gap: 4px; justify-content: flex-end; }
  .act {
    padding: 2px 6px; background: var(--ep-bg-tertiary); border: 1px solid var(--ep-border);
    border-radius: var(--ep-radius-sm); color: var(--ep-text-secondary); font-size: 9px; cursor: pointer;
  }
  .act:hover { background: var(--ep-bg-secondary); color: var(--ep-text-primary); border-color: var(--ep-accent); }
  @media (max-width: 900px) {
    .row { grid-template-columns: 24px 110px 1fr 60px 50px 70px; }
    .mac { display: none; }
  }
</style>
