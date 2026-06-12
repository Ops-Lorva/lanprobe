<script lang="ts">
  import { _ } from 'svelte-i18n';
  import Icons, { type IconName } from './Icons.svelte';
  import InternetIndicator from './InternetIndicator.svelte';
  import LogoMark from './LogoMark.svelte';

  const { currentPage, onNavigate } = $props<{
    currentPage: string;
    onNavigate: (page: string) => void;
  }>();

  const items: { id: string; icon: IconName; key: string }[] = [
    { id: 'dashboard',  icon: 'home',     key: 'nav.dashboard'  },
    { id: 'profiles',   icon: 'folder',   key: 'nav.profiles'   },
    { id: 'discovery',  icon: 'search',   key: 'nav.discovery'  },
    { id: 'monitoring', icon: 'activity', key: 'nav.monitoring' },
    { id: 'ports',      icon: 'scan',     key: 'nav.ports'      },
    { id: 'speedtest',  icon: 'zap',      key: 'nav.speedtest'  },
  ];
</script>

<nav class="sidebar">
  <div class="logo">
    <LogoMark size={24} />
    <span class="logo-text">LanProbe</span>
  </div>

  <div class="nav-main">
    {#each items as item}
      <button
        class="nav-item"
        class:active={currentPage === item.id}
        onclick={() => onNavigate(item.id)}
        title={$_(item.key)}
      >
        <Icons name={item.icon} size={15} />
        <span class="nav-label">{$_(item.key)}</span>
      </button>
    {/each}
  </div>

  <div class="nav-bottom">
    <div class="inet-wrap">
      <InternetIndicator variant="block" />
    </div>
    <button
      class="nav-item"
      class:active={currentPage === 'settings'}
      onclick={() => onNavigate('settings')}
      title={$_('nav.settings')}
    >
      <Icons name="settings" size={15} />
      <span class="nav-label">{$_('nav.settings')}</span>
    </button>
  </div>
</nav>

<style>
  .sidebar {
    width: 168px;
    height: 100vh;
    background: var(--ep-glass-bg);
    border-right: 1px solid var(--ep-glass-border);
    display: flex;
    flex-direction: column;
    padding: 10px 8px;
    flex-shrink: 0;
    gap: 2px;
  }

  .logo {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 8px 14px;
    border-bottom: 1px solid var(--ep-glass-border);
    margin-bottom: 6px;
  }
  .logo-text {
    font-size: 12px;
    font-weight: 800;
    color: var(--ep-text-primary);
    letter-spacing: .4px;
  }

  .nav-main { flex: 1; display: flex; flex-direction: column; gap: 2px; }
  .nav-bottom { display: flex; flex-direction: column; gap: 4px; padding-top: 8px; border-top: 1px solid var(--ep-glass-border); }

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
    font-size: 12.5px;
    font-weight: 500;
    text-align: left;
    transition: background 0.12s, color 0.12s;
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
  .nav-label { flex: 1; }

  /* InternetIndicator pleine largeur dans la sidebar */
  .inet-wrap { width: 100%; margin-bottom: 2px; }
  .inet-wrap :global(.ii-wrap) { display: block; }
  .inet-wrap :global(.ii) { width: 100%; box-sizing: border-box; }
</style>
