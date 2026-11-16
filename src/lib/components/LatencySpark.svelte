<script lang="ts">
  import { sparkPoints } from '../format';
  import type { PingEntry } from '../stores/monitoring';

  /**
   * La latence récente d'une cible, en ligne fine. Série fixe (`s1`…`s3`) :
   * l'identité d'une cible ne change pas avec la palette. Les relevés sans
   * réponse sont marqués d'un trait rouge au pied, et la courbe s'y coupe.
   */
  let { history, series = 1, width = 140, height = 28 }: {
    history: PingEntry[]; series?: number; width?: number; height?: number;
  } = $props();

  const segs = $derived(sparkPoints(history, width, height));
  const denom = $derived(Math.max(1, history.length - 1));
</script>

<svg class="chart spark" viewBox="0 0 {width} {height}" width={width} height={height} aria-hidden="true" preserveAspectRatio="none">
  {#each segs as seg}
    {#if seg.length === 1}
      <circle cx={seg[0][0]} cy={seg[0][1]} r="1.5" class="dotf s{series}" />
    {:else}
      <polyline class="line s{series}" points={seg.map((p) => p.join(',')).join(' ')} style="stroke-width:1.6" />
    {/if}
  {/each}
  {#each history as p, i}
    {#if !p.alive}<rect class="ib" x={(i / denom) * width - 1} y={height - 3} width="2" height="3" />{/if}
  {/each}
</svg>

<style>
  .spark { width: var(--spark-w, 140px); max-width: 100%; height: 28px; }
  .dotf { stroke: none; }
  .dotf.s1 { fill: var(--lp-series-1); } .dotf.s2 { fill: var(--lp-series-2); } .dotf.s3 { fill: var(--lp-series-3); }
</style>
