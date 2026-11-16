<script lang="ts">
  import { onMount } from 'svelte';
  import { _ } from 'svelte-i18n';
  import LogoMark from './LogoMark.svelte';
  import NavIcon, { type NavIconName } from './NavIcon.svelte';
  import StateMark from './StateMark.svelte';
  import InternetIndicator from './InternetIndicator.svelte';
  import { hubStatus } from '../stores/hub';
  import { hubMark } from '../status';

  const { currentPage, onNavigate } = $props<{
    currentPage: string;
    onNavigate: (page: string) => void;
  }>();

  const items: { id: string; icon: NavIconName; key: string }[] = [
    { id: 'dashboard',  icon: 'home',     key: 'nav.dashboard'  },
    { id: 'profiles',   icon: 'folder',   key: 'nav.profiles'   },
    { id: 'discovery',  icon: 'search',   key: 'nav.discovery'  },
    { id: 'monitoring', icon: 'activity', key: 'nav.monitoring' },
    { id: 'ports',      icon: 'ports',    key: 'nav.ports'      },
    { id: 'speedtest',  icon: 'zap',      key: 'nav.speedtest'  },
  ];

  onMount(() => hubStatus.watch());
  const hub = $derived(hubMark($hubStatus));
</script>

<nav class="side" aria-label={$_('nav.sections')}>
  <span class="brand"><LogoMark size={24} /><span>LanProbe</span></span>
  {#each items as item}
    <button
      class="it"
      type="button"
      aria-current={currentPage === item.id ? 'page' : undefined}
      onclick={() => onNavigate(item.id)}
      title={$_(item.key)}
    >
      <NavIcon name={item.icon} />
      <span class="lbl">{$_(item.key)}</span>
    </button>
  {/each}
  <span class="grow"></span>
  <button
    class="it"
    type="button"
    aria-current={currentPage === 'settings' ? 'page' : undefined}
    onclick={() => onNavigate('settings')}
    title={$_('nav.settings')}
  >
    <NavIcon name="settings" />
    <span class="lbl">{$_('nav.settings')}</span>
  </button>
  <div class="hubstate">
    <InternetIndicator variant="block" />
    <StateMark mark={hub} />
    {#if $hubStatus.enrolled}
      <span class="muted">{$hubStatus.url.replace(/^https?:\/\//, '')}</span>
      {#if $hubStatus.site || $hubStatus.name}
        <span class="muted">{[$hubStatus.site, $hubStatus.name].filter(Boolean).join(', ')}</span>
      {/if}
    {/if}
  </div>
</nav>
