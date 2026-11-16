<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { onMount } from 'svelte';
  import { _ } from 'svelte-i18n';

  let hasPerms = $state(true); // optimiste, vérifié au mount
  let installing = $state(false);
  let done = $state(false);
  let error = $state('');

  onMount(async () => {
    hasPerms = await invoke<boolean>('cmd_check_permissions');
  });

  async function setup() {
    installing = true;
    error = '';
    try {
      await invoke('cmd_install_permissions');
      done = true;
      hasPerms = true;
    } catch (e) {
      // "Opération annulée" = l'utilisateur a fermé le prompt, pas une erreur critique
      if (String(e).includes('annulée') || String(e).includes('cancelled')) {
        done = true; // Cache la bannière sans marquer comme configuré
      } else {
        error = String(e);
      }
    }
    installing = false;
  }
</script>

{#if !hasPerms && !done}
  <div class="banner" role="status">
    <svg viewBox="0 0 10 10" aria-hidden="true"><circle cx="5" cy="5" r="3.9" fill="none" stroke="currentColor" stroke-width="1.5" /><path d="M5 1.1A3.9 3.9 0 0 1 5 8.9Z" fill="currentColor" /></svg>
    <span class="grow"><b>{$_('banners.setup_title')}</b> <span class="ink2">{$_('banners.setup_desc')}</span></span>
    {#if error}<span class="err-text">{error}</span>{/if}
    <button class="btn sm primary" type="button" onclick={setup} disabled={installing} aria-busy={installing}>
      {installing ? $_('banners.setup_installing') : $_('banners.setup_install')}
    </button>
    <button class="btn sm quiet" type="button" onclick={() => { done = true; }}>{$_('banners.dismiss')}</button>
  </div>
{/if}
