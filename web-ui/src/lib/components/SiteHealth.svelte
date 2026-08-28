<script lang="ts">
  import { _ } from 'svelte-i18n';
  import StatusMark from './StatusMark.svelte';
  import type { ProbeStatus } from '$lib/api';

  interface Props {
    counts: Record<ProbeStatus, number>;
    siteName: string;
    /** Somme des points en tampon du site — un site peut être « tout vert » et bourré. */
    buffered?: number;
  }
  let { counts, siteName, buffered = 0 }: Props = $props();

  const total = $derived(counts.online + counts.stale + counts.offline);

  /**
   * Ordre de lecture : ce qui va mal d'abord. Sur dix sites repliés, l'œil
   * balaie une colonne de compteurs — le rouge doit toujours occuper la même
   * position, la première, sinon il faut lire pour le trouver.
   */
  const order: ProbeStatus[] = ['offline', 'stale', 'online'];

  const label = $derived(
    order
      .filter((s) => counts[s] > 0)
      .map((s) => `${counts[s]} ${$_(`status.${s}`)}`)
      .join(', '),
  );
</script>

<div class="health">
  <!--
    Barre de proportion : elle sert à repérer un site en peine SANS le déplier.
    Sur un site de trente sondes, deux hors ligne représentent une encoche
    rouge visible à un mètre ; le compteur chiffré à côté donne la valeur
    exacte, la barre donne la proportion.
  -->
  <div
    class="bar"
    role="img"
    aria-label="{$_('fleet.health_bar', { values: { name: siteName } })} : {label}"
  >
    {#each order as s (s)}
      {#if counts[s] > 0}
        <span class="seg {s}" style:flex-grow={counts[s]}></span>
      {/if}
    {/each}
  </div>

  <div class="counters">
    {#if total === 0}
      <span class="muted">{$_('site.probe_count', { values: { n: 0 } })}</span>
    {:else if counts.offline === 0 && counts.stale === 0}
      <span class="ok"><StatusMark status="online" size={10} withLabel={false} /></span>
      <span class="ok-txt">{$_('fleet.site_healthy')}</span>
      <span class="muted">· {$_('site.probe_count', { values: { n: total } })}</span>
    {:else}
      {#each order as s (s)}
        {#if counts[s] > 0}
          <span class="counter {s}">
            <StatusMark status={s} size={10} withLabel={false} />
            <span class="n lp-mono">{counts[s]}</span>
            <span class="cap">{$_(`status.${s}`)}</span>
          </span>
        {/if}
      {/each}
    {/if}

    {#if buffered > 0}
      <span class="counter buffered" title={$_('buffer.pending', { values: { n: buffered } })}>
        <span class="dot" aria-hidden="true"></span>
        <span class="cap">{$_('buffer.label')}</span>
      </span>
    {/if}
  </div>
</div>

<style>
  .health {
    display: flex;
    align-items: center;
    gap: 12px;
    min-width: 0;
    flex-wrap: wrap;
  }

  .bar {
    display: flex;
    gap: 2px; /* l'espace de surface sépare les segments — pas une bordure */
    width: 108px;
    height: 6px;
    border-radius: 3px;
    overflow: hidden;
    background: var(--ep-bg-tertiary);
    flex-shrink: 0;
  }
  .seg {
    display: block;
    flex-basis: 0;
    min-width: 3px;
  }
  .seg.online {
    background: var(--ep-success);
  }
  .seg.stale {
    background: var(--ep-warning);
  }
  .seg.offline {
    background: var(--ep-danger);
  }

  .counters {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 11.5px;
    flex-wrap: wrap;
  }
  .counter {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .n {
    font-weight: 700;
    color: var(--ep-text-primary);
  }
  .cap {
    color: var(--ep-text-secondary);
  }
  .counter.offline .n,
  .counter.offline .cap {
    color: var(--ep-danger);
    font-weight: 600;
  }
  .ok-txt {
    color: var(--ep-text-secondary);
  }
  .ok {
    display: inline-flex;
  }
  .muted {
    color: var(--ep-text-muted);
  }
  .buffered .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--ep-warning);
    display: inline-block;
  }
  .buffered .cap {
    color: var(--ep-warning);
    font-weight: 600;
  }
</style>
