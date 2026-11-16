<script lang="ts">
  import type { Profile } from '../stores/profiles';
  import { api } from '../tauri';
  import { _ } from 'svelte-i18n';
  import { selectedInterface } from '../stores/selectedInterface';

  const { profile, onSave, onCancel } = $props<{
    profile?: Partial<Profile>;
    onSave: (p: Profile) => void;
    onCancel: () => void;
  }>();

  // Le profil s'applique toujours à l'interface active du Dashboard.
  // On ne stocke plus d'interface dans le profil — il est portable.
  let form = $state<Profile>({
    id: profile?.id ?? crypto.randomUUID(),
    name: profile?.name ?? '',
    ip: profile?.ip ?? '',
    subnet: profile?.subnet ?? '255.255.255.0',
    gateway: profile?.gateway ?? '',
    dns_primary: profile?.dns_primary ?? '8.8.8.8',
    dns_secondary: profile?.dns_secondary ?? '8.8.4.4',
    monitor_ips: profile?.monitor_ips ?? [],
  });

  // Édition textuelle : une IP par ligne. Parsé au save pour retomber
  // sur un string[] propre (trim + filter vide).
  let monitorIpsText = $state((profile?.monitor_ips ?? []).join('\n'));

  function save() {
    form.monitor_ips = monitorIpsText
      .split('\n')
      .map((s: string) => s.trim())
      .filter((s: string) => s.length > 0);
    onSave(form);
  }

  async function autoDetect() {
    if (!$selectedInterface) return;
    const details = await api.getInterfaceDetails($selectedInterface);
    if (details.ip) form.ip = details.ip;
    if (details.subnet) form.subnet = details.subnet;
    if (details.gateway) form.gateway = details.gateway;
    if (details.dns.length > 0) form.dns_primary = details.dns[0];
    if (details.dns.length > 1) form.dns_secondary = details.dns[1];
  }
</script>

<div class="overlay" role="presentation">
  <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="h-pf">
    <h2 id="h-pf">{profile?.id ? $_('profiles.form.edit_title') : $_('profiles.form.new_title')}</h2>
    <p class="hint">
      {$_('profiles.form.applied_to')}{#if $selectedInterface} <span class="m">{$selectedInterface}</span>{/if}
    </p>
    <div class="field"><label for="pf-name">{$_('profiles.form.name')}</label><input id="pf-name" class="input" bind:value={form.name} placeholder={$_('profiles.form.name_placeholder')} /></div>
    <div class="grid2">
      <div class="field"><label for="pf-ip">{$_('profiles.form.ip')}</label><input id="pf-ip" class="input m" bind:value={form.ip} placeholder={$_('profiles.form.ip_placeholder')} /></div>
      <div class="field"><label for="pf-mask">{$_('profiles.form.mask')}</label><input id="pf-mask" class="input m" bind:value={form.subnet} placeholder={$_('profiles.form.mask_placeholder')} /></div>
    </div>
    <div class="field">
      <label for="pf-gw">{$_('profiles.form.gateway')} <span class="muted">{$_('profiles.form.gateway_optional')}</span></label>
      <input id="pf-gw" class="input m" bind:value={form.gateway} placeholder={$_('profiles.form.gateway_placeholder')} />
    </div>
    <div class="grid2">
      <div class="field">
        <label for="pf-dns1">{$_('profiles.form.dns_primary')} <span class="muted">{$_('profiles.form.dns_optional')}</span></label>
        <input id="pf-dns1" class="input m" bind:value={form.dns_primary} placeholder={$_('profiles.form.dns_primary_placeholder')} />
      </div>
      <div class="field">
        <label for="pf-dns2">{$_('profiles.form.dns_secondary')} <span class="muted">{$_('profiles.form.dns_optional')}</span></label>
        <input id="pf-dns2" class="input m" bind:value={form.dns_secondary} placeholder={$_('profiles.form.dns_secondary_placeholder')} />
      </div>
    </div>
    <div class="field">
      <label for="pf-mon">{$_('profiles.form.monitor_ips')}</label>
      <textarea id="pf-mon" class="textarea m" bind:value={monitorIpsText} placeholder={$_('profiles.form.monitor_ips_placeholder')} rows="3"></textarea>
      <span class="hint">{$_('profiles.form.monitor_ips_hint')}</span>
    </div>
    <div class="row">
      <button type="button" class="btn quiet" onclick={autoDetect} title={$_('profiles.form.detect_title')}>{$_('profiles.form.detect')}</button>
      <span class="grow"></span>
      <button type="button" class="btn" onclick={onCancel}>{$_('profiles.form.cancel')}</button>
      <button type="button" class="btn primary" onclick={save} disabled={!form.name.trim()}>{$_('profiles.form.save')}</button>
    </div>
  </div>
</div>
