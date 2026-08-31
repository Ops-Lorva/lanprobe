<script lang="ts">
  import { _, locale } from 'svelte-i18n';
  import { axisTime, tooltipTime } from '$lib/time';
  import { normalizeState, type InternetState } from '$lib/charts';
  import type { Point } from '$lib/metrics';

  interface Props {
    points: Point[];
    height?: number;
    /**
     * Fenêtre de temps à représenter (cf. `TimeSeriesChart`). Les trois
     * graphiques d'une fiche doivent partager exactement le même axe, sans
     * quoi on croit lire trois périodes différentes.
     */
    domain?: { from: number; to: number };
    /**
     * Changements d'IP publique à marquer, **en millisecondes** comme les
     * points : le hub, lui, compte en secondes, la conversion se fait chez
     * l'appelant. Un repère daté de 1970 ne lève aucune erreur, il se pose
     * simplement à gauche du graphe.
     *
     * ⚠️ Facultatif : sans cette prop le composant se comporte exactement
     * comme avant, ce qui garde ses autres usages intacts.
     */
    changes?: { at: number; ip: string; label: string | null }[];
  }
  let { points, height = 74, domain, changes = [] }: Props = $props();

  const lang = $derived($locale ?? 'en');

  // Même gouttière gauche que TimeSeriesChart : les trois graphiques d'une
  // fiche partagent la même fenêtre de temps, leurs axes doivent donc tomber
  // sur la même verticale. Décalés, on croit lire deux périodes différentes.
  const PAD = { top: 6, right: 14, bottom: 20, left: 46 };
  let wrap = $state<HTMLDivElement | null>(null);
  let width = $state(720);

  $effect(() => {
    if (!wrap) return;
    const ro = new ResizeObserver(([e]) => (width = Math.max(280, Math.round(e.contentRect.width))));
    ro.observe(wrap);
    return () => ro.disconnect();
  });

  const plotW = $derived(Math.max(10, width - PAD.left - PAD.right));
  const plotH = $derived(Math.max(10, height - PAD.top - PAD.bottom));

  interface Segment {
    from: number;
    to: number;
    state: InternetState | 'gap';
  }

  /**
   * Un état ne se trace pas comme une courbe : entre deux relevés il ne varie
   * pas, il dure. On agrège donc en segments, et on coupe explicitement quand
   * l'écart entre deux relevés dépasse largement la cadence habituelle — un
   * segment étiré sur une coupure d'agent affirmerait « en ligne » pendant une
   * période où personne ne mesurait.
   */
  const segments = $derived.by<Segment[]>(() => {
    const pts = points
      .map((p) => ({ t: p.t, s: normalizeState(p.v) }))
      .sort((a, b) => a.t - b.t);
    if (pts.length === 0) return [];

    const deltas = pts.slice(1).map((p, i) => p.t - pts[i].t).sort((a, b) => a - b);
    const median = deltas.length ? deltas[Math.floor(deltas.length / 2)] : 60_000;
    const gapLimit = Math.max(median * 4, 90_000);

    const out: Segment[] = [];
    for (let i = 0; i < pts.length; i++) {
      const from = pts[i].t;
      const nextT = i + 1 < pts.length ? pts[i + 1].t : from + median;
      const to = Math.min(nextT, from + gapLimit);
      const last = out[out.length - 1];
      if (last && last.state === pts[i].s && Math.abs(last.to - from) < 1) last.to = to;
      else out.push({ from, to, state: pts[i].s });
      if (nextT - from > gapLimit) out.push({ from: to, to: nextT, state: 'gap' });
    }
    return out;
  });

  const bounds = $derived.by(() => {
    if (domain && domain.to > domain.from) return { tMin: domain.from, tMax: domain.to };
    if (segments.length === 0) return { tMin: 0, tMax: 1 };
    const tMin = segments[0].from;
    const tMax = Math.max(segments[segments.length - 1].to, tMin + 1);
    return { tMin, tMax };
  });

  const x = (t: number) => PAD.left + ((t - bounds.tMin) / (bounds.tMax - bounds.tMin)) * plotW;

  /**
   * Second canal : la hauteur. Elle monte avec la gravité, si bien que la
   * bande se lit en niveaux de gris, à l'impression, et pour un daltonien.
   * La couleur seule ne porte jamais l'information.
   */
  const RATIO: Record<Segment['state'], number> = {
    online: 0.34,
    limited: 0.68,
    offline: 1,
    unknown: 0.5,
    gap: 0.12,
  };

  const uptime = $derived.by(() => {
    let online = 0;
    let known = 0;
    for (const s of segments) {
      if (s.state === 'gap') continue;
      const d = s.to - s.from;
      known += d;
      if (s.state === 'online') online += d;
    }
    return known > 0 ? online / known : null;
  });

  const xTicks = $derived.by(() => {
    const n = plotW > 620 ? 6 : 4;
    const span = bounds.tMax - bounds.tMin;
    return Array.from({ length: n }, (_, i) => bounds.tMin + (span * i) / (n - 1));
  });

  const legend: InternetState[] = ['online', 'limited', 'offline'];

  /**
   * Un repère hors de la fenêtre affichée se dessinerait à cheval sur les
   * axes : on ne garde que ceux qui tombent dedans.
   */
  const visibleChanges = $derived(
    changes.filter((c) => c.at >= bounds.tMin && c.at <= bounds.tMax),
  );

  const changeLabel = (c: { at: number; ip: string; label: string | null }) =>
    `${c.label ? `${c.label} — ${c.ip}` : c.ip} · ${tooltipTime(c.at, lang)}`;

  function segLabel(s: Segment) {
    const name = s.state === 'gap' ? $_('charts.internet_unknown') : $_(`charts.internet_${s.state}`);
    return `${name} — ${tooltipTime(s.from, lang)} → ${tooltipTime(s.to, lang)}`;
  }
</script>

<div class="wrap" bind:this={wrap}>
  <svg {width} {height} role="img" aria-label={$_('charts.internet_title')}>
    <line
      x1={PAD.left}
      x2={width - PAD.right}
      y1={PAD.top + plotH}
      y2={PAD.top + plotH}
      class="base"
    />
    {#each segments as s, i (i)}
      {@const h = Math.max(2, plotH * RATIO[s.state])}
      {@const w = Math.max(1.2, x(s.to) - x(s.from))}
      <rect
        x={x(s.from)}
        y={PAD.top + plotH - h}
        width={w}
        height={h}
        class="seg {s.state}"
        rx="1"
      >
        <title>{segLabel(s)}</title>
      </rect>
    {/each}

    <!--
      Les repères passent APRÈS les segments : dessinés avant, ils seraient
      recouverts par la bande et invisibles là où elle est la plus haute,
      c'est-à-dire pendant les coupures — exactement les moments où l'on
      cherche à savoir sur quelle adresse on était.
    -->
    {#each visibleChanges as c (c.at)}
      <g class="ip-change">
        <!-- Trait large et transparent : le survol d'une ligne d'un pixel se
             rate une fois sur deux, et l'adresse ne s'atteindrait plus. -->
        <line
          x1={x(c.at)}
          x2={x(c.at)}
          y1={PAD.top}
          y2={height - PAD.bottom}
          class="hit"
        />
        <line
          x1={x(c.at)}
          x2={x(c.at)}
          y1={PAD.top}
          y2={height - PAD.bottom}
          class="mark"
        />
        <title>{changeLabel(c)}</title>
      </g>
    {/each}

    {#each xTicks as t, i (i)}
      <text
        x={x(t)}
        y={height - 5}
        class="xtick"
        text-anchor={i === 0 ? 'start' : i === xTicks.length - 1 ? 'end' : 'middle'}
      >
        {axisTime(t, lang, bounds.tMax - bounds.tMin)}
      </text>
    {/each}
  </svg>

  <div class="foot">
    <div class="legend">
      {#each legend as s (s)}
        <span class="key">
          <span class="chip {s}" style:height="{4 + RATIO[s] * 8}px"></span>
          <span class="cap">{$_(`charts.internet_${s}`)}</span>
        </span>
      {/each}
    </div>
    {#if uptime != null}
      <span class="uptime">
        {$_('charts.uptime', {
          values: { pct: new Intl.NumberFormat(lang, { style: 'percent', maximumFractionDigits: 1 }).format(uptime) },
        })}
      </span>
    {/if}
  </div>
</div>

<style>
  .wrap {
    width: 100%;
  }
  svg {
    display: block;
    width: 100%;
  }
  .base {
    stroke: var(--lp-grid);
    stroke-width: 1;
  }
  .seg.online {
    fill: var(--ep-success);
  }
  .seg.limited {
    fill: var(--ep-warning);
  }
  .seg.offline {
    fill: var(--ep-danger);
  }
  .seg.unknown,
  .seg.gap {
    fill: var(--ep-text-dim);
  }
  .ip-change .mark {
    stroke: var(--ep-accent);
    stroke-width: 1;
    stroke-dasharray: 3 2;
    opacity: 0.75;
  }
  .ip-change .hit {
    stroke: transparent;
    stroke-width: 9;
  }
  .ip-change:hover .mark {
    opacity: 1;
  }
  .xtick {
    font-family: var(--ep-font-mono);
    font-size: 9px;
    fill: var(--ep-text-muted);
  }

  .foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
    margin-top: 4px;
  }
  .legend {
    display: flex;
    gap: 12px;
    flex-wrap: wrap;
  }
  .key {
    display: inline-flex;
    align-items: flex-end;
    gap: 5px;
    font-size: 11px;
  }
  .chip {
    width: 9px;
    border-radius: 1px;
    display: block;
  }
  .chip.online {
    background: var(--ep-success);
  }
  .chip.limited {
    background: var(--ep-warning);
  }
  .chip.offline {
    background: var(--ep-danger);
  }
  .cap {
    color: var(--ep-text-secondary);
  }
  .uptime {
    font-size: 11px;
    color: var(--ep-text-secondary);
    font-variant-numeric: tabular-nums;
  }
</style>
