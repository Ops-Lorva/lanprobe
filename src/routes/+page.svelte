<script lang="ts">
  import { onMount } from 'svelte';
  import Sidebar from '$lib/components/Sidebar.svelte';
  import Dashboard from '$lib/components/Dashboard.svelte';
  import Profiles from '$lib/components/Profiles.svelte';
  import Monitoring from '$lib/components/Monitoring.svelte';
  import Discovery from '$lib/components/Discovery.svelte';
  import PortScan from '$lib/components/PortScan.svelte';
  import SpeedTest from '$lib/components/SpeedTest.svelte';
  import Settings from '$lib/components/Settings.svelte';
  import UpdateBanner from '$lib/components/UpdateBanner.svelte';
  import SetupBanner from '$lib/components/SetupBanner.svelte';
  import SectionAnchor from '$lib/components/SectionAnchor.svelte';
  import InternetIndicator from '$lib/components/InternetIndicator.svelte';
  import LogoMark from '$lib/components/LogoMark.svelte';
  import { settings } from '$lib/stores/settings';
  import { internetStatus } from '$lib/stores/internetStatus';
  import { _ } from 'svelte-i18n';

  // Onglet actif pour les modes sidebar + grouped.
  let currentPage = $state('dashboard');
  let currentGroup = $state<'network' | 'probes' | 'config'>('network');
  let settingsOpen = $state(false);

  onMount(() => {
    settings.init();
    internetStatus.init();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === ',' && !e.metaKey && !e.ctrlKey
          && !(e.target instanceof HTMLInputElement)
          && !(e.target instanceof HTMLTextAreaElement)) {
        settingsOpen = !settingsOpen;
      }
      if (e.key === 'Escape') settingsOpen = false;
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  // Regroupement pour le mode "grouped" : 3 onglets thématiques.
  const groups = [
    { id: 'network', icon: '🌐', key: 'nav.groups.network' },
    { id: 'probes',  icon: '📡', key: 'nav.groups.probes'  },
    { id: 'config',  icon: '⚙️', key: 'nav.groups.config'  },
  ] as const;
</script>

<div class="app" class:layout-single={$settings.layout === 'single'} class:layout-grouped={$settings.layout === 'grouped'}>
  {#if $settings.layout === 'sidebar'}
    <Sidebar {currentPage} onNavigate={(p) => (currentPage = p)} />
    <div class="main-area">
      <SetupBanner />
      <UpdateBanner />
      <main class="content">
        {#if currentPage === 'dashboard'}<Dashboard />
        {:else if currentPage === 'profiles'}<Profiles />
        {:else if currentPage === 'monitoring'}<Monitoring />
        {:else if currentPage === 'discovery'}<Discovery />
        {:else if currentPage === 'ports'}<PortScan />
        {:else if currentPage === 'speedtest'}<SpeedTest />
        {:else if currentPage === 'settings'}<Settings />
        {/if}
      </main>
    </div>

  {:else if $settings.layout === 'single'}
    <div class="main-area single">
      <SetupBanner />
      <UpdateBanner />
      <nav class="top-tabs">
        <div class="brand"><LogoMark size={22} />LanProbe</div>
        <div class="spacer"></div>
        <InternetIndicator variant="chip" />
        <button class="ico-btn" title={$_('nav.settings')} onclick={() => (settingsOpen = true)}>⚙</button>
      </nav>
      <main class="content grid-content">
        <div class="dash-grid">
          <div class="cell col-4"><Dashboard /></div>
          <div class="cell col-4"><Discovery /></div>
          <div class="cell col-4"><Monitoring /></div>
          <div class="cell col-4"><Profiles /></div>
          <div class="cell col-4"><PortScan /></div>
          <div class="cell col-4"><SpeedTest /></div>
        </div>
      </main>
    </div>

  {:else}
    <!-- grouped : 3 onglets top (Network / Probes / Config) -->
    <div class="main-area grouped">
      <SetupBanner />
      <UpdateBanner />
      <nav class="top-tabs">
        <div class="brand"><LogoMark size={22} />LanProbe</div>
        <div class="tabs">
          {#each groups as g}
            <button class="tab" class:active={currentGroup === g.id} onclick={() => (currentGroup = g.id)}>
              <span class="ico">{g.icon}</span>
              <span>{$_(g.key)}</span>
            </button>
          {/each}
        </div>
        <div class="spacer"></div>
        <InternetIndicator variant="chip" />
      </nav>
      <main class="content">
        {#if currentGroup === 'network'}
          <SectionAnchor id="dashboard" icon="🏠" label={$_('nav.dashboard')} />   <Dashboard />
          <SectionAnchor id="profiles"  icon="📋" label={$_('nav.profiles')} />     <Profiles />
          <SectionAnchor id="discovery" icon="🔍" label={$_('nav.discovery')} /> <Discovery />
        {:else if currentGroup === 'probes'}
          <SectionAnchor id="monitoring" icon="📡" label={$_('nav.monitoring')} /> <Monitoring />
          <SectionAnchor id="ports"      icon="🔌" label={$_('nav.ports')} />      <PortScan />
          <SectionAnchor id="speedtest"  icon="⚡" label={$_('nav.speedtest')} />  <SpeedTest />
        {:else}
          <SectionAnchor id="settings"   icon="⚙️" label={$_('nav.settings')} />  <Settings />
        {/if}
      </main>
    </div>
  {/if}

  {#if settingsOpen}
    <div class="overlay" onclick={() => (settingsOpen = false)} role="presentation">
      <div class="overlay-panel" onclick={(e) => e.stopPropagation()} role="dialog">
        <div class="overlay-head">
          <div class="t">⚙ {$_('nav.settings')}</div>
          <button class="close" onclick={() => (settingsOpen = false)} aria-label="close">✕</button>
        </div>
        <div class="overlay-body">
          <Settings />
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .app { display: flex; height: 100vh; overflow: hidden; background: var(--ep-bg-primary); position: relative; }
  .app.layout-single, .app.layout-grouped { flex-direction: column; }
  .main-area { flex: 1; display: flex; flex-direction: column; overflow-y: hidden; overflow-x: auto; min-width: 0; }
  .content { flex: 1; overflow-y: auto; }

  /* ── Topbar (single + grouped) ─────────────────────── */
  .top-tabs {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 0 20px;
    height: 46px;
    background: var(--ep-glass-bg);
    border-bottom: 1px solid var(--ep-glass-border);
    flex-shrink: 0;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    font-weight: 800;
    letter-spacing: .8px;
    color: var(--ep-text-primary);
  }
  /* Grouped tabs */
  .tabs { display: flex; gap: 3px; }
  .tab {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 6px 14px;
    border: none;
    background: transparent;
    color: var(--ep-text-secondary);
    cursor: pointer;
    font-size: 12.5px;
    font-weight: 500;
    border-radius: var(--ep-radius-md);
    height: 32px;
    transition: background 0.1s, color 0.1s;
  }
  .tab:hover { background: var(--ep-glass-bg-md); color: var(--ep-text-primary); }
  .tab.active { background: var(--ep-accent-dim); border: 1px solid var(--ep-accent-glow); color: var(--ep-accent-bright); font-weight: 600; }

  .spacer { flex: 1; }
  .ico-btn {
    width: 30px; height: 30px; border-radius: var(--ep-radius-md);
    background: var(--ep-glass-bg-md); border: 1px solid var(--ep-glass-border);
    color: var(--ep-text-secondary); font-size: 13px; cursor: pointer;
    display: flex; align-items: center; justify-content: center;
    transition: background 0.1s, color 0.1s;
  }
  .ico-btn:hover { background: var(--ep-accent-dim); border-color: var(--ep-accent-glow); color: var(--ep-accent-bright); }

  /* ── Single layout grid ─────────────────────────────── */
  .grid-content { padding: 14px 16px; overflow: auto; }
  .dash-grid {
    display: grid;
    grid-template-columns: repeat(12, 1fr);
    grid-template-rows: minmax(0, 1fr) minmax(0, 1fr);
    gap: 10px;
    height: 100%;
  }
  .cell {
    background: var(--ep-glass-bg);
    border: 1px solid var(--ep-glass-border);
    border-radius: var(--ep-radius-lg);
    min-width: 0;
    min-height: 0;
    overflow-y: auto;
    overflow-x: auto;
    transition: border-color 0.15s;
  }
  .cell:hover { border-color: var(--ep-glass-border-strong); }
  .col-4 { grid-column: span 4; }
  @media (max-width: 1100px) {
    .grid-content { overflow-y: auto; }
    .dash-grid { grid-template-rows: none; grid-auto-rows: minmax(280px, auto); height: auto; }
    .cell { max-height: 60vh; }
    .col-4 { grid-column: span 6; }
  }
  @media (max-width: 700px) { .col-4 { grid-column: span 12; } }

  /* ── Settings overlay ───────────────────────────────── */
  .overlay {
    position: absolute; inset: 0;
    background: rgba(8,12,20,0.75);
    backdrop-filter: blur(8px);
    display: flex; align-items: center; justify-content: center;
    padding: 20px; z-index: 1000;
  }
  .overlay-panel {
    background: var(--ep-bg-secondary);
    border: 1px solid var(--ep-glass-border-strong);
    border-radius: 16px;
    width: 640px; max-width: 100%;
    max-height: 90vh; overflow: hidden;
    display: flex; flex-direction: column;
    box-shadow: 0 40px 80px rgba(0,0,0,0.6), 0 0 0 1px var(--ep-accent-glow);
  }
  .overlay-head {
    display: flex; align-items: center; justify-content: space-between;
    padding: 16px 22px; border-bottom: 1px solid var(--ep-glass-border);
  }
  .overlay-head .t { font-size: 14px; font-weight: 700; }
  .overlay-head .close {
    width: 28px; height: 28px; border-radius: var(--ep-radius-md);
    background: var(--ep-glass-bg-md); border: 1px solid var(--ep-glass-border);
    color: var(--ep-text-muted); cursor: pointer; font-size: 13px;
    display: flex; align-items: center; justify-content: center;
    transition: background 0.1s;
  }
  .overlay-head .close:hover { background: var(--ep-danger); color: #fff; border-color: var(--ep-danger); }
  .overlay-body { overflow-y: auto; }
</style>
