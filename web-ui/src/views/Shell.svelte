<script lang="ts">
  import { _ } from 'svelte-i18n';
  import LogoMark from '$desktop/components/LogoMark.svelte';
  import Icons from '$desktop/components/Icons.svelte';
  import HubIcon, { type HubIconName } from '$lib/components/HubIcon.svelte';
  import { route } from '$lib/router';
  import { isAdmin } from '$lib/session';
  import FleetView from './FleetView.svelte';
  import ProbeView from './ProbeView.svelte';
  import SettingsView from './SettingsView.svelte';
  import AccountsView from './AccountsView.svelte';
  import AuditView from './AuditView.svelte';
  import NotificationsView from './NotificationsView.svelte';

  const { version, onExpired, logout } = $props<{
    version: string;
    onExpired: () => void;
    logout: () => void;
  }>();

  // « Enrôler » a disparu de la navigation parce que l'enrôlement n'est plus un
  // endroit où aller : c'est le « + » de la ligne du site, là où le site est
  // déjà connu. Une entrée de menu aurait ramené le choix du site que ce « + »
  // supprime.
  //
  // Les cinq entrées sont ordonnées par portée, pas par fréquence : ce qu'on
  // surveille (Parc), ce qui vient nous chercher (Notifications), qui a le
  // droit et qui a fait quoi (Comptes, Journal), puis la configuration
  // (Réglages), qui reste hors du chemin de mise en route — on y arrive quand
  // quelque chose ne marche pas.
  //
  // Le journal a son entrée propre plutôt qu'un onglet dans un écran
  // « Administration » : c'est ce qu'on ouvre sous pression pendant un
  // incident, et c'est aussi l'intitulé par lequel on apprend que le hub en
  // tient un.
  const items: { id: string; href: string; icon: HubIconName; key: string; admin?: true }[] = [
    { id: 'fleet', href: '#/', icon: 'server', key: 'nav.fleet' },
    { id: 'notifications', href: '#/notifications', icon: 'bell', key: 'nav.notifications' },
    { id: 'accounts', href: '#/accounts', icon: 'users', key: 'nav.accounts', admin: true },
    { id: 'audit', href: '#/audit', icon: 'journal', key: 'nav.audit', admin: true },
    { id: 'settings', href: '#/settings', icon: 'settings', key: 'nav.settings' },
  ];

  // Visibles tant que le hub n'a pas dit non. Voir `$lib/session` : il n'existe
  // aucune route qui donne le rôle de la session, et sonder au démarrage
  // déposerait au journal un refus que personne n'a provoqué.
  const visible = $derived(items.filter((i) => !i.admin || $isAdmin !== false));

  // C'est `main` qui défile désormais, pas le document : un changement d'écran
  // doit donc le remonter à la main, sinon on arrive au milieu de la fiche
  // suivante avec le défilement de la précédente.
  let mainEl = $state<HTMLElement | null>(null);
  $effect(() => {
    void $route.name;
    // Passer d'une sonde à l'autre change l'écran sans changer `name` : c'est
    // l'identifiant qu'il faut lire pour que l'effet se redéclenche.
    void ($route.name === 'probe' ? $route.id : '');
    mainEl?.scrollTo({ top: 0 });
    window.scrollTo({ top: 0 });
  });
</script>

<!--
  Même ossature que l'app desktop : barre latérale de 168 px, logo en haut,
  réglages en bas. Elle devient une barre horizontale sous 900 px — une colonne
  de 168 px sur un téléphone coûterait le tiers de la largeur pour rien — puis
  une rangée de pictogrammes seuls sous 620 px, où les cinq entrées tiennent
  encore sur 390 px sans déborder.
-->
<div class="shell">
  <nav class="sidebar">
    <a class="logo" href="#/">
      <LogoMark size={24} />
      <span class="logo-text">{$_('common.app_name')}</span>
      <span class="tag">{$_('common.hub')}</span>
    </a>

    <div class="nav-main">
      {#each visible as item (item.id)}
        <a
          class="nav-item"
          class:active={$route.name === item.id}
          href={item.href}
          title={$_(item.key)}
        >
          <HubIcon name={item.icon} size={15} />
          <span class="nav-label">{$_(item.key)}</span>
        </a>
      {/each}
    </div>

    <!-- Langue et thème sont partis dans Réglages → Général : ce sont des
         préférences, pas de la navigation, et la barre n'a plus à les porter. -->
    <div class="nav-bottom">
      <button class="nav-item logout" onclick={logout}>
        <Icons name="chevron-right" size={15} />
        <span class="nav-label">{$_('nav.logout')}</span>
      </button>
    </div>
  </nav>

  <main bind:this={mainEl}>
    {#if $route.name === 'probe'}
      <ProbeView id={$route.id} {onExpired} />
    {:else if $route.name === 'notifications'}
      <NotificationsView {onExpired} />
    {:else if $route.name === 'accounts'}
      <AccountsView {onExpired} />
    {:else if $route.name === 'audit'}
      <AuditView {onExpired} />
    {:else if $route.name === 'settings'}
      <SettingsView {onExpired} />
    {:else}
      <FleetView {onExpired} />
    {/if}
  </main>
</div>

<style>
  /* Sur grand écran, c'est le panneau de droite qui défile, pas la page : la
     barre latérale n'est alors pas « collée », elle est structurellement hors
     du flux défilant. Aucun `position: sticky` à faire fonctionner, donc rien
     qui puisse se décoller — c'est le modèle des consoles denses (Proxmox,
     Grafana) et le seul qui tienne dans tous les moteurs. */
  .shell {
    display: flex;
    height: 100dvh;
    overflow: hidden;
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
    height: 100%;
    overflow-y: auto;
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

  main {
    flex: 1;
    min-width: 0; /* sans ça, une cellule large pousse la page en défilement horizontal */
    overflow-y: auto;
    overflow-x: hidden;
    padding: 18px 20px 40px;
  }

  /* Sous 900 px la barre devient horizontale : là, le document reprend le
     défilement (la barre d'outils des navigateurs mobiles ne se replie que si
     c'est la page qui bouge) et la barre colle en haut. */
  @media (max-width: 900px) {
    .shell {
      flex-direction: column;
      height: auto;
      min-height: 100dvh;
      overflow: visible;
    }
    main {
      overflow: visible;
    }
    .sidebar {
      width: auto;
      height: auto;
      overflow: visible;
      position: sticky;
      top: 0;
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
