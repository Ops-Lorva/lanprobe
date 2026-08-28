<script lang="ts">
  import { _, locale } from 'svelte-i18n';
  import { compactNumber } from '$lib/time';

  interface Props {
    points: number;
    /** Au-delà, le tampon n'est plus un hoquet mais un incident. */
    alertThreshold?: number;
  }
  let { points, alertThreshold = 1000 }: Props = $props();

  const lang = $derived($locale ?? 'en');
  const level = $derived(points === 0 ? 'ok' : points < alertThreshold ? 'warn' : 'alert');
</script>

<!--
  `buffered_points` n'est pas une statistique de plus : c'est le seul signe
  visible qu'une sonde mesure sans réussir à écrire. À zéro il reste discret
  (un tiret ne se lit pas comme une alerte) ; dès qu'il monte il prend une
  barre de remplissage et une couleur, et au-delà du seuil il passe en rouge.
-->
<span class="buf {level}" title={points > 0 ? $_('buffer.pending', { values: { n: points } }) : ''}>
  {#if points === 0}
    <span class="dash" aria-hidden="true">—</span>
    <span class="lp-sr">{$_('buffer.empty')}</span>
  {:else}
    <span class="bar" aria-hidden="true">
      <span class="fill" style:width="{Math.min(100, (points / alertThreshold) * 100)}%"></span>
    </span>
    <span class="val lp-mono">{compactNumber(points, lang)}</span>
  {/if}
</span>

<style>
  .buf {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
  }
  .dash {
    color: var(--ep-text-dim);
  }
  .bar {
    width: 34px;
    height: 4px;
    border-radius: 2px;
    background: var(--ep-bg-tertiary);
    border: 1px solid var(--ep-border);
    overflow: hidden;
    flex-shrink: 0;
  }
  .fill {
    display: block;
    height: 100%;
    min-width: 3px;
    background: var(--ep-warning);
  }
  .alert .fill {
    background: var(--ep-danger);
  }
  .val {
    font-weight: 600;
    color: var(--ep-warning);
  }
  .alert .val {
    color: var(--ep-danger);
  }
</style>
