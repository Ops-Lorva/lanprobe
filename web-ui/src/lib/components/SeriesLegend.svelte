<script lang="ts">
  import { _ } from 'svelte-i18n';

  export interface LegendRow {
    key: string;
    label: string;
    color: string;
    points: { t: number; v: number }[];
  }

  interface Props {
    rows: LegendRow[];
    /** Unité affichée après les valeurs — `ms`, `Mbit/s`… */
    unit: string;
    decimals?: number;
    /** Clés masquées, tenues par le parent. */
    hidden: Set<string>;
    ontoggle: (key: string) => void;
  }
  let { rows, unit, decimals = 1, hidden, ontoggle }: Props = $props();

  /**
   * Min / moyenne / max d'une courbe.
   *
   * Une valeur isolée ne dit presque rien : c'est la dispersion qui apprend
   * quelque chose. Un lien qui donne 900 puis 200 puis 880 n'a pas le même
   * problème qu'un lien stable à 660, et les deux ont la même moyenne.
   */
  function stats(points: { t: number; v: number }[]) {
    if (points.length === 0) return null;
    let min = Infinity;
    let max = -Infinity;
    let sum = 0;
    for (const p of points) {
      if (p.v < min) min = p.v;
      if (p.v > max) max = p.v;
      sum += p.v;
    }
    return { min, max, avg: sum / points.length, n: points.length };
  }

  const lines = $derived(
    rows.flatMap((r) => {
      const s = stats(r.points);
      return s ? [{ ...r, ...s }] : [];
    }),
  );
</script>

<!--
  ⚠️ C'est la SEULE légende de la carte. Elle l'était en double — une en
  en-tête, une ici — ce qui donnait deux fois la même liste de couleurs à dix
  centimètres d'écart, et obligeait à chercher laquelle des deux répondait au
  clic.

  Chaque ligne est un bouton : la légende sert aussi à éteindre une courbe,
  et plusieurs cibles superposées deviennent illisibles sans ça.
-->
{#if lines.length}
  <div class="legend">
    {#each lines as l (l.key)}
      <button
        type="button"
        class="row"
        class:off={hidden.has(l.key)}
        aria-pressed={!hidden.has(l.key)}
        title={$_('charts.toggle_hint')}
        onclick={() => ontoggle(l.key)}
      >
        <span class="key">
          <span class="dot" style:background={l.color}></span>
          <span class="label">{l.label}</span>
        </span>
        <span class="vals lp-mono">
          <span>{$_('charts.stat_min')} {l.min.toFixed(decimals)}</span>
          <span>{$_('charts.stat_avg')} {l.avg.toFixed(decimals)}</span>
          <span>{$_('charts.stat_max')} {l.max.toFixed(decimals)}</span>
          <span class="unit">{unit}</span>
          <span class="n">{$_('charts.stat_runs', { values: { n: l.n } })}</span>
        </span>
      </button>
    {/each}
  </div>
{/if}

<style>
  .legend {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin-top: 10px;
    padding-top: 8px;
    border-top: 1px solid var(--ep-border);
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    justify-content: space-between;
    gap: 3px 12px;
    width: 100%;
    padding: 3px 6px;
    margin: 0;
    border: none;
    border-radius: 4px;
    background: none;
    color: inherit;
    font: inherit;
    font-size: 11.5px;
    text-align: left;
    cursor: pointer;
  }
  .row:hover {
    background: var(--ep-bg-tertiary);
  }
  .row:focus-visible {
    outline: 2px solid var(--ep-accent);
    outline-offset: -2px;
  }
  /* Masquée : libellé barré ET grisé. La couleur seule ne dit rien à qui ne
     la voit pas, l'opacité seule se confond avec du texte secondaire. */
  .row.off {
    color: var(--ep-text-dim);
  }
  .row.off .label {
    text-decoration: line-through;
  }
  .row.off .dot {
    opacity: 0.3;
  }
  .key {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    color: var(--ep-text-secondary);
  }
  .row.off .key {
    color: inherit;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 2px;
    flex: none;
  }
  .label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .vals {
    display: flex;
    gap: 10px;
    font-variant-numeric: tabular-nums;
  }
  .unit,
  .n {
    color: var(--ep-text-dim);
  }
  @media (max-width: 560px) {
    .vals {
      gap: 8px;
      font-size: 11px;
    }
    /* Sur un écran étroit, l'unité est déjà dans le titre de la carte. */
    .unit {
      display: none;
    }
  }
</style>
