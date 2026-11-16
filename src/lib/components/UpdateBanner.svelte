<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { onMount } from 'svelte';
  import { _ } from 'svelte-i18n';

  interface UpdateInfo {
    current_version: string;
    latest_version: string | null;
    has_update: boolean;
    asset_url: string | null;
    asset_name: string | null;
    platform_supported: boolean;
    release_notes_url: string | null;
  }

  let info = $state<UpdateInfo | null>(null);
  let dismissed = $state(false);
  let installing = $state(false);
  let error = $state<string | null>(null);

  const DISMISS_KEY = 'lanprobe.update.dismissed';

  onMount(async () => {
    try {
      const u = await invoke<UpdateInfo>('cmd_check_update');
      if (u?.has_update && u.latest_version) {
        const last = localStorage.getItem(DISMISS_KEY);
        if (last === u.latest_version) dismissed = true;
        info = u;
      }
    } catch {
      // silent
    }
  });

  function dismiss() {
    dismissed = true;
    if (info?.latest_version) {
      try { localStorage.setItem(DISMISS_KEY, info.latest_version); } catch {}
    }
  }

  async function install() {
    if (!info?.asset_url || !info?.asset_name) return;
    installing = true;
    error = null;
    try {
      await invoke<string>('cmd_apply_update', {
        url: info.asset_url,
        assetName: info.asset_name,
      });
      // L'installeur a été lancé — on ne quitte pas l'app nous-mêmes,
      // il fermera le process quand l'utilisateur confirmera.
    } catch (e) {
      error = String(e);
      installing = false;
    }
  }
</script>

{#if info && !dismissed}
  <div class="banner info" role="status">
    <span class="grow">
      {$_('banners.update_available', { values: { v: info.latest_version } })}
      {#if error}<span class="err-text"> {$_('banners.update_install_error')} : {error}</span>{/if}
      {#if !info.asset_url && info.release_notes_url}<span class="muted"> {$_('banners.update_unsupported')}</span>{/if}
    </span>
    {#if info.asset_url}
      <button class="btn sm primary" type="button" onclick={install} disabled={installing} aria-busy={installing}>
        {installing ? $_('banners.update_installing') : $_('banners.update_install')}
      </button>
    {:else if info.release_notes_url}
      <a class="link" href={info.release_notes_url} target="_blank" rel="noopener noreferrer">{$_('banners.update_view')}</a>
    {/if}
    <button class="btn sm quiet" type="button" onclick={dismiss} disabled={installing}>{$_('banners.dismiss')}</button>
  </div>
{/if}
