<script lang="ts">
  import { _ } from 'svelte-i18n';

  interface Props {
    interfaceName?: string | null;
    hosts: number;
    down: number;
    avgRttMs?: number | null;
    downMbps?: number | null;
    upMbps?: number | null;
  }
  let { interfaceName, hosts, down, avgRttMs, downMbps, upMbps }: Props = $props();
</script>

<div class="rail">
  <div class="tile">
    <div class="lbl">{$_('dashboard.stat_rail.iface')}</div>
    <div class="val mono accent">{interfaceName || '—'}</div>
  </div>
  <div class="tile">
    <div class="lbl">{$_('dashboard.stat_rail.hosts')}</div>
    <div class="val ok">{hosts}</div>
  </div>
  <div class="tile">
    <div class="lbl">{$_('dashboard.stat_rail.down')}</div>
    <div class="val" class:err={down > 0}>{down > 0 ? down : '—'}</div>
  </div>
  <div class="tile">
    <div class="lbl">{$_('dashboard.stat_rail.avg_rtt')}</div>
    <div class="val mono">{avgRttMs != null ? `${avgRttMs} ms` : '—'}</div>
  </div>
  <div class="tile">
    <div class="lbl">{$_('dashboard.stat_rail.throughput')}</div>
    <div class="val mono sm">
      {downMbps != null ? `${Math.round(downMbps)}` : '—'}
      <span class="sep">/</span>
      {upMbps != null ? `${Math.round(upMbps)}` : '—'}
      <span class="unit">Mbps</span>
    </div>
  </div>
</div>

<style>
  .rail {
    display: grid;
    grid-template-columns: repeat(5, 1fr);
    gap: 6px;
    margin-bottom: 12px;
  }
  .tile {
    background: var(--ep-bg-secondary);
    border: 1px solid var(--ep-border);
    border-radius: var(--ep-radius-md);
    padding: 10px 12px;
  }
  .lbl {
    font-size: 9px; text-transform: uppercase; letter-spacing: .8px;
    color: var(--ep-text-dim); margin-bottom: 4px;
  }
  .val { font-size: 18px; font-weight: 700; color: var(--ep-text-primary); }
  .val.sm { font-size: 13px; }
  .val.mono { font-family: var(--ep-font-mono); }
  .val.ok { color: var(--ep-success); }
  .val.err { color: var(--ep-danger); }
  .accent { color: var(--ep-accent-bright); }
  .sep { color: var(--ep-text-dim); margin: 0 2px; }
  .unit { font-size: 9px; color: var(--ep-text-dim); font-weight: 500; margin-left: 2px; }
</style>
