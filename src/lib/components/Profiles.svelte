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
  import ScreenHead from './ScreenHead.svelte';
  import { maskToPrefix } from '../format';
  import { screenKind } from '../screen';
  import type { InterfaceDetails } from '../tauri';

  const isCell = screenKind() === 'cell';

  // Le profil dont la configuration est celle de l'interface en ce moment :
  // même comparaison que le tableau de bord (IP, masque, passerelle, DNS).
  let ifaceNow = $state<InterfaceDetails | null>(null);
  async function loadIface() {
    const name = $selectedInterface;
    if (!name) { ifaceNow = null; return; }
    try { ifaceNow = await api.getInterfaceDetails(name); } catch { ifaceNow = null; }
  }
  $effect(() => { void $selectedInterface; loadIface(); });
  const appliedId = $derived.by(() => {
    const i = ifaceNow;
    if (!i || i.dhcp_enabled) return null;
    return $profiles.find(p => p.ip === (i.ip ?? '') && p.subnet === (i.subnet ?? '')
      && p.gateway === (i.gateway ?? '') && p.dns_primary === (i.dns[0] ?? ''))?.id ?? null;
  });
  const addr = (p: Profile) => {
    const n = maskToPrefix(p.subnet);
    return n != null ? `${p.ip}/${n}` : `${p.ip} / ${p.subnet}`;
  };
  let confirmRemove = $state<Profile | null>(null);

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
      // 🔴 **Changer de profil, c'est repartir de zéro.** TOUTES les
      // surveillances en cours s'arrêtent, y compris celles que le nouveau
      // profil redemande : elles visaient le réseau d'un autre site, et leur
      // historique ne décrit plus rien. Les garder mélangerait dans une même
      // courbe deux réseaux qui n'ont rien à voir — et les minutes de bascule
      // s'y liraient comme une panne du nouveau site.
      //
      // ⚠️ `cmd_stop_ping` et pas un simple retrait d'affichage : le retrait
      // doit être ÉCRIT, sinon le hub le réapplique au battement suivant et la
      // cible revient toute seule.
      for (const ip of Array.from($monitoring.keys())) {
        monitoring.removeHost(ip);
        try { await invoke('cmd_stop_ping', { ip }); }
        catch (e) { console.error('[Profiles] cmd_stop_ping failed for', ip, e); }
      }
      // Puis exactement ce que le nouveau profil demande, et rien d'autre.
      for (const ip of p.monitor_ips ?? []) {
        monitoring.addHost(ip);
        try { await invoke('cmd_start_ping', { ip }); }
        catch (e) { console.error('[Profiles] cmd_start_ping failed for', ip, e); }
      }
      await loadIface();
    } catch (e) { flashStatus(`${tr('common.error')}: ${e}`); }
  }

  function openNew() { editing = undefined; showForm = true; }
  function openEdit(p: Profile) { editing = p; showForm = true; }
  function handleSave(p: Profile) {
    if (editing?.id) profiles.edit(p); else profiles.add(p);
    showForm = false;
  }
</script>

<div class="screen">
  <ScreenHead title={$_('profiles.title')}>
    {#snippet acts()}
      <button class="btn" class:sm={isCell} type="button" onclick={openNew}>{isCell ? $_('profiles.new_short') : $_('profiles.new')}</button>
    {/snippet}
  </ScreenHead>

  <div class="screen-body">
    <p class="hint" style="margin-bottom:12px">
      {#if $selectedInterface}
        {$_('profiles.target', { values: { iface: $selectedInterface } })}
      {:else}
        <span class="err-text">{$_('profiles.target_missing')}</span>
      {/if}
    </p>
    {#if status}<div class="banner info" role="status"><span class="grow">{status}</span></div>{/if}

    <div class="tbl-wrap">
      {#if $profiles.length === 0}
        <div class="state"><p>{$_('profiles.empty')}</p></div>
      {:else}
        <table class="t">
          <thead>
            <tr>
              <th>{$_('profiles.col_profile')}</th>
              <th>{$_('profiles.col_address')}</th>
              <th class="opt">{$_('profiles.col_gateway')}</th>
              <th class="opt">{$_('profiles.col_dns')}</th>
              <th class="num opt">{$_('profiles.col_targets')}</th>
              <th><span class="visually-hidden">{$_('profiles.col_actions')}</span></th>
            </tr>
          </thead>
          <tbody>
            {#each $profiles as p (p.id)}
              <tr class:sel={appliedId === p.id}>
                <td>
                  <span class="nm">{p.name}</span>
                  {#if appliedId === p.id}<span class="sub2">{$_('profiles.applied_on', { values: { iface: $selectedInterface } })}</span>{/if}
                </td>
                <td class="m">{addr(p)}</td>
                <td class="m opt">{p.gateway || '—'}</td>
                <td class="m opt">{[p.dns_primary, p.dns_secondary].filter(Boolean).join(', ') || '—'}</td>
                <td class="num opt" title={(p.monitor_ips ?? []).join('\n') || undefined}>{p.monitor_ips?.length ?? 0}</td>
                <td class="act">
                  {#if appliedId === p.id}
                    <button class="btn sm quiet" type="button" disabled>{$_('profiles.applied_short')}</button>
                  {:else}
                    <button class="btn sm" type="button" onclick={() => applyProfile(p)} disabled={!$selectedInterface}>{$_('profiles.apply')}</button>
                  {/if}
                  <button class="btn sm quiet" type="button" onclick={() => openEdit(p)}>{$_('profiles.edit')}</button>
                  <button class="btn sm quiet" type="button" onclick={() => (confirmRemove = p)}>{$_('profiles.remove')}</button>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </div>
  </div>
</div>

{#if showForm}
  <ProfileForm profile={editing} onSave={handleSave} onCancel={() => showForm = false} />
{/if}

{#if confirmRemove}
  <div class="overlay" role="presentation" onclick={() => (confirmRemove = null)}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div class="dialog" role="alertdialog" aria-modal="true" aria-labelledby="h-prm" tabindex="-1" onclick={(e) => e.stopPropagation()}>
      <h2 id="h-prm">{$_('profiles.remove_title', { values: { name: confirmRemove.name } })}</h2>
      <p class="muted">{$_('profiles.remove_body')}</p>
      <div class="row">
        <button class="btn" type="button" onclick={() => (confirmRemove = null)}>{$_('common.cancel')}</button>
        <button class="btn danger" type="button" onclick={() => { profiles.remove(confirmRemove!.id); confirmRemove = null; }}>{$_('profiles.remove')}</button>
      </div>
    </div>
  </div>
{/if}
