<script lang="ts">
  import { onMount } from 'svelte';
  import { _ } from 'svelte-i18n';
  import { portscan, type ScanEntry } from '../stores/portscan';
  import { portscanProfiles, type PortScanProfile } from '../stores/portscanProfiles';
  import { scheduler } from '../stores/scheduler';
  import { get } from 'svelte/store';
  import SchedulerControl from './SchedulerControl.svelte';
  import ScreenHead from './ScreenHead.svelte';
  import StateMark from './StateMark.svelte';
  import { discoveryStore } from '../stores/discovery';
  import { screenKind } from '../screen';
  import { locale } from 'svelte-i18n';

  const isCell = screenKind() === 'cell';
  const lang = $derived($locale ?? 'en');
  /** La machine dont le détail s'affiche à droite ; la première par défaut. */
  let selectedIp = $state<string | null>(null);
  const entries = $derived([...$portscan.values()]);
  const current = $derived(entries.find((e) => e.ip === selectedIp) ?? entries[0] ?? null);
  const hostOf = (ip: string) => $discoveryStore.results.get(ip);
  const hm = (ts: number) => new Date(ts).toLocaleTimeString(lang, { hour: '2-digit', minute: '2-digit' });
  const profileLabel = (p: PortScanProfile) =>
    $_('port_scan.profile_option', { values: { name: p.from_hub ? `★ ${p.name}` : p.name, t: p.tcp_ports.length, u: p.udp_ports.length } });

  let newIp = $state('');
  let showManager = $state(false);
  let editing = $state<PortScanProfile | null>(null);
  let editName = $state('');
  let editTcpText = $state('');
  let editUdpText = $state('');

  onMount(() => { portscanProfiles.init(); scheduler.init(); });

  let activeId = $state('builtin:common');
  portscanProfiles.active.subscribe(v => { activeId = v; });

  // 🔴 Les sélecteurs n'affichent PAS la liste complète : dès que la sonde est
  // rattachée à un hub, ses profils de base disparaissent de l'écran (§ 25,
  // décision du 02/10). Le hub sème les mêmes et fait autorité ; les montrer
  // tous les deux donnait trois paires de même nom aux contenus différents —
  // `Common` 16T/6U en local contre 0/0 au hub. Rien n'est supprimé : ils
  // resservent le jour où la sonde est désenrôlée.
  const visibleProfiles = portscanProfiles.visible;
  // Le profil actif parmi ceux qui s'affichent, repli compris : sinon une sonde
  // qu'on vient d'enrôler garderait un profil de base invisible comme actif.
  const currentProfile = portscanProfiles.current;
  const activeProfile = $derived($currentProfile);

  function parsePorts(text: string): number[] {
    const out = new Set<number>();
    for (const raw of text.split(/[,\s\n]+/)) {
      const t = raw.trim();
      if (!t) continue;
      const range = t.match(/^(\d+)-(\d+)$/);
      if (range) {
        const a = Math.max(1, Math.min(65535, parseInt(range[1], 10)));
        const b = Math.max(1, Math.min(65535, parseInt(range[2], 10)));
        for (let p = Math.min(a, b); p <= Math.max(a, b); p++) out.add(p);
      } else {
        const p = parseInt(t, 10);
        if (p >= 1 && p <= 65535) out.add(p);
      }
    }
    return [...out].sort((a, b) => a - b);
  }

  async function addHost() {
    const ip = newIp.trim();
    if (!ip) return;
    newIp = '';
    selectedIp = ip;
    await portscan.add(ip, activeProfile?.tcp_ports, activeProfile?.udp_ports, activeProfile?.id ?? null, activeProfile?.name ?? null);
    const sched = get(scheduler);
    scheduler.save({ ...sched, portscan_targets: [...new Set([...sched.portscan_targets, ip])] });
  }

  function removeHost(ip: string) {
    portscan.remove(ip);
    const sched = get(scheduler);
    scheduler.save({ ...sched, portscan_targets: sched.portscan_targets.filter(t => t !== ip) });
  }

  function rescan(entry: ScanEntry) {
    // ⚠️ Cherché parmi les profils AFFICHÉS : relancer avec un profil de base
    // que l'écran ne montre plus scannerait une liste de ports invisible.
    const prof = $visibleProfiles.find(p => p.id === entry.profileId) ?? activeProfile;
    portscan.add(entry.ip, prof?.tcp_ports, prof?.udp_ports, prof?.id ?? null, prof?.name ?? null);
  }

  function rescanWith(ip: string, profileId: string) {
    const prof = $visibleProfiles.find(p => p.id === profileId);
    if (!prof) return;
    portscan.add(ip, prof.tcp_ports, prof.udp_ports, prof.id, prof.name);
  }

  function formatScannedAt(ts: number | null): string {
    if (!ts) return '';
    const delta = Math.floor((Date.now() - ts) / 1000);
    if (delta < 5) return $_('port_scan.just_now');
    if (delta < 60) return `${delta}s`;
    if (delta < 3600) return `${Math.floor(delta / 60)}m`;
    if (delta < 86400) return `${Math.floor(delta / 3600)}h`;
    return `${Math.floor(delta / 86400)}d`;
  }

  function openNew() {
    editing = { id: `custom:${Date.now()}`, name: '', tcp_ports: [], udp_ports: [] };
    editName = '';
    editTcpText = '';
    editUdpText = '';
  }

  function openEdit(p: PortScanProfile) {
    editing = { ...p };
    editName = p.name;
    editTcpText = p.tcp_ports.join(', ');
    editUdpText = p.udp_ports.join(', ');
  }

  function saveEdit() {
    if (!editing || !editName.trim()) return;
    const next: PortScanProfile = {
      id: editing.id,
      name: editName.trim(),
      tcp_ports: parsePorts(editTcpText),
      udp_ports: parsePorts(editUdpText),
    };
    const exists = $portscanProfiles.some(p => p.id === next.id);
    if (exists) portscanProfiles.edit(next);
    else portscanProfiles.add(next);
    editing = null;
  }

  function removeProfile(p: PortScanProfile) {
    if (p.builtin) return;
    portscanProfiles.remove(p.id);
    // ⚠️ On ne retombe pas en dur sur `builtin:common` : il est invisible dès
    // que la sonde est rattachée à un hub. Le premier profil affiché, ou rien
    // du tout — et rien du tout veut dire « la sonde garde sa propre liste ».
    if (activeId === p.id) {
      const reste = $visibleProfiles.filter(x => x.id !== p.id);
      portscanProfiles.setActive(reste[0]?.id ?? 'builtin:common');
    }
  }

  function openCount(e: ScanEntry): number {
    return e.tcpResults.filter(r => r.open).length + e.udpResults.filter(r => r.open).length;
  }
</script>

{#snippet profileSelect(cls: string)}
  <select class="select {cls}" style="width:auto;max-width:220px" value={activeProfile?.id ?? activeId} aria-label={$_('port_scan.profile')}
          onchange={(e) => portscanProfiles.setActive((e.currentTarget as HTMLSelectElement).value)}>
    {#each $visibleProfiles as p (p.id)}
      <option value={p.id}>{profileLabel(p)}</option>
    {/each}
  </select>
{/snippet}

<div class="screen">
  <ScreenHead title={$_('port_scan.title')}>
    {#snippet acts()}
      {#if isCell}
        <button class="btn sm primary" type="button" onclick={addHost} disabled={!newIp.trim()}>{$_('port_scan.add')}</button>
      {:else}
        <input class="input m" style="width:200px" bind:value={newIp} placeholder={$_('port_scan.placeholder')}
               aria-label={$_('port_scan.placeholder')} onkeydown={(e) => e.key === 'Enter' && addHost()} />
        {@render profileSelect('')}
        <button class="btn primary" type="button" onclick={addHost} disabled={!newIp.trim()}>{$_('port_scan.add')}</button>
      {/if}
    {/snippet}
  </ScreenHead>

  <div class="screen-body">
    {#if isCell}
      <div class="input-row" style="margin-bottom:8px">
        <input class="input m" style="flex:1;min-width:120px" bind:value={newIp} placeholder={$_('port_scan.placeholder')}
               aria-label={$_('port_scan.placeholder')} onkeydown={(e) => e.key === 'Enter' && addHost()} />
        {@render profileSelect('')}
      </div>
    {/if}
    <div class="filters">
      <SchedulerControl field="portscan_interval_min" />
      <span class="grow"></span>
      <button class="btn sm quiet" type="button" onclick={() => showManager = true}>{$_('port_scan.manage_profiles')}</button>
    </div>

    {#if entries.length === 0}
      <div class="tbl-wrap"><div class="state"><p>{$_('port_scan.empty')}</p></div></div>
    {:else}
      <div class="split">
        <div class="tbl-wrap hosts">
          <table class="t">
            <tbody>
              {#each entries as entry (entry.ip)}
                {@const h = hostOf(entry.ip)}
                <tr class="click" class:sel={current?.ip === entry.ip} class:trouble={!!entry.error}
                    onclick={() => (selectedIp = entry.ip)}>
                  <td>
                    <button class="nm m rowbtn" type="button" aria-pressed={current?.ip === entry.ip}
                            onclick={(e) => { e.stopPropagation(); selectedIp = entry.ip; }}>{entry.ip}</button>
                    {#if h?.hostname}<span class="sub2">{h.hostname}</span>{/if}
                  </td>
                  <td class="num">
                    {#if entry.scanning}<span class="muted">{$_('port_scan.scanning')}</span>
                    {:else if entry.error}<StateMark mark={{ tone: 'bad', shape: 'square', key: 'port_scan.error' }} />
                    {:else}{$_('port_scan.open_count', { values: { n: openCount(entry) } })}{/if}
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>

        {#if current}
          {@const h = hostOf(current.ip)}
          {@const open = [...current.tcpResults, ...current.udpResults].filter(r => r.open)}
          {@const shut = current.tcpResults.length + current.udpResults.length - open.length}
          <section class="panel" aria-labelledby="h-ps">
            <div class="chart-head">
              <h2 id="h-ps" class="m">{current.ip}</h2>
              {#if h?.hostname || h?.vendor}<span class="muted">{[h.hostname, h.vendor].filter(Boolean).join(', ')}</span>{/if}
              <span class="acts">
                <button class="btn sm" type="button" onclick={() => rescan(current)} disabled={current.scanning}>{$_('port_scan.rescan')}</button>
                <button class="btn sm quiet" type="button" onclick={() => removeHost(current.ip)}>{$_('port_scan.remove')}</button>
              </span>
            </div>
            <p class="psub">
              {#if current.scanning}{$_('port_scan.pending')}
              {:else if current.scannedAt}
                {$_('port_scan.scanned_line', { values: {
                  at: hm(current.scannedAt), ago: formatScannedAt(current.scannedAt),
                  profile: current.profileName ?? '—', t: current.tcpResults.length, u: current.udpResults.length } })}
              {/if}
            </p>
            <div class="input-row" style="margin-bottom:12px">
              <label class="muted" for="rescan-with">{$_('port_scan.rescan_with')}</label>
              <select id="rescan-with" class="select" style="width:auto" value={$visibleProfiles.some((p) => p.id === current.profileId) ? current.profileId : (activeProfile?.id ?? activeId)} disabled={current.scanning}
                      onchange={(e) => rescanWith(current.ip, (e.currentTarget as HTMLSelectElement).value)}>
                {#each $visibleProfiles as p (p.id)}
                  <option value={p.id}>{profileLabel(p)}</option>
                {/each}
              </select>
            </div>
            {#if current.error}
              <div class="banner bad" role="alert"><span class="grow">{current.error}</span></div>
            {/if}
            {#if current.scanning && open.length === 0}
              <div class="skel" style="width:60%;margin:8px 0"></div>
              <div class="skel" style="width:40%"></div>
            {:else if open.length === 0}
              <p class="muted">{$_('port_scan.no_open')}</p>
            {:else}
              <div class="tbl-wrap" style="border:0">
                <table class="t">
                  <thead><tr><th>{$_('port_scan.col_port')}</th><th>{$_('port_scan.col_proto')}</th><th>{$_('port_scan.col_service')}</th><th>{$_('port_scan.col_state')}</th></tr></thead>
                  <tbody>
                    {#each open as r (r.proto + r.port)}
                      <tr>
                        <td class="m num">{r.port}</td>
                        <td>{r.proto.toUpperCase()}</td>
                        <td class="m">{r.service || '—'}</td>
                        <td><StateMark mark={{ tone: 'ok', shape: 'dot', key: 'port_scan.state_open' }} /></td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              </div>
            {/if}
            {#if !current.scanning && shut > 0}
              <p class="hint" style="margin-top:8px">{$_('port_scan.closed_count', { values: { n: shut } })}</p>
            {/if}
            {#if current.udpResults.length > 0}
              <p class="hint">{$_('port_scan.udp_hint')}</p>
            {/if}
          </section>
        {/if}
      </div>
    {/if}
  </div>
</div>

{#if showManager}
  <div class="overlay" role="presentation"
    onclick={() => { showManager = false; editing = null; }}
    onkeydown={(e) => { if (e.key === 'Escape') { showManager = false; editing = null; } }}>
    <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="h-psm" tabindex="-1"
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => e.stopPropagation()}>
      <div class="chart-head">
        <h2 id="h-psm">{editing ? ($portscanProfiles.some(p => p.id === editing?.id) ? $_('port_scan.edit_profile') : $_('port_scan.new_profile')) : $_('port_scan.manage_profiles')}</h2>
        <span class="acts"><button class="btn sm quiet" type="button" onclick={() => { showManager = false; editing = null; }}>{$_('common.close')}</button></span>
      </div>

      {#if editing}
        <div class="field">
          <label for="psp-name">{$_('port_scan.profile_name')}</label>
          <input id="psp-name" class="input" bind:value={editName} placeholder={$_('port_scan.profile_name_ph')} />
        </div>
        <div class="field">
          <label for="psp-tcp">{$_('port_scan.tcp_ports')}</label>
          <textarea id="psp-tcp" class="textarea m" bind:value={editTcpText} placeholder="22, 80, 443, 8000-8100"></textarea>
        </div>
        <div class="field">
          <label for="psp-udp">{$_('port_scan.udp_ports')}</label>
          <textarea id="psp-udp" class="textarea m" bind:value={editUdpText} placeholder="53, 161, 500"></textarea>
        </div>
        <div class="row">
          <button class="btn" type="button" onclick={() => editing = null}>{$_('port_scan.cancel')}</button>
          <button class="btn primary" type="button" onclick={saveEdit} disabled={!editName.trim()}>{$_('port_scan.save')}</button>
        </div>
      {:else}
        {#if $visibleProfiles.some(p => p.from_hub)}
          <p class="hint">{$_('port_scan.from_hub_hint')}</p>
        {/if}
        <div class="tbl-wrap">
          <table class="t">
            <tbody>
              {#each $visibleProfiles as p (p.id)}
                <tr>
                  <td>
                    <span class="nm">{p.from_hub ? `★ ${p.name}` : p.name}</span>
                    <!-- ⚠️ Le hub fait autorité sur ce profil : l'éditer ici
                         tiendrait jusqu'au prochain battement, pas au-delà. -->
                    <span class="sub2">{p.tcp_ports.length} TCP · {p.udp_ports.length} UDP{#if p.from_hub} · {$_('port_scan.from_hub')}{/if}</span>
                  </td>
                  <td class="act">
                    {#if !p.builtin}
                      <button class="btn sm quiet" type="button" onclick={() => openEdit(p)}>{$_('common.edit')}</button>
                      <button class="btn sm quiet" type="button" onclick={() => removeProfile(p)}>{$_('common.delete')}</button>
                    {:else}
                      <button class="btn sm quiet" type="button" onclick={() => openEdit({ ...p, id: `custom:${Date.now()}`, name: $_('port_scan.copy_of', { values: { name: p.name } }), builtin: false })}>{$_('port_scan.duplicate')}</button>
                    {/if}
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
        <div class="row"><button class="btn primary" type="button" onclick={openNew}>{$_('port_scan.new_profile')}</button></div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .rowbtn { border: 0; background: none; padding: 0; cursor: pointer; text-align: left; }
  .rowbtn:hover { text-decoration: underline; }
  .hosts { max-height: 70vh; overflow-y: auto; }
</style>
