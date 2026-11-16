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
  import InternetIndicator from '$lib/components/InternetIndicator.svelte';
  import LogoMark from '$lib/components/LogoMark.svelte';
  import NavIcon from '$lib/components/NavIcon.svelte';
  import ScreenScope from '$lib/components/ScreenScope.svelte';
  import { settings } from '$lib/stores/settings';
  import { internetStatus } from '$lib/stores/internetStatus';
  import { _ } from 'svelte-i18n';
  import { tick } from 'svelte';
  import { setNavigate } from '$lib/screen';

  // Écran actif (barre latérale) et onglet actif (trois onglets).
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

  // Trois onglets thématiques, et les écrans que chacun empile.
  const groups = [
    { id: 'network', key: 'nav.groups.network', sections: ['dashboard', 'profiles', 'discovery'] },
    { id: 'probes',  key: 'nav.groups.probes',  sections: ['monitoring', 'ports', 'speedtest'] },
    { id: 'config',  key: 'nav.groups.config',  sections: ['settings'] },
  ] as const;
  const group = $derived(groups.find((g) => g.id === currentGroup) ?? groups[0]);

  // « Tout voir » et autres renvois : l'écran (barre latérale) ou l'onglet
  // qui le contient, puis sa section (trois onglets). En « une page », tout
  // est déjà à l'écran : le lien n'est pas proposé (cf. Dashboard).
  setNavigate(async (p) => {
    if ($settings.layout === 'sidebar') { currentPage = p; return; }
    const g = groups.find((x) => (x.sections as readonly string[]).includes(p));
    if (!g) return;
    currentGroup = g.id;
    await tick();
    document.getElementById(`s-${p}`)?.scrollIntoView({ block: 'start' });
  });
</script>

{#snippet screen(id: string)}
  {#if id === 'dashboard'}<Dashboard />
  {:else if id === 'profiles'}<Profiles />
  {:else if id === 'monitoring'}<Monitoring />
  {:else if id === 'discovery'}<Discovery />
  {:else if id === 'ports'}<PortScan />
  {:else if id === 'speedtest'}<SpeedTest />
  {:else if id === 'settings'}<Settings />
  {/if}
{/snippet}

{#snippet banners()}
  <div class="banners">
    <SetupBanner />
    <UpdateBanner />
  </div>
{/snippet}

{#if $settings.layout === 'sidebar'}
  <div class="app-side">
    <Sidebar {currentPage} onNavigate={(p) => (currentPage = p)} />
    <main class="appmain">
      {@render banners()}
      <ScreenScope kind="page">{@render screen(currentPage)}</ScreenScope>
    </main>
  </div>

{:else if $settings.layout === 'single'}
  <div class="app-top">
    <header class="apptop">
      <span class="brand"><LogoMark size={22} /><span>LanProbe</span></span>
      <span class="grow"></span>
      <InternetIndicator variant="chip" />
      <button class="btn icon quiet" type="button" title={$_('nav.settings')} aria-label={$_('nav.settings')}
              aria-expanded={settingsOpen} onclick={() => (settingsOpen = true)}>
        <NavIcon name="settings" />
      </button>
    </header>
    {@render banners()}
    <main class="dash-grid">
      <ScreenScope kind="cell">
        {#each ['dashboard', 'discovery', 'monitoring', 'profiles', 'ports', 'speedtest'] as id (id)}
          <section class="cell" aria-label={$_(`nav.${id}`)}>{@render screen(id)}</section>
        {/each}
      </ScreenScope>
    </main>
  </div>

{:else}
  <div class="app-top">
    <header class="apptop">
      <span class="brand"><LogoMark size={22} /><span>LanProbe</span></span>
      <nav class="tabs apptabs" aria-label={$_('nav.groups.label')}>
        {#each groups as g (g.id)}
          <button type="button" aria-current={currentGroup === g.id ? 'page' : undefined}
                  onclick={() => (currentGroup = g.id)}>{$_(g.key)}</button>
        {/each}
      </nav>
      <span class="grow"></span>
      <InternetIndicator variant="chip" />
    </header>
    {@render banners()}
    <div class="scroll">
      <main class="gmain">
        {#if group.sections.length > 1}
          <nav class="jump" aria-label={$_('nav.sections')}>
            {#each group.sections as id (id)}<a href="#s-{id}">{$_(`nav.${id}`)}</a>{/each}
          </nav>
        {/if}
        {#key currentGroup}
          <ScreenScope kind="section">
            {#each group.sections as id (id)}
              <section class="gsec" id="s-{id}">{@render screen(id)}</section>
            {/each}
          </ScreenScope>
        {/key}
      </main>
    </div>
  </div>
{/if}

{#if settingsOpen}
  <div class="overlay" role="presentation" onclick={() => (settingsOpen = false)}>
    <!-- La touche Échap est écoutée sur la fenêtre (voir onMount). -->
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div class="dialog overlay-panel" role="dialog" aria-modal="true" aria-labelledby="ov-title" tabindex="-1"
         onclick={(e) => e.stopPropagation()}>
      <div class="cell-h">
        <h2 id="ov-title">{$_('nav.settings')}</h2>
        <div class="acts">
          <button class="btn sm quiet" type="button" onclick={() => (settingsOpen = false)}>{$_('common.close')}</button>
        </div>
      </div>
      <div class="ov-body">
        <ScreenScope kind="bare"><Settings /></ScreenScope>
      </div>
    </div>
  </div>
{/if}
