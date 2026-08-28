<script lang="ts">
  import { locale } from 'svelte-i18n';
  import { axisTime, tooltipTime } from '$lib/time';
  import type { Serie } from '$lib/charts';

  interface Props {
    series: Serie[];
    unit: string;
    height?: number;
    /** Décimales à l'affichage des valeurs. */
    decimals?: number;
  }
  let { series, unit, height = 168, decimals = 0 }: Props = $props();

  const lang = $derived($locale ?? 'en');

  // ── Géométrie ────────────────────────────────────────────────────────────
  const PAD = { top: 10, right: 14, bottom: 22, left: 46 };
  let wrap = $state<HTMLDivElement | null>(null);
  let width = $state(720);

  $effect(() => {
    if (!wrap) return;
    const ro = new ResizeObserver(([entry]) => {
      // On mesure la largeur réelle plutôt que d'étirer un viewBox : un SVG
      // mis à l'échelle déforme les traits de 2 px et le texte des axes.
      width = Math.max(280, Math.round(entry.contentRect.width));
    });
    ro.observe(wrap);
    return () => ro.disconnect();
  });

  const plotW = $derived(Math.max(10, width - PAD.left - PAD.right));
  const plotH = $derived(Math.max(10, height - PAD.top - PAD.bottom));

  /**
   * LTTB (largest-triangle-three-buckets) : sur sept jours de ticks, tracer un
   * point par pixel est inutile et lent. Un simple « un point sur N » gomme les
   * pics — donc les incidents. LTTB garde la silhouette, pics compris.
   */
  function decimate(pts: { t: number; v: number }[], target: number) {
    if (pts.length <= target || target < 3) return pts;
    const bucket = (pts.length - 2) / (target - 2);
    const out = [pts[0]];
    let a = 0;
    for (let i = 0; i < target - 2; i++) {
      const rangeStart = Math.floor((i + 1) * bucket) + 1;
      const rangeEnd = Math.min(Math.floor((i + 2) * bucket) + 1, pts.length);
      let avgT = 0;
      let avgV = 0;
      for (let j = rangeStart; j < rangeEnd; j++) {
        avgT += pts[j].t;
        avgV += pts[j].v;
      }
      const cnt = Math.max(1, rangeEnd - rangeStart);
      avgT /= cnt;
      avgV /= cnt;

      const from = Math.floor(i * bucket) + 1;
      const to = Math.floor((i + 1) * bucket) + 1;
      let best = from;
      let bestArea = -1;
      for (let j = from; j < Math.min(to, pts.length); j++) {
        const area = Math.abs(
          (pts[a].t - avgT) * (pts[j].v - pts[a].v) - (pts[a].t - pts[j].t) * (avgV - pts[a].v),
        );
        if (area > bestArea) {
          bestArea = area;
          best = j;
        }
      }
      out.push(pts[best]);
      a = best;
    }
    out.push(pts[pts.length - 1]);
    return out;
  }

  const drawn = $derived(
    series.map((s) => ({ ...s, points: decimate(s.points, Math.max(60, Math.floor(plotW / 2))) })),
  );

  const bounds = $derived.by(() => {
    let tMin = Infinity;
    let tMax = -Infinity;
    let vMax = 0;
    for (const s of series) {
      for (const p of s.points) {
        if (p.t < tMin) tMin = p.t;
        if (p.t > tMax) tMax = p.t;
        if (p.v > vMax) vMax = p.v;
      }
    }
    if (!Number.isFinite(tMin)) return { tMin: 0, tMax: 1, vMax: 1 };
    if (tMax === tMin) tMax = tMin + 1;
    return { tMin, tMax, vMax };
  });

  /** Graduations rondes : 0 / 25 / 50 / 75, jamais 0 / 23,7 / 47,4. */
  function niceTicks(max: number, count = 4) {
    if (max <= 0) return [0, 1];
    const raw = max / count;
    const mag = Math.pow(10, Math.floor(Math.log10(raw)));
    const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((s) => s >= raw) ?? mag * 10;
    const ticks: number[] = [];
    for (let v = 0; v <= max + step * 0.001; v += step) ticks.push(v);
    if (ticks[ticks.length - 1] < max) ticks.push(ticks[ticks.length - 1] + step);
    return ticks;
  }

  const ticks = $derived(niceTicks(bounds.vMax));
  const yMax = $derived(Math.max(ticks[ticks.length - 1], 1));

  const x = (t: number) => PAD.left + ((t - bounds.tMin) / (bounds.tMax - bounds.tMin)) * plotW;
  const y = (v: number) => PAD.top + plotH - (v / yMax) * plotH;

  function path(points: { t: number; v: number }[]) {
    if (points.length === 0) return '';
    if (points.length === 1) {
      // Un point isolé n'a pas de ligne : on le rend visible par son marqueur.
      return `M${x(points[0].t)},${y(points[0].v)}L${x(points[0].t)},${y(points[0].v)}`;
    }
    return points.map((p, i) => `${i ? 'L' : 'M'}${x(p.t).toFixed(1)},${y(p.v).toFixed(1)}`).join('');
  }

  function areaPath(points: { t: number; v: number }[]) {
    if (points.length < 2) return '';
    const base = PAD.top + plotH;
    return `${path(points)}L${x(points[points.length - 1].t).toFixed(1)},${base}L${x(points[0].t).toFixed(1)},${base}Z`;
  }

  const xTicks = $derived.by(() => {
    const n = plotW > 620 ? 6 : plotW > 400 ? 4 : 3;
    const span = bounds.tMax - bounds.tMin;
    return Array.from({ length: n }, (_, i) => bounds.tMin + (span * i) / (n - 1));
  });

  // ── Survol / curseur clavier ─────────────────────────────────────────────
  let cursorT = $state<number | null>(null);

  function nearest(points: { t: number; v: number }[], t: number) {
    if (points.length === 0) return null;
    let best = points[0];
    let bestD = Math.abs(points[0].t - t);
    for (const p of points) {
      const d = Math.abs(p.t - t);
      if (d < bestD) {
        bestD = d;
        best = p;
      }
    }
    return best;
  }

  function onMove(e: PointerEvent) {
    const rect = (e.currentTarget as SVGElement).getBoundingClientRect();
    const px = e.clientX - rect.left;
    const ratio = (px - PAD.left) / plotW;
    if (ratio < -0.02 || ratio > 1.02) {
      cursorT = null;
      return;
    }
    cursorT = bounds.tMin + Math.min(1, Math.max(0, ratio)) * (bounds.tMax - bounds.tMin);
  }

  function onKey(e: KeyboardEvent) {
    // Le clavier accède aux mêmes valeurs que le survol — une infobulle ne
    // doit jamais être le seul chemin vers un chiffre.
    const step = (bounds.tMax - bounds.tMin) / 40;
    if (e.key === 'ArrowRight') cursorT = Math.min(bounds.tMax, (cursorT ?? bounds.tMin) + step);
    else if (e.key === 'ArrowLeft') cursorT = Math.max(bounds.tMin, (cursorT ?? bounds.tMax) - step);
    else if (e.key === 'Home') cursorT = bounds.tMin;
    else if (e.key === 'End') cursorT = bounds.tMax;
    else if (e.key === 'Escape') cursorT = null;
    else return;
    e.preventDefault();
  }

  const hover = $derived.by(() => {
    if (cursorT == null) return null;
    const rows = drawn
      .map((s) => ({ serie: s, point: nearest(s.points, cursorT!) }))
      .filter((r) => r.point) as { serie: Serie; point: { t: number; v: number } }[];
    if (rows.length === 0) return null;
    const anchor = rows.reduce((a, b) =>
      Math.abs(a.point.t - cursorT!) <= Math.abs(b.point.t - cursorT!) ? a : b,
    );
    return { rows, t: anchor.point.t };
  });

  const fmt = (v: number) => v.toFixed(decimals);

  /**
   * Les graduations arrondissent plus que les valeurs : un axe qui affiche
   * « 600,0 / 400,0 » ajoute un chiffre qui ne dit rien. La précision reste
   * dans l'infobulle et le tableau.
   */
  const tickStep = $derived(ticks.length > 1 ? ticks[1] - ticks[0] : 1);
  const tickFmt = (v: number) =>
    v.toFixed(tickStep >= 10 ? 0 : tickStep >= 1 ? (Number.isInteger(v) ? 0 : 1) : 2);
  const tipLeft = $derived(
    hover ? Math.min(Math.max(x(hover.t), PAD.left + 8), width - PAD.right - 8) : 0,
  );
</script>

<div class="wrap" bind:this={wrap}>
  <!--
    Le graphique est volontairement focusable et écoute le clavier : c'est le
    seul moyen d'atteindre les valeurs sans souris, et le tableau ne remplace
    pas la lecture point par point. La règle a11y de Svelte vise les images
    décoratives rendues cliquables, pas ce cas.
  -->
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <svg
    {width}
    {height}
    role="img"
    tabindex="0"
    aria-label={series.map((s) => `${s.label} (${unit})`).join(', ')}
    onpointermove={onMove}
    onpointerleave={() => (cursorT = null)}
    onkeydown={onKey}
    onblur={() => (cursorT = null)}
  >
    <!-- Grille : filets pleins d'un cran au-dessus du fond, jamais pointillés -->
    {#each ticks as tick (tick)}
      <line x1={PAD.left} x2={width - PAD.right} y1={y(tick)} y2={y(tick)} class="grid" />
      <text x={PAD.left - 8} y={y(tick) + 3.5} class="ytick">{tickFmt(tick)}</text>
    {/each}

    {#each xTicks as t, i (i)}
      <text
        x={x(t)}
        y={height - 6}
        class="xtick"
        text-anchor={i === 0 ? 'start' : i === xTicks.length - 1 ? 'end' : 'middle'}
      >
        {axisTime(t, lang, bounds.tMax - bounds.tMin)}
      </text>
    {/each}

    {#each drawn as s (s.key)}
      {#if drawn.length === 1}
        <path d={areaPath(s.points)} fill={s.color} fill-opacity="0.1" />
      {/if}
      <path
        d={path(s.points)}
        fill="none"
        stroke={s.color}
        stroke-width="2"
        stroke-linejoin="round"
        stroke-linecap="round"
      />
      {#if s.points.length}
        <!-- Marqueur de fin : anneau de 2 px en couleur de surface pour rester
             lisible quand deux séries se croisent. -->
        <circle
          cx={x(s.points[s.points.length - 1].t)}
          cy={y(s.points[s.points.length - 1].v)}
          r="4"
          fill={s.color}
          stroke="var(--lp-chart-surface)"
          stroke-width="2"
        />
      {/if}
    {/each}

    {#if hover}
      <line x1={x(hover.t)} x2={x(hover.t)} y1={PAD.top} y2={PAD.top + plotH} class="cursor" />
      {#each hover.rows as r (r.serie.key)}
        <circle
          cx={x(r.point.t)}
          cy={y(r.point.v)}
          r="4"
          fill={r.serie.color}
          stroke="var(--lp-chart-surface)"
          stroke-width="2"
        />
      {/each}
    {/if}
  </svg>

  {#if hover}
    <div class="tip" style:left="{tipLeft}px" role="status">
      <div class="tip-t lp-mono">{tooltipTime(hover.t, lang)}</div>
      {#each hover.rows as r (r.serie.key)}
        <div class="tip-row">
          <span class="swatch" style:background={r.serie.color}></span>
          <span class="tip-lbl">{r.serie.label}</span>
          <span class="tip-val lp-mono">{fmt(r.point.v)} {unit}</span>
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .wrap {
    position: relative;
    width: 100%;
    /* Le conteneur inclut la bande d'axe : sinon la carte se met à défiler
       verticalement sur 20 px, ce qui est toujours un bug. */
  }
  svg {
    display: block;
    width: 100%;
    touch-action: pan-y;
  }
  svg:focus-visible {
    outline: 2px solid var(--ep-accent-bright);
    outline-offset: 2px;
  }
  .grid {
    stroke: var(--lp-grid);
    stroke-width: 1;
  }
  .cursor {
    stroke: var(--ep-text-muted);
    stroke-width: 1;
  }
  .ytick,
  .xtick {
    font-family: var(--ep-font-mono);
    font-size: 9px;
    fill: var(--ep-text-muted);
    font-variant-numeric: tabular-nums;
  }
  .ytick {
    text-anchor: end;
  }

  .tip {
    position: absolute;
    top: 4px;
    transform: translateX(-50%);
    background: var(--ep-bg-tertiary);
    border: 1px solid var(--ep-border-strong);
    border-radius: var(--ep-radius-md);
    padding: 6px 8px;
    font-size: 11px;
    pointer-events: none;
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.3);
    white-space: nowrap;
    z-index: 2;
  }
  .tip-t {
    font-size: 9.5px;
    color: var(--ep-text-muted);
    margin-bottom: 3px;
  }
  .tip-row {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .swatch {
    width: 8px;
    height: 3px;
    border-radius: 2px;
    flex-shrink: 0;
  }
  /* Le texte porte des tokens de texte ; l'identité vient de la pastille. */
  .tip-lbl {
    color: var(--ep-text-secondary);
  }
  .tip-val {
    color: var(--ep-text-primary);
    font-weight: 600;
    margin-left: auto;
  }
</style>
