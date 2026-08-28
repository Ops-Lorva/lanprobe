<script lang="ts">
  import { _ } from 'svelte-i18n';
  import LogoMark from '$desktop/components/LogoMark.svelte';
  import Icons from '$desktop/components/Icons.svelte';
  import LangTheme from '$lib/components/LangTheme.svelte';
  import { route } from '$lib/router';
  import FleetView from './FleetView.svelte';
  import ProbeView from './ProbeView.svelte';
  import EnrollView from './EnrollView.svelte';
  import SettingsView from './SettingsView.svelte';

  const { version, onExpired, logout } = $props<{
    version: string;
    onExpired: () => void;
    logout: () => void;
  }>();

  // Les réglages restent hors du chemin de mise en route : on y arrive quand
  // quelque chose ne marche pas, pas au premier démarrage.
  const items = [
    { id: 'fleet', href: '#/', icon: 'server' as const, key: 'nav.fleet' },
    { id: 'enroll', href: '#/enroll', icon: 'scan' as const, key: 'nav.enroll' },
    { id: 'settings', href: '#/settings', icon: 'settings' as const, key: 'nav.settings' },
  ];
</script>

<!--
  Même ossature que l'app desktop : barre latérale de 168 px, logo en haut,
  réglages en bas. Elle devient une barre horizontale sous 900 px — la
  navigation compte deux entrées, une colonne de 168 px sur un téléphone
  coûterait le tiers de la largeur pour rien.
-->
<div class="shell">
  <nav class="sidebar">
    <a class="logo" href="#/">
      <LogoMark size={24} />
      <span class="logo-text">{$_('common.app_name')}</span>
      <span class="tag">{$_('common.hub')}</span>
    </a>

    <div class="nav-main">
      {#each items as item (item.id)}
        <a class="nav-item" class:active={$route.name === item.id} href={item.href}>
          <Icons name={item.icon} size={15} />
          <span class="nav-label">{$_(item.key)}</span>
        </a>
      {/each}
    </div>

    <div class="nav-bottom">
      <LangTheme />
      <button class="nav-item logout" onclick={logout}>
        <Icons name="chevron-right" size={15} />
        <span class="nav-label">{$_('nav.logout')}</span>
      </button>
      {#if version}
        <span class="version lp-mono">{$_('nav.version', { values: { version } })}</span>
      {/if}
    </div>
  </nav>

  <main>
    {#if $route.name === 'probe'}
      <ProbeView id={$route.id} {onExpired} />
    {:else if $route.name === 'enroll'}
      <EnrollView {onExpired} />
    {:else if $route.name === 'settings'}
      <SettingsView {onExpired} />
    {:else}
      <FleetView {onExpired} />
    {/if}
  </main>
</div>

<style>
  .shell {
    display: flex;
    min-height: 100dvh;
    background: var(--ep-bg-primary);
  }

  .sidebar {
    width: 168px;
    flex-shrink: 0;
    background: var(--ep-glass-bg);
    border-right: 1px solid var(--ep-glass-border);
    display: flex;
    flex-direction: column;
    padding: 10px 8px;
    gap: 2px;
    position: sticky;
    top: 0;
    height: 100dvh;
  }

  .logo {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 8px 14px;
    border-bottom: 1px solid var(--ep-glass-border);
    margin-bottom: 6px;
    text-decoration: none;
    flex-wrap: wrap;
  }
  .logo-text {
    font-size: 12px;
    font-weight: 800;
    color: var(--ep-text-primary);
    letter-spacing: 0.4px;
  }
  .tag {
    font-size: 8px;
    text-transform: uppercase;
    letter-spacing: 1px;
    color: var(--ep-accent-bright);
    border: 1px solid var(--ep-accent-glow);
    background: var(--ep-accent-dim);
    border-radius: 999px;
    padding: 1px 5px;
  }

  .nav-main {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .nav-bottom {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding-top: 8px;
    border-top: 1px solid var(--ep-glass-border);
  }

  .nav-item {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 8px 10px;
    border: none;
    background: transparent;
    border-radius: var(--ep-radius-md);
    color: var(--ep-text-secondary);
    cursor: pointer;
    font-family: var(--ep-font-sans);
    font-size: 12.5px;
    font-weight: 500;
    text-align: left;
    text-decoration: none;
    transition:
      background 0.12s,
      color 0.12s;
  }
  .nav-item:hover {
    background: var(--ep-glass-bg-md);
    color: var(--ep-text-primary);
  }
  .nav-item.active {
    background: var(--ep-accent-dim);
    border: 1px solid var(--ep-accent-glow);
    color: var(--ep-accent-bright);
    font-weight: 600;
  }
  .nav-label {
    flex: 1;
  }
  .version {
    font-size: 9.5px;
    color: var(--ep-text-dim);
    padding: 0 10px 2px;
  }

  main {
    flex: 1;
    min-width: 0; /* sans ça, une cellule large pousse la page en défilement horizontal */
    padding: 18px 20px 40px;
  }

  @media (max-width: 900px) {
    .shell {
      flex-direction: column;
    }
    .sidebar {
      width: auto;
      height: auto;
      flex-direction: row;
      align-items: center;
      gap: 10px;
      border-right: none;
      border-bottom: 1px solid var(--ep-glass-border);
      padding: 8px 12px;
      background: var(--ep-bg-secondary);
      z-index: 5;
    }
    .logo {
      border-bottom: none;
      margin: 0;
      padding: 0;
      flex-wrap: nowrap;
    }
    .nav-main {
      flex-direction: row;
      flex: 1;
    }
    .nav-bottom {
      flex-direction: row;
      align-items: center;
      border-top: none;
      padding-top: 0;
    }
    .version {
      display: none;
    }
    main {
      padding: 14px 12px 36px;
    }
  }
  @media (max-width: 620px) {
    .nav-label {
      display: none;
    }
    .logo-text,
    .tag {
      display: none;
    }
  }
</style>
