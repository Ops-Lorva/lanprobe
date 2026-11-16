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
  import { locale } from 'svelte-i18n';
  import ScreenHead from './ScreenHead.svelte';
  import StateMark from './StateMark.svelte';
  import LatencySpark from './LatencySpark.svelte';
  import NavIcon from './NavIcon.svelte';
  import SchedulerControl from './SchedulerControl.svelte';
  import { internetStatus } from '../stores/internetStatus';
  import { hubStatus } from '../stores/hub';
  import { hubMark, internetMark, targetMark } from '../status';
  import { fmtNum, targetStats } from '../format';
  import { screenKind, navigateFn } from '../screen';

  const isWeb = typeof window !== 'undefined' && (window as any).__LANPROBE_WEB__ === true;
  const isHeadless = typeof window !== 'undefined' && (window as any).__LANPROBE_HEADLESS__ === true;
  const readOnly = isWeb && !isHeadless;

  let interfaces = $state<string[]>([]);
  let selected = $state<string>($selectedInterface);
  let activeInterface = $state<InterfaceDetails | null>(null);
  // L'interface choisie n'a plus de lien : elle sort de la liste mais reste
  // proposée, en tête, marquée déconnectée.
  const selectedDown = $derived(!!selected && !interfaces.includes(selected));
  const ifaceOptions = $derived(selectedDown ? [selected, ...interfaces] : interfaces);
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
    // ⚠️ On oublie la valeur AVANT d'interroger.
    //
    // En cas d'échec, l'ancienne restait affichée : on passait de l'Ethernet
    // au Wi-Fi coupé, la requête échouait, et l'écran continuait de montrer
    // l'adresse publique de l'Ethernet. Une adresse plausible, attribuée à la
    // mauvaise interface — pire qu'un champ vide, parce que personne ne la
    // remet en cause.
    publicIp = null;
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
      // 🔴 **Repasser en DHCP, c'est changer de réseau.** Les surveillances en
      // cours visaient les adresses du plan précédent : elles ne mesurent plus
      // rien de réel et passeraient toutes en rouge, fabriquant une panne qui
      // n'existe pas. Même geste que l'application d'un profil réseau, pour la
      // même raison.
      //
      // ⚠️ Rien ne redémarre derrière : contrairement à un profil, le DHCP ne
      // porte aucune liste de cibles. On s'arrête, on ne devine pas.
      //
      // ⚠️ `cmd_stop_ping` et pas un simple retrait d'affichage : le retrait
      // doit être ÉCRIT, sinon le hub le réapplique au battement suivant et la
      // cible revient toute seule.
      for (const ip of Array.from($monitoring.keys())) {
        monitoring.removeHost(ip);
        try { await invoke('cmd_stop_ping', { ip }); }
        catch (e) { console.error('[Dashboard] cmd_stop_ping failed for', ip, e); }
      }
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
    // La liste ne porte que les liens actifs. L'interface choisie qui tombe
    // reste sélectionnée (marquée déconnectée) : on ne bascule pas en douce
    // sur une autre, la mesure changerait de lien sans qu'on le voie.
    if (!selected) {
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

  // Rattachement à un hub : relevé partagé (`stores/hub`). Le rattachement est
  // optionnel, son absence n'est pas un défaut à signaler.
  // ⚠️ `refused_status` : `null` = rien n'a répondu, un code = on a répondu et
  // on a refusé. Sans lui, un 403 renvoyé par un proxy devant un hub en pleine
  // forme s'affichait « Hub injoignable », et on cherchait une panne réseau.
  onMount(() => hubStatus.watch());
  const hub = $derived($hubStatus);

  // Le dernier test de débit, pour le panneau Internet.
  let lastSpeed = $state<{ download_mbps: number; upload_mbps: number } | null>(null);
  onMount(async () => {
    try {
      const snap = await invoke<{ latest: { download_mbps: number; upload_mbps: number } | null }>('cmd_get_speedtest_snapshot');
      lastSpeed = snap?.latest ?? null;
    } catch { /* aucun test encore */ }
  });

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
      // Un choix enregistré est gardé même si son lien est tombé.
      selected = $selectedInterface || (interfaces[0] ?? '');
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
  // Dans une case de la grille (une page), le tableau de bord se résume au
  // relevé chiffré et à l'interface : le reste fait doublon avec les cases
  // voisines (Découverte, Surveillance…).
  const kind = screenKind();
  const isCell = kind === 'cell';
  const navigate = navigateFn();
  const lang = $derived($locale ?? 'en');

  // La passerelle, si elle est surveillée : sa latence est le premier chiffre
  // qu'on regarde quand « le réseau rame ».
  const gatewayMs = $derived.by(() => {
    const gw = activeInterface?.gateway;
    return gw ? ($monitoring.get(gw)?.current?.latency_ms ?? null) : null;
  });
  const beatAgo = $derived.by(() => {
    if (!hub.last_beat_at) return null;
    return Math.max(0, Math.floor(Date.now() / 1000 - hub.last_beat_at));
  });

  // Le relevé chiffré, tiré du store de découverte réseau
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

<div class="screen">
  <ScreenHead title={$_('dashboard.title')}>
    {#snippet acts()}
      {#if ifaceOptions.length > 1 && !readOnly}
        <select class="select" style="width:auto" value={selected} onchange={onSelect} aria-label={$_('dashboard.interface')}>
          {#each ifaceOptions as name}
            <option value={name}>{name === selected && selectedDown ? `${name} (${$_('dashboard.iface_disconnected')})` : name}</option>
          {/each}
        </select>
      {/if}
      <button class="btn icon" class:sm={isCell} type="button" onclick={refresh} disabled={loading}
              aria-label={$_('dashboard.refresh')} title={$_('dashboard.refresh')} aria-busy={loading}>
        <NavIcon name="refresh" />
      </button>
    {/snippet}
  </ScreenHead>

  <div class="screen-body">
    {#if status}<div class="banner info" role="status"><span class="grow">{status}</span></div>{/if}

    <dl class="rail">
      <div><dt>{$_('dashboard.rail.hosts')}</dt><dd>{hostsCount}</dd></div>
      {#if downCount > 0}
        <div class="bad"><dt>{$_('dashboard.rail.silent')}</dt><dd><StateMark mark={targetMark(false)} label={String(downCount)} /></dd></div>
      {/if}
      <div class:ok={pingHosts.length > 0 && aliveCount === pingHosts.length}
           class:warn={aliveCount < pingHosts.length && aliveCount > 0}
           class:bad={pingHosts.length > 0 && aliveCount === 0}>
        <dt>{$_('dashboard.rail.targets')}</dt>
        <dd class="num">{aliveCount}/{pingHosts.length}</dd>
      </div>
      {#if !isCell}
        <div><dt>{$_('dashboard.rail.avg_rtt')}</dt><dd>{avgRtt != null ? avgRtt : '—'}{#if avgRtt != null}<span class="u">ms</span>{/if}</dd></div>
      {/if}
      {#if gatewayMs != null}
        <div><dt>{$_('dashboard.gateway')}</dt><dd>{fmtNum(gatewayMs, lang)}<span class="u">ms</span></dd></div>
      {/if}
      {#if !isCell}
        <div><dt>{$_('dashboard.rail.profiles')}</dt><dd>{$profiles.length}</dd></div>
      {/if}
    </dl>

    {#if isCell}
      {#if activeInterface}
        <dl class="kv">
          <dt>{$_('dashboard.interface')}</dt>
          <dd>{activeInterface.name}{#if selectedDown} <span class="err-text">{$_('dashboard.iface_disconnected')}</span>{/if}{#if activeProfile} <span class="tag">{activeProfile.label}</span>{/if}</dd>
          <dt>{$_('dashboard.ip')}</dt><dd class="m">{activeInterface.ip ?? '—'}{activeInterface.subnet ? ` / ${activeInterface.subnet}` : ''}</dd>
          <dt>{$_('dashboard.gateway')}</dt><dd class="m">{activeInterface.gateway ?? '—'}</dd>
          <dt>{$_('dashboard.dns')}</dt><dd class="m">{activeInterface.dns.join(', ') || '—'}</dd>
          <dt>{$_('dashboard.public_ip')}</dt><dd class="m">{publicIpLoading ? '…' : (publicIp?.ip ?? '—')}</dd>
          <dt>{$_('dashboard.hub')}</dt><dd><StateMark mark={hubMark(hub)} /></dd>
        </dl>
      {:else}
        <p class="muted">{$_('dashboard.no_interface')}</p>
      {/if}
    {:else}
      <div class="grid3">
        <section class="panel" aria-labelledby="h-if">
          <h2 id="h-if">{$_('dashboard.interface')}</h2>
          {#if activeInterface}
            <p class="psub">
              {activeInterface.name}{#if activeProfile}, {activeProfile.label}{/if}
              {#if selectedDown}<span class="err-text"> · {$_('dashboard.iface_disconnected')}</span>{/if}
            </p>
            <dl class="kv">
              <dt>{$_('dashboard.ip')}</dt><dd class="m">{activeInterface.ip ?? '—'}</dd>
              <dt>{$_('dashboard.mask')}</dt><dd class="m">{activeInterface.subnet ?? '—'}</dd>
              <dt>{$_('dashboard.gateway')}</dt><dd class="m">{activeInterface.gateway ?? '—'}</dd>
              <dt>{$_('dashboard.dns')}</dt><dd class="m">{activeInterface.dns.join(', ') || '—'}</dd>
              <dt>{$_('dashboard.dhcp')}</dt>
              <dd>
                {#if activeInterface.dhcp_enabled}
                  <StateMark mark={{ tone: 'ok', shape: 'dot', key: 'dashboard.dhcp_active' }} />
                {:else if readOnly}
                  {$_('dashboard.dhcp_no')}
                {:else}
                  <button class="btn sm" type="button" onclick={enableDhcp} disabled={dhcpBusy} aria-busy={dhcpBusy}>
                    {dhcpBusy ? '…' : $_('dashboard.dhcp_enable')}
                  </button>
                {/if}
              </dd>
            </dl>
          {:else}
            <p class="psub">{$_('dashboard.no_interface')}</p>
          {/if}
        </section>

        <section class="panel" aria-labelledby="h-in">
          <h2 id="h-in">{$_('dashboard.internet')}</h2>
          <p class="psub"><StateMark mark={internetMark($internetStatus?.state)} /></p>
          <dl class="kv">
            <dt>{$_('dashboard.public_ip')}</dt>
            <dd>
              {#if publicIpLoading}<span class="muted">…</span>
              {:else if publicIp}
                <span class="m">{publicIp.ip}</span>
                {#if publicIp.city || publicIp.country || publicIp.isp}
                  <span class="muted"> · {[[publicIp.city, publicIp.country].filter(Boolean).join(', '), publicIp.isp].filter(Boolean).join(', ')}</span>
                {/if}
              {:else}<span class="muted" title={publicIpError || undefined}>—</span>{/if}
            </dd>
            {#if $internetStatus}
              <dt>{$_('dashboard.availability')}</dt><dd class="num">{fmtNum($internetStatus.uptime_pct, lang, 1)} %</dd>
            {/if}
            {#if lastSpeed}
              <dt>{$_('dashboard.last_speedtest')}</dt>
              <dd class="num">{Math.round(lastSpeed.download_mbps)} / {Math.round(lastSpeed.upload_mbps)}<span class="u">Mbit/s</span></dd>
            {/if}
          </dl>
        </section>

        <section class="panel ticks" aria-labelledby="h-hub">
          <h2 id="h-hub">{$_('dashboard.hub')}</h2>
          <!-- Trois états, trois conduites (voir `status.ts`) : injoignable se
               répare seul, révoquée jamais, refusée se cherche côté proxy. -->
          <p class="psub" title={hub.last_error ? `${hub.url} — ${hub.last_error}` : hub.url || undefined}>
            <StateMark mark={hubMark(hub)} />
          </p>
          {#if hub.enrolled}
            <dl class="kv">
              <dt>{$_('dashboard.hub')}</dt><dd>{hub.url.replace(/^https?:\/\//, '')}</dd>
              {#if hub.site}<dt>{$_('settings.hub.site')}</dt><dd>{hub.site}</dd>{/if}
              <dt>{$_('dashboard.heartbeat')}</dt>
              <dd class="num">
                {beatAgo != null ? $_('dashboard.heartbeat_ago', { values: { n: beatAgo } }) : '—'}{#if hub.heartbeat_interval_secs}, {$_('dashboard.heartbeat_every', { values: { n: hub.heartbeat_interval_secs } })}{/if}
              </dd>
              <dt>{$_('dashboard.buffer')}</dt>
              <dd>{hub.buffered_points > 0 ? $_('dashboard.hub_buffered', { values: { n: hub.buffered_points } }) : $_('dashboard.buffer_empty')}</dd>
            </dl>
          {:else}
            <p class="muted">{$_('dashboard.hub_none')}</p>
          {/if}
        </section>
      </div>

      <div class="sec-head">
        <h2 class="sec">{$_('nav.monitoring')}</h2>
        <span class="sub">{$_('dashboard.monitoring_window')}</span>
        {#if navigate}<span class="acts"><button class="link" type="button" onclick={() => navigate('monitoring')}>{$_('dashboard.see_all')}</button></span>{/if}
      </div>
      {#if pingHosts.length === 0}
        <div class="tbl-wrap"><div class="state"><p>{$_('dashboard.no_targets')}</p></div></div>
      {:else}
        <div class="tbl-wrap">
          <table class="t">
            <thead><tr><th>{$_('monitoring.col_target')}</th><th>{$_('monitoring.col_state')}</th><th>{$_('monitoring.col_recent')}</th><th class="num">{$_('monitoring.col_latency')}</th><th class="num">{$_('monitoring.col_loss')}</th></tr></thead>
            <tbody>
              {#each pingHosts as h, i (h.ip)}
                {@const st = targetStats(h.history)}
                <tr class:trouble={h.current && !h.current.alive}>
                  <td><span class="nm m">{h.ip}</span></td>
                  <td><StateMark mark={targetMark(h.current?.alive)} /></td>
                  <td><LatencySpark history={h.history} series={(i % 3) + 1} /></td>
                  <td class="num">{h.current?.alive ? fmtNum(h.current.latency_ms, lang) : '—'}{#if h.current?.alive}<span class="u">ms</span>{/if}</td>
                  <td class="num">{fmtNum(st.loss, lang, 0)}{#if st.loss != null}<span class="u">%</span>{/if}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}

      <div class="sec-head"><h2 class="sec">{$_('scheduler.auto_run')}</h2></div>
      <div class="setrow" style="border:0;padding-top:0">
        <div><div class="k">{$_('nav.discovery')}</div><div class="d">{$_('dashboard.auto_hint')}</div></div>
        <SchedulerControl field="discovery_interval_min" />
      </div>
      <div class="setrow" style="border:0;padding-top:0">
        <div><div class="k">{$_('nav.speedtest')}</div></div>
        <SchedulerControl field="speedtest_interval_min" />
      </div>

      <div class="sec-head">
        <h2 class="sec">{$_('dashboard.sections.live_hosts')}</h2>
        <span class="sub">{$_('discovery.hosts_count', { values: { n: hostList.length } })}</span>
      </div>
      {#if hostList.length === 0}
        <div class="tbl-wrap"><div class="state"><p>{$_('dashboard.sections.no_hosts')}</p></div></div>
      {:else}
        <div class="tbl-wrap">
          <table class="t">
            <thead><tr><th>{$_('discovery.table.ip')}</th><th>{$_('discovery.table.hostname')}</th><th class="opt">{$_('discovery.table.mac')}</th><th class="opt">{$_('discovery.table.vendor')}</th><th class="num">{$_('discovery.table.latency')}</th></tr></thead>
            <tbody>
              {#each hostList as h (h.ip)}
                <tr>
                  <td class="m">{h.ip}</td>
                  <td>{h.hostname ?? '—'}</td>
                  <td class="m muted opt">{h.mac ?? '—'}</td>
                  <td class="opt">{h.vendor ?? '—'}</td>
                  <td class="num">{h.latency_ms != null ? `${fmtNum(h.latency_ms, lang)} ms` : '—'}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    {/if}
  </div>
</div>
