<script lang="ts">
  import { _, locale } from 'svelte-i18n';
  import { fmtNum } from '../format';
  import type { PingEntry } from '../stores/monitoring';

  /**
   * La latence d'une cible sur la fenêtre gardée par l'app, avec un réticule
   * qui lit chaque relevé. Mêmes conventions que les courbes du hub : grille
   * discrète, axe en ms, trous non reliés, pertes marquées en rouge au pied.
   */
  let { history, series = 1 }: { history: PingEntry[]; series?: number } = $props();

  const W = 720, H = 180, L = 40, R = 8, T = 8, B = 22;
  const lang = $derived($locale ?? 'en');
  const vals = $derived(history.map((p) => (p.alive ? p.latency_ms : null)));
  const max = $derived.by(() => {
    const m = Math.max(1, ...vals.filter((v): v is number => v != null));
    const step = m <= 5 ? 1 : m <= 20 ? 5 : m <= 50 ? 10 : m <= 200 ? 50 : 100;
    return Math.ceil((m * 1.1) / step) * step;
  });
  const t0 = $derived(history[0]?.timestamp ?? 0);
  const t1 = $derived(Math.max(t0 + 1, history[history.length - 1]?.timestamp ?? 1));
  const x = (t: number) => L + ((t - t0) / (t1 - t0)) * (W - L - R);
  const y = (v: number) => T + (1 - v / max) * (H - T - B);
  const ticks = $derived([0, 0.5, 1].map((f) => f * max));
  const segs = $derived.by(() => {
    const out: string[] = [];
    let cur: string[] = [];
    history.forEach((p) => {
      if (p.alive && p.latency_ms != null) cur.push(`${x(p.timestamp).toFixed(1)},${y(p.latency_ms).toFixed(1)}`);
      else if (cur.length) { out.push(cur.join(' ')); cur = []; }
    });
    if (cur.length) out.push(cur.join(' '));
    return out;
  });
  const hm = (t: number) => new Date(t * 1000).toLocaleTimeString(lang, { hour: '2-digit', minute: '2-digit', second: '2-digit' });

  let svg = $state<SVGSVGElement | null>(null);
  let hover = $state<number | null>(null);
  function move(e: PointerEvent) {
    if (!svg || history.length === 0) return;
    const r = svg.getBoundingClientRect();
    const px = ((e.clientX - r.left) / r.width) * W;
    if (px < L || px > W - R) { hover = null; return; }
    const t = t0 + ((px - L) / (W - L - R)) * (t1 - t0);
    let best = 0;
    for (let i = 1; i < history.length; i++) {
      if (Math.abs(history[i].timestamp - t) < Math.abs(history[best].timestamp - t)) best = i;
    }
    hover = best;
  }
  const hp = $derived(hover != null ? history[hover] : null);
</script>

<div class="chart-wrap">
  {#if history.length < 2}
    <p class="muted">{$_('monitoring.chart_empty')}</p>
  {:else}
    <svg bind:this={svg} class="chart" viewBox="0 0 {W} {H}" role="img" aria-label={$_('monitoring.chart_label')}
         onpointermove={move} onpointerleave={() => (hover = null)}>
      <g class="grid">
        {#each ticks as v}<line x1={L} x2={W - R} y1={y(v)} y2={y(v)} />{/each}
      </g>
      <g class="ax">
        {#each ticks as v}<text x={L - 6} y={y(v) + 4} text-anchor="end">{fmtNum(v, lang, 0)}</text>{/each}
        <text x={L} y={H - 4}>{hm(t0)}</text>
        <text x={W - R} y={H - 4} text-anchor="end">{hm(t1)}</text>
      </g>
      {#each history as p}
        {#if !p.alive}<rect class="ib" x={x(p.timestamp) - 1.5} y={H - B - 4} width="3" height="4" />{/if}
      {/each}
      {#each segs as s}<polyline class="line s{series}" points={s} />{/each}
      {#if hp}<line class="cur" x1={x(hp.timestamp)} x2={x(hp.timestamp)} y1={T} y2={H - B} />{/if}
    </svg>
    {#if hp && hover != null}
      <div class="tip" style="left: min(calc({(x(hp.timestamp) / W) * 100}% + 14px), calc(100% - 170px)); top: 8px">
        <div class="tt">{hm(hp.timestamp)}</div>
        <div class="tr">
          <span class="sw s{series}"></span>{$_('monitoring.col_latency')}
          <b>{hp.alive ? `${fmtNum(hp.latency_ms, lang)} ms` : $_('status.target_down')}</b>
        </div>
      </div>
    {/if}
  {/if}
</div>
