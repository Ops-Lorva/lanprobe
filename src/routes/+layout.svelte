<script lang="ts">
  import '@fontsource/geist-sans/400.css';
  import '@fontsource/geist-sans/500.css';
  import '@fontsource/geist-sans/600.css';
  import '@fontsource/geist-sans/700.css';
  import '@fontsource/geist-sans/800.css';
  import '@fontsource/geist-mono/400.css';
  import '@fontsource/geist-mono/500.css';
  import '@fontsource/geist-mono/600.css';
  import '@fontsource/geist-mono/700.css';
  import '../app.css';
  import type { Snippet } from 'svelte';
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { monitoring } from '$lib/stores/monitoring';
  import { discoveryStore } from '$lib/stores/discovery';
  import { portscan } from '$lib/stores/portscan';
  import { portscanProfiles } from '$lib/stores/portscanProfiles';
  import { profiles } from '$lib/stores/profiles';
  import { settings } from '$lib/stores/settings';
  import { selectedInterface } from '$lib/stores/selectedInterface';
  import { applyRemoteConfig } from '$lib/stores/configStore';
  import { initI18n } from '$lib/i18n';
  import { isLoading } from 'svelte-i18n';
  import SetupScreen from '$lib/components/SetupScreen.svelte';
  import LoginScreen from '$lib/components/LoginScreen.svelte';

  initI18n('en');

  const { children } = $props<{ children: Snippet }>();

  // Détecté via le marqueur injecté par le shim Tauri du lanprobe-server
  // quand il sert l'UI en HTTP. En mode desktop Tauri natif, ce flag est
  // absent et on boot directement dans l'app sans prompt de login.
  const isWeb = typeof window !== 'undefined' && (window as any).__LANPROBE_WEB__ === true;

  // 'init'  : on ne sait pas encore (fetch /api/status en cours)
  // 'setup' : aucun user configuré → SetupScreen
  // 'login' : users configurés mais pas de cookie valide → LoginScreen
  // 'app'   : go, on rend l'app principale
  let authPhase = $state<'init' | 'setup' | 'login' | 'app'>(isWeb ? 'init' : 'app');

  async function checkAuth() {
    try {
      const r = await fetch('/api/status', { credentials: 'same-origin' });
      const s = await r.json();
      if (s.needs_setup) authPhase = 'setup';
      else if (!s.authenticated) authPhase = 'login';
      else authPhase = 'app';
    } catch {
      // Si /api/status plante on préfère tenter l'app : c'est le desktop
      // natif qui n'a pas d'endpoint à lui, et le check est une précaution.
      authPhase = 'app';
    }
  }

  async function bootApp() {
    try {
      const saved = localStorage.getItem('lanprobe.dashboard.interface');
      if (saved) await invoke('cmd_set_selected_interface', { name: saved });
    } catch {}
    await monitoring.init();
    await Promise.all([
      settings.init(),
      portscanProfiles.init(),
      profiles.init(),
      selectedInterface.init(),
      portscan.init(),
    ]);

    // Bus `config:update` : quand un client (desktop ou web) modifie la
    // config, le backend pousse le blob complet — on remplace le cache
    // local et on relance les init() pour que les stores Svelte
    // réagissent en live sans que l'utilisateur ait à rafraîchir.
    await listen<unknown>('config:update', ({ payload }) => {
      applyRemoteConfig(payload);
      settings.init();
      profiles.init();
      portscanProfiles.init();
    });

    await listen('discovery:done', () => {
      const s = get(settings);
      if (!s.autoPortScanProfileId) return;
      const prof = get(portscanProfiles).find(p => p.id === s.autoPortScanProfileId);
      if (!prof) return;
      const hosts = Array.from(get(discoveryStore).results.values());
      for (const h of hosts) {
        portscan.add(h.ip, prof.tcp_ports, prof.udp_ports, prof.id, prof.name);
      }
    });

  }

  onMount(async () => {
    if (isWeb) await checkAuth();
    if (authPhase === 'app') await bootApp();
  });

  async function onAuthDone() {
    authPhase = 'app';
    await bootApp();
  }
</script>

{#if !$isLoading}
  {#if authPhase === 'init'}
    <div style="position:fixed;inset:0;display:flex;align-items:center;justify-content:center;color:#888;font-family:monospace;">Chargement…</div>
  {:else if authPhase === 'setup'}
    <SetupScreen onDone={onAuthDone} />
  {:else if authPhase === 'login'}
    <LoginScreen onDone={onAuthDone} />
  {:else}
    {@render children()}
  {/if}
{/if}
