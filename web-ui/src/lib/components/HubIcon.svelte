<script lang="ts">
  import Icons from '$desktop/components/Icons.svelte';

  /**
   * Les trois pictogrammes que le hub ajoute au jeu partagé avec l'app desktop.
   *
   * Ils sont définis ici et non dans `src/lib/components/Icons.svelte` parce
   * que Comptes, Journal et Notifications sont des écrans de hub : l'app
   * desktop n'a ni comptes ni journal à montrer. Même grille 16×16, même trait
   * de 1,5 et même `currentColor` que le jeu partagé — la barre de navigation
   * doit se lire comme une seule série, pas comme deux familles côte à côte.
   */
  /** Les noms du jeu partagé que la barre du hub utilise réellement. */
  type SharedName = 'server' | 'settings' | 'gear';
  export type HubIconName = 'users' | 'journal' | 'bell' | SharedName;

  const { name, size = 16 } = $props<{ name: HubIconName; size?: number }>();
</script>

{#if name === 'users'}
  <svg
    width={size}
    height={size}
    viewBox="0 0 16 16"
    fill="none"
    stroke="currentColor"
    stroke-width="1.5"
    stroke-linecap="round"
    stroke-linejoin="round"
  >
    <circle cx="6" cy="5" r="2.6" />
    <path d="M1.5 14v-.8A3.7 3.7 0 015.2 9.5h1.6a3.7 3.7 0 013.7 3.7v.8" />
    <path d="M11 3.2a2.6 2.6 0 010 4.9" />
    <path d="M12.4 9.7A3.7 3.7 0 0114.5 13v1" />
  </svg>
{:else if name === 'journal'}
  <svg
    width={size}
    height={size}
    viewBox="0 0 16 16"
    fill="none"
    stroke="currentColor"
    stroke-width="1.5"
    stroke-linecap="round"
    stroke-linejoin="round"
  >
    <path d="M3 2.5h9a1 1 0 011 1v9a1 1 0 01-1 1H3" />
    <path d="M3 2.5a1.5 1.5 0 000 3h1.5V2.5H3z" />
    <path d="M3 13.5a1.5 1.5 0 010-3h1.5v3H3z" />
    <path d="M6.8 6h4M6.8 9h2.6" />
  </svg>
{:else if name === 'bell'}
  <svg
    width={size}
    height={size}
    viewBox="0 0 16 16"
    fill="none"
    stroke="currentColor"
    stroke-width="1.5"
    stroke-linecap="round"
    stroke-linejoin="round"
  >
    <path d="M4 6.6a4 4 0 018 0c0 3 1.2 4.1 1.2 4.1H2.8S4 9.6 4 6.6z" />
    <path d="M6.6 13a1.6 1.6 0 002.8 0" />
  </svg>
{:else}
  <Icons {name} {size} />
{/if}
