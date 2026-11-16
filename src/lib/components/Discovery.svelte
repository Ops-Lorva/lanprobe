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
  import { matchingHosts } from '../discovery-search';
  import { locale } from 'svelte-i18n';
  import ScreenHead from './ScreenHead.svelte';
  import { fmtNum } from '../format';
  import { screenKind } from '../screen';

  const isCell = screenKind() === 'cell';
  const lang = $derived($locale ?? 'en');
  /** Retour bref après « Surveiller » ou « Ports » : l'action part ailleurs. */
  let sent = $state('');
  let sentTimer: ReturnType<typeof setTimeout> | null = null;
  function flashSent(msg: string) {
    sent = msg;
    if (sentTimer) clearTimeout(sentTimer);
    sentTimer = setTimeout(() => { sent = ''; sentTimer = null; }, 4000);
  }

  // L'état vit dans le store — persiste pendant toute la session même si on change de page
  let cidr = $state('');
  let detecting = $state(false);
  /** Ce qu'on cherche dans la liste déjà trouvée. Aucun paquet n'en part. */
  let search = $state('');

  const found = $derived(
    Array.from($discoveryStore.results.values()).sort((a, b) => {
      const toNum = (ip: string) => ip.split('.').reduce((acc, b) => acc * 256 + +b, 0);
      return toNum(a.ip) - toNum(b.ip);
    })
  );

  /** La liste réduite à ce qu'on cherche — IP, nom, MAC (ponctuation ignorée) ou constructeur. */
  const shown = $derived(matchingHosts(found, search));

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
    // ⚠️ Le profil actif **parmi ceux qui s'affichent** (§ 25) : une sonde
    // rattachée à un hub n'affiche plus ses profils de base, et scanner avec
    // une liste de ports que l'écran ne montre nulle part serait invérifiable.
    const active = get(portscanProfiles.current);
    portscan.add(ip, active?.tcp_ports, active?.udp_ports, active?.id ?? null, active?.name ?? null);
    flashSent($_('discovery.sent_ports', { values: { ip } }));
  }
  async function addPingMonitorFor(ip: string) {
    monitoring.addHost(ip);
    try { await invoke('cmd_start_ping', { ip }); } catch (e) { console.error(e); }
    flashSent($_('discovery.sent_monitor', { values: { ip } }));
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

{#snippet scanControls()}
  {#if $discoveryStore.scanning}
    <button class="btn" class:sm={isCell} type="button" onclick={cancelScan}>{$_('discovery.cancel')}</button>
  {:else}
    <button class="btn primary" class:sm={isCell} type="button" onclick={scan} disabled={detecting || !cidr}>{$_('discovery.scan')}</button>
  {/if}
{/snippet}

<div class="screen">
  <ScreenHead title={$_('discovery.title')}>
    {#snippet acts()}
      {#if isCell}
        {@render scanControls()}
      {:else}
        <input
          class="input m"
          style="width:180px"
          bind:value={cidr}
          oninput={(e) => discoveryStore.setCidr((e.currentTarget as HTMLInputElement).value)}
          placeholder={detecting ? '…' : '192.168.1.0/24'}
          disabled={detecting || $discoveryStore.scanning}
          onkeydown={(e) => e.key === 'Enter' && !$discoveryStore.scanning && scan()}
          title={$_('discovery.cidr_hint')}
          aria-label={$_('discovery.cidr_label')}
        />
        <button class="btn" type="button" onclick={autoDetect} disabled={detecting || $discoveryStore.scanning} title={$_('discovery.auto_detect')}>{$_('discovery.local_network')}</button>
        {@render scanControls()}
      {/if}
    {/snippet}
  </ScreenHead>

  <div class="screen-body">
    {#if isCell}
      <div class="input-row" style="margin-bottom:8px">
        <input class="input m" style="flex:1;min-width:140px" bind:value={cidr}
          oninput={(e) => discoveryStore.setCidr((e.currentTarget as HTMLInputElement).value)}
          placeholder={detecting ? '…' : '192.168.1.0/24'} disabled={detecting || $discoveryStore.scanning}
          onkeydown={(e) => e.key === 'Enter' && !$discoveryStore.scanning && scan()}
          title={$_('discovery.cidr_hint')} aria-label={$_('discovery.cidr_label')} />
        <button class="btn" type="button" onclick={autoDetect} disabled={detecting || $discoveryStore.scanning} title={$_('discovery.auto_detect')}>{$_('discovery.local_network')}</button>
      </div>
    {:else}
      <div class="filters">
        <SchedulerControl field="discovery_interval_min" />
      </div>
    {/if}

    {#if $discoveryStore.scanning}
      <div class="banner info" role="status">
        <span class="grow">{$_('discovery.scanning_found', { values: { n: found.length } })}</span>
        <span class="progress indet"><span></span></span>
      </div>
    {/if}
    {#if $discoveryStore.error}
      <div class="banner bad" role="alert"><span class="grow">{$discoveryStore.error}</span></div>
    {/if}
    {#if sent}
      <div class="banner info" role="status"><span class="grow">{sent}</span></div>
    {/if}

    {#if found.length > 0}
      <div class="filters">
        <input class="input search" type="search" bind:value={search} placeholder={$_('discovery.search')} aria-label={$_('discovery.search')} />
        <span class="grow"></span>
        <span class="muted">{$_('discovery.hosts_count', { values: { n: found.length } })}</span>
      </div>
    {/if}

    {#if found.length > 0 || $discoveryStore.scanning}
      <div class="tbl-wrap">
        <table class="t">
          <thead>
            <tr>
              <th>{$_('discovery.table.ip')}</th>
              <th>{$_('discovery.table.hostname')}</th>
              <th class="opt">{$_('discovery.table.mac')}</th>
              <th class="opt">{$_('discovery.table.vendor')}</th>
              <th class="num">{$_('discovery.table.latency')}</th>
              <th class="opt"><span class="visually-hidden">{$_('discovery.table.actions')}</span></th>
            </tr>
          </thead>
          <tbody>
            {#each shown as h (h.ip)}
              <tr class="host-row" oncontextmenu={(e) => openContextMenu(e, h.ip)}>
                <td class="m">{h.ip}</td>
                <td style="white-space:nowrap">{h.hostname ?? '—'}</td>
                <td class="m muted opt">{h.mac ?? '—'}</td>
                <td class="opt">{h.vendor ?? '—'}</td>
                <td class="num">
                  {#if h.latency_ms != null}{fmtNum(h.latency_ms, lang)} ms
                  {:else if $discoveryStore.scanning}<span class="muted">…</span>
                  {:else}—{/if}
                </td>
                <td class="act opt">
                  <button class="btn sm quiet" type="button" title={$_('discovery.actions.add_ping')} onclick={() => addPingMonitorFor(h.ip)}>{$_('discovery.actions.ping_label')}</button>
                  <button class="btn sm quiet" type="button" title={$_('discovery.actions.scan_ports')} onclick={() => scanPortsFor(h.ip)}>{$_('discovery.actions.ports_label')}</button>
                </td>
              </tr>
            {/each}
            {#if $discoveryStore.scanning}
              {#each [0, 1] as _i}
                <tr aria-hidden="true">
                  <td><div class="skel" style="width:110px"></div></td>
                  <td><div class="skel" style="width:90px"></div></td>
                  <td class="opt"><div class="skel" style="width:130px"></div></td>
                  <td class="opt"><div class="skel" style="width:60px"></div></td>
                  <td><div class="skel" style="width:40px;margin-left:auto"></div></td>
                  <td class="opt"></td>
                </tr>
              {/each}
            {/if}
          </tbody>
        </table>
        {#if found.length > 0 && shown.length === 0}
          <!-- ⚠️ « Rien ne correspond » n'est pas « rien trouvé » : le scan, lui,
               a vu des machines. Les confondre ferait relancer un balayage. -->
          <div class="state"><p>{$_('discovery.no_match')}</p></div>
        {/if}
      </div>
    {:else}
      <div class="tbl-wrap"><div class="state"><p>{$_('discovery.empty')}</p></div></div>
    {/if}
  </div>
</div>

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
    <div class="ctx-header m">{ctxMenu.ip}</div>
    <button class="ctx-item" type="button" role="menuitem" onclick={ctxAddPingMonitor}>{$_('discovery.ctx.add_ping')}</button>
    <button class="ctx-item" type="button" role="menuitem" onclick={ctxScanPorts}>{$_('discovery.ctx.scan_ports')}</button>
  </div>
{/if}

<style>
  .ctx-menu {
    position: fixed; z-index: var(--z-dropdown); min-width: 220px; padding: var(--space-2xs);
    background: var(--color-paper-4); border: 1px solid var(--color-rule-2);
    border-radius: var(--radius-ctl); box-shadow: var(--shadow-pop);
  }
  .ctx-header { padding: var(--space-2xs) var(--space-xs); color: var(--color-muted); font-size: var(--text-xs); }
  .ctx-item { display: block; width: 100%; padding: var(--space-xs); border: 0; border-radius: 3px; background: none; text-align: left; cursor: pointer; }
  .ctx-item:hover, .ctx-item:focus-visible { background: var(--color-accent-wash); color: var(--color-accent-text); }
</style>
