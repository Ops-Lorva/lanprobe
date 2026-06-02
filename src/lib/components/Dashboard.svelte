<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { _ } from 'svelte-i18n';
  import { profiles } from '../stores/profiles';
  import { monitoring } from '../stores/monitoring';
  import { discoveryStore } from '../stores/discovery';
  import { selectedInterface } from '../stores/selectedInterface';
  import { settings } from '../stores/settings';
  import { api, type InterfaceDetails } from '../tauri';
  import StatRail from './StatRail.svelte';
  import HostRow from './HostRow.svelte';

  const isWeb = typeof window !== 'undefined' && (window as any).__LANPROBE_WEB__ === true;
  const isHeadless = typeof window !== 'undefined' && (window as any).__LANPROBE_HEADLESS__ === true;
  const readOnly = isWeb && !isHeadless;

  let interfaces = $state<string[]>([]);
  let selected = $state<string>($selectedInterface);
  let activeInterface = $state<InterfaceDetails | null>(null);
  let loading = $state(false);
  let dhcpBusy = $state(false);
  let status = $state('');
  let statusTimer: ReturnType<typeof setTimeout> | null = null;

  // Le message DHCP ("appliqué sur eth0") doit disparaître tout seul après
  // 5s — sinon il reste épinglé en haut du dashboard jusqu'au prochain clic.
  function flashStatus(msg: string) {
    status = msg;
    if (statusTimer) clearTimeout(statusTimer);
    statusTimer = setTimeout(() => { status = ''; statusTimer = null; }, 5000);
  }

  interface PublicIpInfo { ip: string; country: string | null; city: string | null; isp: string | null; }
  let publicIp = $state<PublicIpInfo | null>(null);
  let publicIpLoading = $state(false);
  let publicIpError = $state('');

  async function loadPublicIp() {
    publicIpLoading = true;
    publicIpError = '';
    try {
      publicIp = await invoke<PublicIpInfo>('cmd_get_public_ip');
    } catch (e) {
      publicIpError = String(e);
    } finally {
      publicIpLoading = false;
    }
  }

  async function enableDhcp() {
    if (!selected) return;
    dhcpBusy = true;
    status = '';
    if (statusTimer) { clearTimeout(statusTimer); statusTimer = null; }
    try {
      await api.applyDhcp(selected);
      try { await invoke('cmd_reset_internet_monitor'); } catch {}
      flashStatus($_('dashboard.dhcp_applied', { values: { iface: selected } }));
      await loadDetails(selected);
    } catch (e) {
      flashStatus(`${$_('common.error')}: ${e}`);
    } finally {
      dhcpBusy = false;
    }
  }

  async function loadDetails(name: string) {
    if (!name) return;
    loading = true;
    try {
      activeInterface = await invoke<InterfaceDetails>('cmd_get_interface_details', { name });
    } finally {
      loading = false;
    }
  }

  async function onSelect(e: Event) {
    const name = (e.target as HTMLSelectElement).value;
    selected = name;
    await selectedInterface.select(name);
    await loadDetails(name);
    loadPublicIp();
  }

  async function refresh() {
    // Refresh efface aussi le flash status : les messages "DHCP appliqué"
    // ou "erreur" deviennent caducs dès qu'on recharge l'état.
    status = '';
    if (statusTimer) { clearTimeout(statusTimer); statusTimer = null; }
    interfaces = await invoke<string[]>('cmd_list_interfaces');
    if (!interfaces.includes(selected)) {
      selected = interfaces[0] ?? '';
      await selectedInterface.select(selected);
    }
    await loadDetails(selected);
    loadPublicIp();
  }

  let refreshTimer: ReturnType<typeof setInterval> | null = null;

  function restartRefreshTimer(sec: number) {
    if (refreshTimer) { clearInterval(refreshTimer); refreshTimer = null; }
    if (sec > 0) {
      refreshTimer = setInterval(() => { if (selected) loadDetails(selected); }, sec * 1000);
    }
  }

  onMount(async () => {
    await profiles.init();
    interfaces = await invoke<string[]>('cmd_list_interfaces');
    if (isWeb && !isHeadless) {
      // Web desktop : on ne force jamais la sélection. Le desktop est la
      // source de vérité — on lit ce qu'il a choisi et on se cale
      // dessus (le store a déjà hydraté depuis cmd_get_selected_interface).
      const backend = await invoke<string | null>('cmd_get_selected_interface');
      selected = backend ?? $selectedInterface ?? (interfaces[0] ?? '');
    } else {
      // Desktop ou headless : on choisit l'interface et on le pousse au backend.
      selected = interfaces.includes($selectedInterface) ? $selectedInterface : (interfaces[0] ?? '');
      await selectedInterface.select(selected);
    }
    await loadDetails(selected);
    loadPublicIp();
  });

  // Rafraichissement auto configurable (settings). Redemarre le timer des
  // que la valeur change. Egalement redeclenche a chaque montage de la page.
  $effect(() => { restartRefreshTimer($settings.dashboardRefreshSec); });

  // Synchro bi-directionnelle : si le backend diffuse un changement
  // d'interface (event `interface:selected` capturé par le store), on
  // recharge les détails pour refléter la nouvelle sélection sans que
  // l'utilisateur ait à rafraîchir. Couvre le cas où le desktop change
  // d'interface et on veut voir le même Dashboard côté web.
  $effect(() => {
    const name = $selectedInterface;
    if (name && name !== selected) {
      selected = name;
      loadDetails(name);
      loadPublicIp();
    }
  });

  onDestroy(() => { if (refreshTimer) clearInterval(refreshTimer); });

  const pingHosts = $derived([...$monitoring.values()]);
  const aliveCount = $derived(pingHosts.filter(h => h.current?.alive).length);
  // En single la mini-card monitoring est redondante avec le composant
  // Monitoring affiché juste à côté dans la grille, on la masque.
  const showMonitoringCard = $derived($settings.layout !== 'single');
  // En single, tout le dashboard se résume à StatRail + active interface :
  // les autres blocs (profiles card, monitoring mini, live hosts) font
  // doublon avec les autres cells de la grille (Discovery, Monitoring…).
  const isSingle = $derived($settings.layout === 'single');

  // Métriques dérivées pour StatRail depuis le store de découverte réseau
  const discoveredHosts = $derived(Array.from($discoveryStore.results.values()));
  const hostsCount = $derived(discoveredHosts.length);
  const downCount = $derived(discoveredHosts.filter(h => h.latency_ms == null).length);
  const avgRtt = $derived.by(() => {
    const alive = discoveredHosts
      .map(h => h.latency_ms)
      .filter((v): v is number => typeof v === 'number');
    if (!alive.length) return null;
    return Math.round(alive.reduce((a, b) => a + b, 0) / alive.length);
  });
  const hostList = $derived(
    [...discoveredHosts].sort((a, b) => {
      const toNum = (ip: string) => ip.split('.').reduce((acc, p) => acc * 256 + +p, 0);
      return toNum(a.ip) - toNum(b.ip);
    })
  );

  // Détermine si la conf actuelle de l'interface correspond à un profil
  // enregistré. On compare IP / subnet / gateway / DNS primaire — si tout
  // matche exactement un profil de cette interface, on l'affiche comme tel.
  // Sinon : "DHCP" si l'interface est en DHCP, ou "Custom".
  const activeProfile = $derived.by(() => {
    if (!activeInterface) return null;
    if (activeInterface.dhcp_enabled) return { label: $_('dashboard.dhcp'), kind: 'dhcp' as const };
    const match = $profiles.find(p =>
      p.ip === (activeInterface!.ip ?? '') &&
      p.subnet === (activeInterface!.subnet ?? '') &&
      p.gateway === (activeInterface!.gateway ?? '') &&
      p.dns_primary === (activeInterface!.dns[0] ?? '')
    );
    if (match) return { label: match.name, kind: 'profile' as const };
    return { label: $_('dashboard.profile_custom'), kind: 'custom' as const };
  });
</script>

<div class="page">
  <div class="page-header">
    <h1>{$_('dashboard.title')}</h1>
    <button onclick={refresh} disabled={loading}>{loading ? '…' : $_('dashboard.refresh')}</button>
  </div>

  <StatRail
    interfaceName={activeInterface?.name ?? selected}
    hosts={hostsCount}
    down={downCount}
    avgRttMs={avgRtt}
    downMbps={null}
    upMbps={null}
  />

  <div class="grid" class:grid-compact={!showMonitoringCard} class:grid-single={isSingle}>
    <div class="card wide">
      <div class="card-label-row">
        <span class="card-label">{$_('dashboard.active_interface')}</span>
        {#if interfaces.length > 1 && !readOnly}
          <select class="iface-select" value={selected} onchange={onSelect}>
            {#each interfaces as name}
              <option value={name}>{name}</option>
            {/each}
          </select>
        {/if}
      </div>
      {#if activeInterface}
        <div class="card-title-row">
          <div class="title-with-badge">
            <div class="card-title">{activeInterface.name}</div>
            {#if activeProfile}
              <span class="profile-badge {activeProfile.kind}">{activeProfile.label}</span>
            {/if}
          </div>
          {#if !readOnly}
            <button class="dhcp-btn" onclick={enableDhcp} disabled={dhcpBusy || activeInterface.dhcp_enabled}>
              {dhcpBusy ? '…' : (activeInterface.dhcp_enabled ? $_('dashboard.dhcp_active') : $_('dashboard.dhcp_enable'))}
            </button>
          {/if}
        </div>
        <div class="info-grid">
          <div class="info-item"><span class="info-label">{$_('dashboard.ip')}</span><span class="info-val accent">{activeInterface.ip ?? '—'}</span></div>
          <div class="info-item"><span class="info-label">{$_('dashboard.mask')}</span><span class="info-val">{activeInterface.subnet ?? '—'}</span></div>
          <div class="info-item"><span class="info-label">{$_('dashboard.gateway')}</span><span class="info-val">{activeInterface.gateway ?? '—'}</span></div>
          <div class="info-item"><span class="info-label">{$_('dashboard.dns')}</span><span class="info-val">{activeInterface.dns.join(', ') || '—'}</span></div>
          <div class="info-item"><span class="info-label">{$_('dashboard.dhcp')}</span><span class="info-val">{activeInterface.dhcp_enabled ? $_('dashboard.dhcp_yes') : $_('dashboard.dhcp_no')}</span></div>
          <div class="info-item public-ip-item">
            <span class="info-label">Public IP</span>
            {#if publicIpLoading}
              <span class="info-val muted-val">…</span>
            {:else if publicIp}
              <span class="info-val accent">{publicIp.ip}</span>
              {#if publicIp.city || publicIp.country}
                <span class="info-sub">{[publicIp.city, publicIp.country].filter(Boolean).join(', ')}{publicIp.isp ? ` · ${publicIp.isp}` : ''}</span>
              {/if}
            {:else if publicIpError}
              <span class="info-val muted-val" title={publicIpError}>—</span>
            {:else}
              <span class="info-val muted-val">—</span>
            {/if}
          </div>
        </div>
        {#if status}<div class="status-line">{status}</div>{/if}
      {:else}
        <p class="muted">{$_('dashboard.no_interface')}</p>
      {/if}
    </div>

    {#if !isSingle}
    <div class="card">
      <div class="card-label">{$_('dashboard.profiles_card')}</div>
      <div class="big-num">{$profiles.length}</div>
      <div class="muted">{$_('dashboard.profiles_saved')}</div>
    </div>
    {/if}

    {#if showMonitoringCard}
      <div class="card">
        <div class="card-label">{$_('dashboard.monitoring_card')}</div>
        <div class="big-num" class:ok={aliveCount > 0 && aliveCount === pingHosts.length} class:warn={aliveCount > 0 && aliveCount < pingHosts.length} class:bad={pingHosts.length > 0 && aliveCount === 0}>
          {aliveCount}<span class="big-num-sub">/{pingHosts.length}</span>
        </div>
        <div class="muted">{$_('dashboard.ping_online')}</div>
        {#each pingHosts.slice(0, 3) as h}
          <div class="ping-row">
            <span class="dot" class:green={h.current?.alive} class:red={h.current && !h.current.alive}></span>
            <span class="mono">{h.ip}</span>
            <span class="muted">{h.current?.latency_ms != null ? h.current.latency_ms + 'ms' : '—'}</span>
          </div>
        {/each}
      </div>
    {/if}
  </div>

  {#if !isSingle}
  <section class="hosts-section">
    <h2>{$_('dashboard.sections.live_hosts')}</h2>
    {#if hostList.length === 0}
      <p class="empty">{$_('dashboard.sections.no_hosts')}</p>
    {:else}
      <div class="table">
        {#each hostList as h (h.ip)}
          <HostRow
            ip={h.ip}
            hostname={h.hostname}
            mac={h.mac}
            vendor={h.vendor}
            latencyMs={h.latency_ms ?? null}
            history={[]}
          />
        {/each}
      </div>
    {/if}
  </section>
  {/if}
</div>

<style>
  .page { padding: 24px; }
  .page-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 24px; }
  h1 { font-size: 20px; font-weight: 700; }
  button { padding: 6px 14px; border-radius: 6px; border: 1px solid var(--ep-border); background: var(--ep-bg-tertiary); color: var(--ep-text-primary); cursor: pointer; font-size: 12px; }
  button:disabled { opacity: .5; cursor: wait; }
  .grid { display: grid; grid-template-columns: 2fr 1fr 1fr; gap: 16px; }
  .grid.grid-compact { grid-template-columns: 2fr 1fr; }
  .grid.grid-single { grid-template-columns: 1fr; }
  .card {
    background: var(--ep-glass-bg);
    border: 1px solid var(--ep-glass-border);
    border-radius: var(--ep-radius-lg);
    padding: 18px;
    transition: border-color 0.15s;
  }
  .card:hover { border-color: var(--ep-glass-border-strong); }
  .card-label-row { display: flex; align-items: center; justify-content: space-between; margin-bottom: 10px; }
  .card-label { font-size: 11px; text-transform: uppercase; letter-spacing: .5px; color: var(--ep-text-muted); }
  .iface-select {
    background: var(--ep-bg-tertiary);
    border: 1px solid var(--ep-glass-border);
    color: var(--ep-text-primary);
    font-size: 12px;
    font-weight: 500;
    padding: 4px 10px;
    border-radius: 6px;
    cursor: pointer;
    max-width: 180px;
  }
  .iface-select:focus { outline: none; border-color: var(--ep-accent); }
  .card-title-row { display: flex; justify-content: space-between; align-items: center; margin-bottom: 12px; gap: 12px; }
  .title-with-badge { display: flex; align-items: center; gap: 10px; min-width: 0; }
  .card-title { font-size: 15px; font-weight: 700; }
  .profile-badge {
    font-size: 10px; font-weight: 700; text-transform: uppercase; letter-spacing: .4px;
    padding: 3px 8px; border-radius: 10px; border: 1px solid transparent;
    white-space: nowrap;
  }
  .profile-badge.dhcp { background: color-mix(in srgb, var(--ep-accent) 15%, transparent); color: var(--ep-accent); border-color: color-mix(in srgb, var(--ep-accent) 30%, transparent); }
  .profile-badge.profile { background: color-mix(in srgb, var(--ep-success) 15%, transparent); color: var(--ep-success); border-color: color-mix(in srgb, var(--ep-success) 30%, transparent); }
  .profile-badge.custom { background: var(--ep-bg-tertiary); color: var(--ep-text-muted); border-color: var(--ep-border); }
  .dhcp-btn { padding: 5px 12px; border-radius: 6px; border: 1px solid var(--ep-border); background: var(--ep-bg-tertiary); color: var(--ep-text-primary); font-size: 11px; font-weight: 600; cursor: pointer; }
  .dhcp-btn:hover:not(:disabled) { border-color: var(--ep-accent); color: var(--ep-accent); }
  .dhcp-btn:disabled { opacity: .5; cursor: not-allowed; }
  .status-line { margin-top: 10px; font-size: 12px; color: var(--ep-success); font-family: var(--ep-font-mono); }
  .info-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }
  .info-item { display: flex; flex-direction: column; gap: 2px; }
  .info-label { font-size: 10px; text-transform: uppercase; letter-spacing: .5px; color: var(--ep-text-muted); }
  .info-val { font-family: var(--ep-font-mono); font-size: 12px; font-weight: 600; }
  .info-val.accent { color: var(--ep-accent); }
  .info-val.muted-val { color: var(--ep-text-muted); }
  .info-sub { font-size: 10px; color: var(--ep-text-muted); font-family: var(--ep-font-mono); margin-top: 1px; }
  .public-ip-item { grid-column: span 2; }
  .big-num { font-size: 40px; font-weight: 800; line-height: 1.1; }
  .big-num.ok { color: var(--ep-success); }
  .big-num.warn { color: #f59e0b; }
  .big-num.bad { color: var(--ep-danger); }
  .big-num-sub { font-size: 24px; color: var(--ep-text-muted); }
  .muted { font-size: 12px; color: var(--ep-text-secondary); margin-top: 4px; }
  .ping-row { display: flex; align-items: center; gap: 8px; margin-top: 8px; font-size: 12px; }
  .dot { width: 8px; height: 8px; border-radius: 50%; background: var(--ep-text-muted); flex-shrink: 0; }
  .dot.green { background: var(--ep-success); }
  .dot.red { background: var(--ep-danger); }
  .mono { font-family: var(--ep-font-mono); flex: 1; }
  .hosts-section { margin-top: 24px; }
  .hosts-section h2 {
    font-size: 11px; text-transform: uppercase; letter-spacing: .8px;
    color: var(--ep-text-dim); margin: 0 0 8px; font-weight: 600;
  }
  .table {
    background: var(--ep-glass-bg);
    border: 1px solid var(--ep-glass-border);
    border-radius: var(--ep-radius-lg);
    overflow: hidden;
    transition: border-color 0.15s;
  }
  .table:hover { border-color: var(--ep-glass-border-strong); }
  .empty {
    padding: 24px; text-align: center; color: var(--ep-text-muted);
    background: var(--ep-glass-bg); border: 1px solid var(--ep-glass-border);
    border-radius: var(--ep-radius-lg); font-size: 12px;
  }
</style>
