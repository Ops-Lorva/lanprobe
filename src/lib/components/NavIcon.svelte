<script lang="ts" module>
  export type NavIconName = 'home' | 'folder' | 'search' | 'activity' | 'ports' | 'zap' | 'settings' | 'refresh';
</script>

<script lang="ts">
  // Pictogrammes de la navigation, repris de la maquette « Relevé » : grille
  // 20×20, trait 1,6. `Icons.svelte` reste celui que le hub partage.
  let { name }: { name: NavIconName } = $props();
</script>

<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
  {#if name === 'home'}
    <path d="M3 8.5 10 3l7 5.5V17H3z" /><path d="M8 17v-5h4v5" />
  {:else if name === 'folder'}
    <path d="M2.5 5.5h5l1.5 2h8.5V16h-15z" />
  {:else if name === 'search'}
    <circle cx="9" cy="9" r="5" /><path d="m13 13 4 4" />
  {:else if name === 'activity'}
    <path d="M2 10h3.5l2-5 4 10 2-5H18" />
  {:else if name === 'ports'}
    <path d="M3 7V3h4M13 3h4v4M17 13v4h-4M7 17H3v-4M6 10h8" />
  {:else if name === 'zap'}
    <path d="M11 2 4 11h5l-1 7 7-9h-5z" />
  {:else if name === 'settings'}
    <circle cx="10" cy="10" r="2.6" /><path d="M10 2.5v2M10 15.5v2M2.5 10h2M15.5 10h2M4.7 4.7l1.4 1.4M13.9 13.9l1.4 1.4M4.7 15.3l1.4-1.4M13.9 6.1l1.4-1.4" />
  {:else if name === 'refresh'}
    <path d="M16 7a6.5 6.5 0 1 0 .5 5" /><path d="M16.5 3v4h-4" />
  {/if}
</svg>
