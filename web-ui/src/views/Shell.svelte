<script lang="ts">
  import { _ } from 'svelte-i18n';
  import LogoMark from '$desktop/components/LogoMark.svelte';
  import { route } from '$lib/router';
  import { activeNavId } from '$lib/nav-active';
  import { identity, isAdmin } from '$lib/session';
  import FleetView from './FleetView.svelte';
  import NotificationsView from './NotificationsView.svelte';
  import AccountsView from './AccountsView.svelte';
  import ProbeView from './ProbeView.svelte';
  import SettingsView from './SettingsView.svelte';
  import AuditView from './AuditView.svelte';

  const { version, onExpired, logout } = $props<{
    version: string;
    onExpired: () => void;
    logout: () => void;
  }>();

  // DA « Relevé » : une barre haute, cinq destinations. Notifications et
  // Comptes quittent la bande d'onglets de Réglages — on y va pour constater
  // (qui est prévenu, qui peut entrer), pas pour préparer.
  //
  // « Enrôler » n'y figure pas : c'est le bouton « Ajouter une sonde » du parc,
  // et le « + » de chaque site, là où le site est déjà connu.
  //
  // ⚠️ Journal et Comptes restent réservés à `admin`, comme leurs routes côté
  // hub : le masquage est du confort, la vérification est serveur (§ 18).
  const items: { id: string; href: string; key: string; admin?: true }[] = [
    { id: 'fleet', href: '#/', key: 'nav.fleet' },
    { id: 'notifications', href: '#/notifications', key: 'nav.notifications' },
    { id: 'accounts', href: '#/accounts', key: 'nav.accounts', admin: true },
    { id: 'audit', href: '#/audit', key: 'nav.audit', admin: true },
    { id: 'settings', href: '#/settings', key: 'nav.settings' },
  ];

  const visible = $derived(items.filter((i) => !i.admin || $isAdmin));

  // Quelle entrée est surlignée : la règle est dans `nav-active.ts`, où elle
  // est testée. Écrite ici, dans un `class:active={…}`, elle n'était couverte
  // par rien — et c'est là qu'elle s'est trompée sur « Mon compte ».
  const activeNav = $derived(activeNavId($route));

  let menuOpen = $state(false);

  /** Les deux premières lettres du compte, comme l'app iOS. */
  const initials = $derived(($identity?.username ?? '').slice(0, 2).toUpperCase());

  // C'est `main` qui défile désormais, pas le document : un changement d'écran
  // doit donc le remonter à la main, sinon on arrive au milieu de la fiche
  // suivante avec le défilement de la précédente.
  let mainEl = $state<HTMLElement | null>(null);
  $effect(() => {
    void $route.name;
    // Passer d'une sonde à l'autre — ou d'un onglet de Réglages à l'autre —
    // change l'écran sans changer `name` : il faut lire ce qui, lui, change,
    // sinon on arrive au milieu du panneau suivant avec le défilement du
    // précédent.
    void ($route.name === 'probe' ? $route.id : '');
    void ($route.name === 'settings' ? $route.tab : '');
    mainEl?.scrollTo({ top: 0 });
    window.scrollTo({ top: 0 });
  });
</script>

<!--
  Barre haute collante, contenu centré à 1440 px. C'est le document qui
  défile (voir `web.css`) : la barre colle, et la barre d'outils des
  navigateurs mobiles se replie. Sous 60 rem les destinations passent dans un
  menu.
-->
<header class="top">
  <div class="top-in">
    <a class="brand" href="#/">
      <LogoMark size={24} />
      <span>{$_('common.app_name')}</span>
    </a>

    <button
      class="lp-btn ghost sm menu-btn"
      aria-expanded={menuOpen}
      aria-controls="mainnav"
      onclick={() => (menuOpen = !menuOpen)}
    >
      {$_('nav.menu')}
    </button>

    <nav class="mainnav" class:open={menuOpen} id="mainnav" aria-label={$_('nav.menu')}>
      {#each visible as item (item.id)}
        <a
          href={item.href}
          aria-current={activeNav === item.id ? 'page' : undefined}
          onclick={() => (menuOpen = false)}
        >
          {$_(item.key)}
        </a>
      {/each}
    </nav>

    <span class="spacer"></span>

    <!--
      « Mon compte » : MON identité sur ce hub — mot de passe, second facteur,
      clés d'accès, appareils appairés. Sa place est contre « Se
      déconnecter » : les deux parlent de la même session.
    -->
    <a
      class="who"
      href="#/account"
      aria-current={activeNav === 'account' ? 'page' : undefined}
      title={$_('settings.tab_account')}
    >
      <span class="av" aria-hidden="true">{initials}</span>
      <span class="name">
        {$identity?.username ?? ''}{#if $identity?.role}, {$_(
            `accounts.role_${$identity.role}`,
          ).toLowerCase()}{/if}
      </span>
    </a>
    <button class="lp-btn ghost sm" onclick={logout}>{$_('nav.logout')}</button>
  </div>
</header>

<main class="page" bind:this={mainEl}>
  {#if $route.name === 'probe'}
    <ProbeView id={$route.id} {onExpired} />
  {:else if $route.name === 'audit'}
    <AuditView {onExpired} />
  {:else if $route.name === 'notifications'}
    <header class="lp-head"><h1>{$_('nav.notifications')}</h1></header>
    <NotificationsView {onExpired} />
  {:else if $route.name === 'accounts' && $isAdmin}
    <AccountsView {onExpired} />
    <!-- « Mon compte » n'est pas un onglet de Réglages, mais c'est encore
         `SettingsView` qui dessine son écran (sans bande d'onglets, il le
         sait). -->
  {:else if $route.name === 'settings' || $route.name === 'account'}
    <SettingsView {onExpired} />
  {:else}
    <FleetView {onExpired} />
  {/if}
  {#if version}
    <p class="ver">{$_('nav.version', { values: { version } })}</p>
  {/if}
</main>

<style>
  .top {
    position: sticky;
    top: 0;
    z-index: 300;
    background: color-mix(in oklab, var(--color-paper) 92%, transparent);
    backdrop-filter: blur(8px);
    border-bottom: 1px solid var(--color-rule);
  }
  .top-in {
    max-width: 1440px;
    margin-inline: auto;
    padding: 0 32px;
    display: flex;
    align-items: center;
    gap: 24px;
    height: 52px;
  }
  .brand {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-weight: 600;
    font-size: 1rem;
    letter-spacing: -0.01em;
    color: var(--color-ink);
    text-decoration: none;
  }
  .mainnav {
    display: flex;
    gap: 2px;
  }
  .mainnav a {
    position: relative;
    display: inline-flex;
    align-items: center;
    height: 52px;
    padding: 0 12px;
    color: var(--color-ink-2);
    text-decoration: none;
    font-weight: 500;
    transition: color 120ms cubic-bezier(0.16, 1, 0.3, 1);
  }
  .mainnav a:hover,
  .mainnav a[aria-current='page'] {
    color: var(--color-ink);
  }
  .mainnav a[aria-current='page']::after {
    content: '';
    position: absolute;
    left: 12px;
    right: 12px;
    bottom: -1px;
    height: 2px;
    background: var(--color-accent);
  }
  .mainnav a:focus-visible {
    outline-offset: -6px;
  }
  .spacer {
    flex: 1;
  }
  .who {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-size: 0.8125rem;
    color: var(--color-ink-2);
    text-decoration: none;
    border-radius: 5px;
    padding: 4px 6px;
  }
  .who:hover,
  .who[aria-current='page'] {
    color: var(--color-ink);
    background: var(--color-paper-3);
  }
  .av {
    width: 26px;
    height: 26px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--color-paper-4);
    color: var(--color-ink);
    font-weight: 600;
    font-size: 0.75rem;
  }
  .menu-btn {
    display: none;
  }

  .page {
    display: block;
    max-width: 1440px;
    margin-inline: auto;
    padding: 24px 32px 96px;
    min-width: 0;
  }
  .ver {
    margin-top: 64px;
    font-size: 0.75rem;
    color: var(--color-muted);
  }

  @media (max-width: 60rem) {
    .top-in {
      padding: 0 24px;
      gap: 12px;
    }
    .menu-btn {
      display: inline-flex;
      order: 3;
    }
    .mainnav {
      display: none;
      position: absolute;
      top: 52px;
      left: 0;
      right: 0;
      flex-direction: column;
      background: var(--color-paper-2);
      border-bottom: 1px solid var(--color-rule);
      padding: 8px 24px;
    }
    .mainnav.open {
      display: flex;
    }
    .mainnav a {
      height: 44px;
    }
    .mainnav a[aria-current='page']::after {
      left: -12px;
      right: auto;
      top: 10px;
      bottom: 10px;
      width: 2px;
      height: auto;
    }
    .who .name {
      display: none;
    }
    .page {
      padding: 16px 24px 72px;
    }
  }
  @media (max-width: 40rem) {
    .top-in,
    .page {
      padding-inline: 16px;
    }
  }
</style>
