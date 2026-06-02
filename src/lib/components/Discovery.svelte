<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { onMount, onDestroy } from 'svelte';
  import { get } from 'svelte/store';
  import { _ } from 'svelte-i18n';
  import { discoveryStore, type DiscoveredHost } from '../stores/discovery';
  import { portscan } from '../stores/portscan';
  import { scheduler } from '../stores/scheduler';
  import { portscanProfiles } from '../stores/portscanProfiles';
  import { monitoring } from '../stores/monitoring';
  import { selectedInterface } from '../stores/selectedInterface';
  import SchedulerControl from './SchedulerControl.svelte';

  // L'état vit dans le store — persiste pendant toute la session même si on change de page
  let cidr = $state('');
  let detecting = $state(false);

  const found = $derived(
    Array.from($discoveryStore.results.values()).sort((a, b) => {
      const toNum = (ip: string) => ip.split('.').reduce((acc, b) => acc * 256 + +b, 0);
      return toNum(a.ip) - toNum(b.ip);
    })
  );

  let unlistenHost: (() => void) | null = null;
  let unlistenLatency: (() => void) | null = null;
  let unlistenMac: (() => void) | null = null;
  let unlistenDone: (() => void) | null = null;

  onMount(async () => {
    scheduler.init();
    // On redétecte toujours au mount à partir de l'interface sélectionnée :
    // sur Windows, se reposer sur le cache du store pouvait laisser le CIDR
    // du vEthernet WSL (fallback route par défaut) alors que l'utilisateur
    // avait bien choisi la bonne interface sur le Dashboard.
    detecting = true;
    try {
      const local = await invoke<string | null>('cmd_get_local_network_cidr', {
        ifaceName: $selectedInterface || null,
      });
      cidr = local ?? $discoveryStore.cidr ?? '192.168.1.0/24';
    } catch { cidr = $discoveryStore.cidr ?? '192.168.1.0/24'; }
    detecting = false;
    discoveryStore.setCidr(cidr);

    // Ré-enregistre les listeners à chaque montage (le scan peut toujours tourner en bg)
    unlistenHost = await listen<DiscoveredHost>('discovery:host', ({ payload }) => {
      discoveryStore.addHost(payload);
    });
    unlistenLatency = await listen<{ ip: string; latency_ms: number }>('discovery:host_latency', ({ payload }) => {
      discoveryStore.updateLatency(payload.ip, payload.latency_ms);
    });
    unlistenMac = await listen<{ ip: string; mac: string; vendor: string | null }>('discovery:host_mac', ({ payload }) => {
      discoveryStore.updateMac(payload.ip, payload.mac, payload.vendor);
    });
    unlistenDone = await listen('discovery:done', () => {
      discoveryStore.setScanning(false);
    });
    // Hydrate depuis le backend : si le desktop a déjà lancé un scan
    // (ou si on est un client web fraîchement connecté), on récupère les
    // hosts déjà trouvés en un coup plutôt que d'attendre un nouveau scan.
    await discoveryStore.hydrate();
  });

  onDestroy(() => {
    unlistenHost?.();
    unlistenLatency?.();
    unlistenMac?.();
    unlistenDone?.();
    // Le store garde son état — le scan continue en background si besoin
  });

  async function autoDetect() {
    detecting = true;
    try {
      const local = await invoke<string | null>('cmd_get_local_network_cidr', {
        ifaceName: $selectedInterface || null,
      });
      if (local) { cidr = local; discoveryStore.setCidr(local); }
    } catch {}
    detecting = false;
  }

  // Quand l'utilisateur change d'interface sur le Dashboard, on re-détecte
  // automatiquement le CIDR pour rester aligné (sinon on reste scotché sur
  // l'ancien subnet, ex. celui de vEthernet WSL sur Windows).
  let lastIface = $state<string>('');
  $effect(() => {
    const iface = $selectedInterface;
    if (iface && iface !== lastIface) {
      lastIface = iface;
      if (!$discoveryStore.scanning) autoDetect();
    }
  });

  async function scan() {
    if (!cidr.trim() || $discoveryStore.scanning) return;
    discoveryStore.clear();
    discoveryStore.setScanning(true);
    discoveryStore.setError('');
    scheduler.save({ ...get(scheduler), discovery_cidr: cidr });
    try {
      await invoke('cmd_scan_network', { cidr });
    } catch (e) {
      discoveryStore.setError(String(e));
      discoveryStore.setScanning(false);
    }
  }

  async function cancelScan() {
    // Optimiste : on bascule l'UI immédiatement. Le backend émettra
    // aussi discovery:done tout de suite (idempotent via le listener).
    discoveryStore.setScanning(false);
    try { await invoke('cmd_cancel_scan'); } catch (e) { console.error(e); }
  }

  // Menu contextuel clic-droit sur un hôte
  let ctxMenu = $state<{ x: number; y: number; ip: string } | null>(null);

  function openContextMenu(e: MouseEvent, ip: string) {
    e.preventDefault();
    ctxMenu = { x: e.clientX, y: e.clientY, ip };
  }
  function closeContextMenu() { ctxMenu = null; }

  function scanPortsFor(ip: string) {
    const active = get(portscanProfiles).find(p => p.id === get(portscanProfiles.active));
    portscan.add(ip, active?.tcp_ports, active?.udp_ports, active?.id ?? null, active?.name ?? null);
  }
  async function addPingMonitorFor(ip: string) {
    monitoring.addHost(ip);
    try { await invoke('cmd_start_ping', { ip }); } catch (e) { console.error(e); }
  }

  function ctxScanPorts() {
    if (!ctxMenu) return;
    scanPortsFor(ctxMenu.ip);
    closeContextMenu();
  }
  async function ctxAddPingMonitor() {
    if (!ctxMenu) return;
    await addPingMonitorFor(ctxMenu.ip);
    closeContextMenu();
  }
</script>

<svelte:window onclick={closeContextMenu} oncontextmenu={(e) => { if (!(e.target as HTMLElement).closest('.host-row')) closeContextMenu(); }} />

<div class="page">
  <div class="header">
    <div class="title-row">
      <h1>{$_('discovery.title')}</h1>
      {#if $discoveryStore.scanning}
        <span class="badge scanning">{$_('discovery.scanning')}</span>
      {:else if found.length > 0}
        <span class="badge done">{$_('discovery.hosts_count', { values: { n: found.length } })}</span>
      {/if}
    </div>
    <div class="scan-row">
      <input
        class="cidr-input mono"
        bind:value={cidr}
        oninput={(e) => discoveryStore.setCidr((e.currentTarget as HTMLInputElement).value)}
        placeholder={detecting ? '…' : '192.168.1.0/24'}
        disabled={detecting || $discoveryStore.scanning}
        onkeydown={(e) => e.key === 'Enter' && !$discoveryStore.scanning && scan()}
        title={$_('discovery.cidr_hint')}
      />
      <button class="btn-icon" onclick={autoDetect} disabled={detecting || $discoveryStore.scanning} title={$_('discovery.auto_detect')}>⟳</button>
      {#if $discoveryStore.scanning}
        <button class="danger" onclick={cancelScan}>{$_('discovery.cancel')}</button>
      {:else}
        <button class="primary" onclick={scan} disabled={detecting || !cidr}>{$_('discovery.scan')}</button>
      {/if}
      <SchedulerControl field="discovery_interval_min" />
    </div>
  </div>

  {#if $discoveryStore.error}
    <div class="error">{$discoveryStore.error}</div>
  {/if}

  {#if found.length > 0}
    <table>
      <thead>
        <tr><th>{$_('discovery.table.ip')}</th><th>{$_('discovery.table.hostname')}</th><th>{$_('discovery.table.mac')}</th><th>{$_('discovery.table.latency')}</th><th class="col-actions">{$_('discovery.table.actions')}</th></tr>
      </thead>
      <tbody>
        {#each found as h (h.ip)}
          <tr class="host-row" oncontextmenu={(e) => openContextMenu(e, h.ip)}>
            <td class="mono">{h.ip}</td>
            <td class="secondary">{h.hostname ?? '—'}</td>
            <td class="mono secondary mac-cell">
              <span class="mac-addr">{h.mac ?? '—'}</span>
              {#if h.vendor}<span class="vendor">{h.vendor}</span>{/if}
            </td>
            <td class="mono">
              {#if h.latency_ms != null}
                <span class="latency" class:fast={h.latency_ms < 5} class:ok={h.latency_ms >= 5 && h.latency_ms < 50}>{h.latency_ms}ms</span>
              {:else if $discoveryStore.scanning}
                <span class="ping-pending">…</span>
              {:else}—{/if}
            </td>
            <td class="actions-cell">
              <button class="act" title={$_('discovery.actions.scan_ports')} onclick={() => scanPortsFor(h.ip)}>{$_('discovery.actions.ports_label')}</button>
              <button class="act" title={$_('discovery.actions.add_ping')} onclick={() => addPingMonitorFor(h.ip)}>{$_('discovery.actions.ping_label')}</button>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {:else if !$discoveryStore.scanning}
    <div class="placeholder">{$_('discovery.empty')}</div>
  {/if}

  {#if ctxMenu}
    <div
      class="ctx-menu"
      style="left: {ctxMenu.x}px; top: {ctxMenu.y}px;"
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => { if (e.key === 'Escape') closeContextMenu(); }}
      oncontextmenu={(e) => e.preventDefault()}
      role="menu"
      tabindex="-1"
    >
      <div class="ctx-header mono">{ctxMenu.ip}</div>
      <button class="ctx-item" onclick={ctxScanPorts}>{$_('discovery.ctx.scan_ports')}</button>
      <button class="ctx-item" onclick={ctxAddPingMonitor}>{$_('discovery.ctx.add_ping')}</button>
    </div>
  {/if}
</div>

<style>
  .page { padding: 24px; }
  .header { display: flex; justify-content: space-between; align-items: flex-start; margin-bottom: 20px; flex-wrap: wrap; gap: 12px; }
  .title-row { display: flex; align-items: center; gap: 10px; }
  h1 { font-size: 20px; font-weight: 700; }
  .badge { font-size: 11px; font-weight: 600; padding: 3px 8px; border-radius: 10px; }
  .badge.scanning { background: color-mix(in srgb, var(--ep-accent) 15%, transparent); color: var(--ep-accent); }
  .badge.done { background: color-mix(in srgb, var(--ep-success) 15%, transparent); color: var(--ep-success); }
  .scan-row { display: flex; gap: 6px; align-items: center; }
  .cidr-input {
    background: var(--ep-bg-tertiary);
    border: 1px solid var(--ep-border);
    border-radius: 6px;
    padding: 7px 12px;
    color: var(--ep-text-primary);
    font-size: 13px;
    font-weight: 600;
    width: 160px;
  }
  .cidr-input:focus {
    outline: none;
    border-color: var(--ep-accent);
  }
  .cidr-input:disabled { opacity: 0.6; }
  button { padding: 7px 14px; border-radius: 6px; border: 1px solid var(--ep-border); background: var(--ep-bg-tertiary); color: var(--ep-text-primary); cursor: pointer; font-size: 13px; font-weight: 600; }
  button.primary { background: var(--ep-accent); border-color: var(--ep-accent); color: #fff; }
  button.danger { background: var(--ep-danger); border-color: var(--ep-danger); color: #fff; }
  button.btn-icon { width: 32px; padding: 0; display: flex; align-items: center; justify-content: center; font-size: 16px; }
  button:disabled { opacity: 0.5; cursor: not-allowed; }
  .error { color: var(--ep-danger); font-size: 13px; margin-bottom: 12px; }
  .placeholder { background: var(--ep-glass-bg); border: 1px dashed var(--ep-glass-border); border-radius: var(--ep-radius-lg); padding: 40px; text-align: center; color: var(--ep-text-muted); font-size: 14px; }
  table { width: 100%; border-collapse: collapse; font-size: 13px; }
  th { text-align: left; padding: 8px 12px; border-bottom: 1px solid var(--ep-glass-border); color: var(--ep-text-secondary); font-size: 11px; text-transform: uppercase; letter-spacing: .5px; }
  td { padding: 8px 12px; border-bottom: 1px solid var(--ep-glass-border); }
  .host-row:hover { background: var(--ep-glass-bg); }
  .col-actions { text-align: right; }
  .actions-cell { text-align: right; white-space: nowrap; }
  .actions-cell .act { padding: 4px 10px; margin-left: 4px; font-size: 11px; font-weight: 600; border: 1px solid var(--ep-border); background: var(--ep-bg-tertiary); color: var(--ep-text-primary); border-radius: 6px; cursor: pointer; }
  .actions-cell .act:hover { border-color: var(--ep-accent); color: var(--ep-accent); }
  .mono { font-family: var(--ep-font-mono); }
  .secondary { color: var(--ep-text-secondary); }
  .mac-cell { display: flex; flex-direction: column; line-height: 1.25; }
  .vendor { font-size: 10px; color: var(--ep-text-muted); }
  .latency { font-weight: 600; color: var(--ep-text-secondary); }
  .latency.fast { color: var(--ep-success); }
  .latency.ok { color: var(--ep-accent); }
  .ping-pending { color: var(--ep-text-muted); animation: pulse 1s ease-in-out infinite; }
  @keyframes pulse { 0%, 100% { opacity: 0.3; } 50% { opacity: 1; } }
  .host-row { cursor: context-menu; }
  .ctx-menu {
    position: fixed; z-index: 1000;
    background: var(--ep-bg-secondary); border: 1px solid var(--ep-glass-border-strong);
    border-radius: var(--ep-radius-md); padding: 4px;
    box-shadow: 0 8px 24px rgba(0,0,0,0.4);
    min-width: 220px;
  }
  .ctx-header { padding: 6px 10px 4px; font-size: 11px; color: var(--ep-text-muted); border-bottom: 1px solid var(--ep-glass-border); margin-bottom: 4px; }
  .ctx-item {
    display: block; width: 100%; text-align: left;
    padding: 8px 10px; border: none; background: transparent;
    color: var(--ep-text-primary); font-size: 12px; cursor: pointer;
    border-radius: 4px;
  }
  .ctx-item:hover { background: var(--ep-glass-bg-md); }
</style>
