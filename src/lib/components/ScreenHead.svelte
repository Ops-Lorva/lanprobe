<script lang="ts">
  import type { Snippet } from 'svelte';
  import { screenKind } from '../screen';

  /**
   * L'en-tête d'un écran : un titre et ses actions. Sa forme suit la
   * disposition (voir `../screen.ts`), le contenu ne change pas.
   */
  let { title, acts, id }: { title: string; acts?: Snippet; id?: string } = $props();
  const kind = screenKind();
</script>

{#if kind === 'cell'}
  <div class="cell-h">
    <h2 {id}>{title}</h2>
    {#if acts}<div class="acts">{@render acts()}</div>{/if}
  </div>
{:else if kind === 'section'}
  <div class="appbar">
    <h2 {id}>{title}</h2>
    {#if acts}<div class="acts">{@render acts()}</div>{/if}
  </div>
{:else if kind === 'page'}
  <div class="appbar">
    <h1 {id}>{title}</h1>
    {#if acts}<div class="acts">{@render acts()}</div>{/if}
  </div>
{:else if acts}
  <div class="appbar bare"><div class="acts">{@render acts()}</div></div>
{/if}
