<script lang="ts">
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { _ } from 'svelte-i18n';
  import { profiles, type Profile } from '../stores/profiles';
  import { selectedInterface } from '../stores/selectedInterface';
  import { monitoring } from '../stores/monitoring';
  import ProfileForm from './ProfileForm.svelte';
  import { api } from '../tauri';
  import { invoke } from '@tauri-apps/api/core';

  let showForm = $state(false);
  let editing = $state<Partial<Profile> | undefined>();
  let status = $state('');
  let statusTimer: ReturnType<typeof setTimeout> | null = null;

  // Auto-dismiss du message ("profil appliqué", "no iface", erreur…) après
  // 5s pour qu'il ne reste pas bloqué jusqu'au prochain clic.
  function flashStatus(msg: string) {
    status = msg;
    if (statusTimer) clearTimeout(statusTimer);
    statusTimer = setTimeout(() => { status = ''; statusTimer = null; }, 5000);
  }

  onMount(() => profiles.init());

  async function applyProfile(p: Profile) {
    const tr = get(_);
    const iface = $selectedInterface;
    if (!iface) { flashStatus(tr('profiles.no_iface')); return; }
    try {
      await api.applyStatic({ interface: iface, ip: p.ip, subnet: p.subnet, gateway: p.gateway, dns_primary: p.dns_primary, dns_secondary: p.dns_secondary || undefined });
      // Reset de l'historique internet — l'ancienne fenêtre d'uptime est
      // celle d'une config réseau qui n'existe plus (changement d'IP/gw).
      try { await invoke('cmd_reset_internet_monitor'); } catch {}
      flashStatus(tr('profiles.applied', { values: { name: p.name, iface } }));
      const wanted = new Set(p.monitor_ips ?? []);
      // 🔴 Les surveillances de l'ANCIEN profil s'arrêtent. Elles visaient le
      // réseau d'un autre site : après la bascule, elles ne mesurent plus rien
      // de réel et passent toutes en rouge — on fabrique une panne chez un
      // client qu'on a quitté. Même raison que la remise à zéro de l'historique
      // internet juste au-dessus.
      //
      // ⚠️ Ce qui est dans le NOUVEAU profil n'est pas touché : arrêter puis
      // relancer la même cible ferait un trou dans sa courbe pour rien.
      //
      // ⚠️ `cmd_stop_ping` et pas seulement un retrait d'affichage : le retrait
      // doit être ÉCRIT, sinon le hub le réapplique au battement suivant et la
      // cible revient toute seule.
      for (const ip of Array.from($monitoring.keys())) {
        if (wanted.has(ip)) continue;
        monitoring.removeHost(ip);
        try { await invoke('cmd_stop_ping', { ip }); }
        catch (e) { console.error('[Profiles] cmd_stop_ping failed for', ip, e); }
      }
      // Auto-ping : démarre un monitor pour chaque IP associée au profil,
      // skip celles déjà en cours pour ne pas doubler les tasks.
      for (const ip of wanted) {
        if ($monitoring.has(ip)) continue;
        monitoring.addHost(ip);
        try { await invoke('cmd_start_ping', { ip }); }
        catch (e) { console.error('[Profiles] cmd_start_ping failed for', ip, e); }
      }
    } catch (e) { flashStatus(`${tr('common.error')}: ${e}`); }
  }

  function openNew() { editing = undefined; showForm = true; }
  function openEdit(p: Profile) { editing = p; showForm = true; }
  function handleSave(p: Profile) {
    if (editing?.id) profiles.edit(p); else profiles.add(p);
    showForm = false;
  }
</script>

<div class="page">
  <div class="header">
    <h1>{$_('profiles.title')}</h1>
    <button class="primary" onclick={openNew}>{$_('profiles.new')}</button>
  </div>
  <div class="target-hint">
    {$_('profiles.target_label')}
    {#if $selectedInterface}
      <span class="iface-name">{$selectedInterface}</span>
      <span class="muted">{$_('profiles.target_selected_suffix')}</span>
    {:else}
      <span class="warn">{$_('profiles.target_missing')}</span>
    {/if}
  </div>
  {#if status}<div class="status">{status}</div>{/if}
  <div class="list">
    {#each $profiles as p (p.id)}
      <div class="card">
        <div class="info">
          <div class="name">{p.name}</div>
          <div class="meta">{p.ip} / {p.subnet}</div>
          <div class="meta">GW: {p.gateway} · DNS: {p.dns_primary}</div>
          {#if p.monitor_ips && p.monitor_ips.length > 0}
            <div class="meta monitor" title={p.monitor_ips.join('\n')}>📡 {$_('profiles.monitor_count', { values: { n: p.monitor_ips.length } })}</div>
          {/if}
        </div>
        <div class="actions">
          <button class="primary" onclick={() => applyProfile(p)}>{$_('profiles.apply')}</button>
          <button onclick={() => openEdit(p)}>{$_('profiles.edit')}</button>
          <button class="danger" onclick={() => profiles.remove(p.id)}>{$_('profiles.remove')}</button>
        </div>
      </div>
    {:else}
      <p class="empty">{$_('profiles.empty')}</p>
    {/each}
  </div>
  {#if showForm}
    <ProfileForm profile={editing} onSave={handleSave} onCancel={() => showForm = false} />
  {/if}
</div>

<style>
  .page { padding: 24px; }
  .header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 12px; }
  h1 { font-size: 20px; font-weight: 700; }
  .target-hint { font-size: 12px; padding: 8px 12px; background: var(--ep-bg-secondary); border: 1px solid var(--ep-border); border-radius: 6px; margin-bottom: 12px; }
  .target-hint .iface-name { font-family: var(--ep-font-mono); font-weight: 700; color: var(--ep-accent); }
  .target-hint .muted { color: var(--ep-text-muted); }
  .target-hint .warn { color: var(--ep-danger); font-weight: 600; }
  .status { padding: 8px 12px; background: var(--ep-bg-tertiary); border-radius: 6px; font-size: 13px; color: var(--ep-success); margin-bottom: 16px; }
  .list { display: flex; flex-direction: column; gap: 10px; }
  .card { background: var(--ep-bg-secondary); border: 1px solid var(--ep-border); border-radius: 10px; padding: 16px; display: flex; justify-content: space-between; align-items: center; gap: 16px; }
  .name { font-weight: 700; font-size: 15px; margin-bottom: 4px; }
  .meta { font-size: 12px; color: var(--ep-text-secondary); font-family: var(--ep-font-mono); }
  .meta.monitor { color: var(--ep-accent); margin-top: 2px; }
  .actions { display: flex; gap: 6px; flex-shrink: 0; }
  button { padding: 6px 12px; border-radius: 6px; border: 1px solid var(--ep-border); background: var(--ep-bg-tertiary); color: var(--ep-text-primary); cursor: pointer; font-size: 12px; font-weight: 600; }
  button.primary { background: var(--ep-accent); border-color: var(--ep-accent); color: #fff; }
  button.danger { background: transparent; border-color: var(--ep-danger); color: var(--ep-danger); }
  .empty { color: var(--ep-text-muted); font-size: 14px; margin-top: 20px; }
</style>
