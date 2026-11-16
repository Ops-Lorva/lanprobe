<script lang="ts">
  import { _ } from 'svelte-i18n';
  import type { Mark } from '../status';

  /**
   * Forme + mot, jamais la couleur seule (voir `../status.ts`). Le mot peut
   * être remplacé (`label`) quand l'écran en dit plus, la forme jamais.
   */
  let { mark, label, since }: { mark: Mark; label?: string; since?: string } = $props();
</script>

<span class="st {mark.tone}">
  <svg viewBox="0 0 10 10" aria-hidden="true">
    {#if mark.shape === 'dot'}
      <circle cx="5" cy="5" r="4.2" fill="currentColor" />
    {:else if mark.shape === 'half'}
      <circle cx="5" cy="5" r="3.9" fill="none" stroke="currentColor" stroke-width="1.5" />
      <path d="M5 1.1A3.9 3.9 0 0 1 5 8.9Z" fill="currentColor" />
    {:else if mark.shape === 'ring'}
      <circle cx="5" cy="5" r="3.7" fill="none" stroke="currentColor" stroke-width="1.7" />
    {:else}
      <rect x="1" y="1" width="8" height="8" rx="1" fill="currentColor" />
    {/if}
  </svg>
  {label ?? $_(mark.key, { values: mark.values })}
  {#if since}<span class="since">{since}</span>{/if}
</span>
