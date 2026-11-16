<script lang="ts">
  import { _, locale } from 'svelte-i18n';
  import TimeSeriesChart from './TimeSeriesChart.svelte';
  import { lossCells, lossLevel } from '$lib/heartbeat';

  /**
   * « Une cible = une rangée » : la latence de chaque cible à SON échelle, et,
   * juste dessous, sa perte en niveaux de gris sur le même temps.
   *
   * ⚠️ Pas de double axe ni d'échelle commune : une passerelle à 1 ms sous un
   * résolveur public à 150 ms s'écrasait sur la ligne de base. Passerelle
   * calme et résolveur en pic au même instant : le problème est en amont.
   */
  interface Row {
    key: string;
    target: string;
    label: string;
    color: string;
    points: { t: number; v: number }[];
    peak?: { t: number; v: number }[];
  }
  interface Props {
    rows: Row[];
    /** Disponibilité `alive` par adresse : 1 = répond, moyenne quand le hub agrège. */
    alive: Record<string, { t: number; v: number }[]>;
    domain: { from: number; to: number };
    unit: string;
    /** L'adresse de la passerelle, pour la nommer. */
    gateway?: string | null;
  }
  let { rows, alive, domain, unit, gateway = null }: Props = $props();

  const lang = $derived($locale ?? 'en');
  const CELLS = 96;

  const fmt = $derived(
    new Intl.NumberFormat(lang, { minimumFractionDigits: 1, maximumFractionDigits: 1 }),
  );
  const fmt2 = $derived(
    new Intl.NumberFormat(lang, { minimumFractionDigits: 2, maximumFractionDigits: 2 }),
  );

  function stats(r: Row) {
    const v = r.points.map((p) => p.v).sort((a, b) => a - b);
    if (v.length === 0) return null;
    const avg = v.reduce((a, b) => a + b, 0) / v.length;
    const p95 = v[Math.min(v.length - 1, Math.floor(v.length * 0.95))];
    const peaks = (r.peak ?? []).map((p) => p.v);
    const max = Math.max(v[v.length - 1], ...peaks);
    const a = alive[r.target] ?? [];
    const loss = a.length ? (1 - a.reduce((s, p) => s + p.v, 0) / a.length) * 100 : null;
    return { avg, p95, max, loss };
  }
</script>

<div class="rows">
  {#each rows as r, i (r.key)}
    {@const st = stats(r)}
    {@const cells = lossCells(alive[r.target] ?? [], domain.from, domain.to, CELLS)}
    <div class="row">
      <div class="rh">
        <span class="sw" style:background={r.color}></span>
        <b>{r.target && r.target === gateway ? $_('charts.gateway') : r.label}</b>
        {#if r.target && (r.target !== r.label || r.target === gateway)}<code>{r.target}</code>{/if}
        {#if st}
          <span class="st">
            {$_('charts.row_stats', {
              values: {
                avg: fmt.format(st.avg),
                p95: fmt.format(st.p95),
                max: fmt.format(st.max),
                unit,
              },
            })}{#if st.loss != null}, {$_('charts.row_loss', { values: { loss: fmt2.format(st.loss) } })}{/if}
          </span>
        {/if}
      </div>
      <TimeSeriesChart series={[r]} {domain} {unit} height={i === rows.length - 1 ? 116 : 94} axis={i === rows.length - 1} />
      <div class="loss" role="img" aria-label={$_('charts.loss_aria', { values: { target: r.target || r.label } })}>
        <span class="ll">{$_('charts.loss_short')}</span>
        <div class="cells">
          {#each cells as c, i (i)}
            <i class={c == null ? 'none' : `l${lossLevel(c)}`}></i>
          {/each}
        </div>
      </div>
    </div>
  {/each}
</div>

<div class="lp-legend foot">
  <span>{$_('charts.loss_legend')}</span>
  <span><i class="lp-sw l0"></i>0</span>
  <span><i class="lp-sw l1"></i>{$_('charts.loss_l1')}</span>
  <span><i class="lp-sw l2"></i>{$_('charts.loss_l2')}</span>
  <span><i class="lp-sw l3"></i>{$_('charts.loss_l3')}</span>
  <span><i class="lp-sw l4"></i>{$_('charts.loss_l4')}</span>
</div>

<style>
  .rows {
    display: grid;
  }
  .row {
    padding: 12px 0;
    border-bottom: 1px solid var(--color-rule);
  }
  .row:first-child {
    padding-top: 0;
  }
  .rh {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 4px 8px;
    font-size: 0.875rem;
    margin-bottom: 4px;
  }
  .sw {
    width: 14px;
    height: 3px;
    border-radius: 2px;
    align-self: center;
  }
  b {
    font-weight: 600;
  }
  code {
    font-family: var(--ep-font-mono);
    font-size: 0.8125rem;
    color: var(--color-ink-2);
  }
  .st {
    margin-left: auto;
    color: var(--color-muted);
    font-size: 0.8125rem;
    font-variant-numeric: tabular-nums;
  }
  /* Même gouttière que le graphe (46 px à gauche, 14 à droite) : chaque case
     tombe sous l'instant qu'elle décrit. */
  .loss {
    position: relative;
    padding: 0 14px 0 46px;
  }
  .ll {
    position: absolute;
    left: 0;
    width: 40px;
    text-align: right;
    font-size: 0.75rem;
    color: var(--color-muted);
    line-height: 12px;
  }
  .cells {
    display: grid;
    grid-template-columns: repeat(96, minmax(0, 1fr));
    gap: 1px;
    height: 12px;
  }
  .cells i {
    display: block;
    border-radius: 1px;
  }
  .l0 {
    background: var(--loss-0);
  }
  .l1 {
    background: var(--loss-1);
  }
  .l2 {
    background: var(--loss-2);
  }
  .l3 {
    background: var(--loss-3);
  }
  .l4 {
    background: var(--loss-4);
  }
  .none {
    background: transparent;
  }
  .foot {
    margin-top: 12px;
  }
</style>
