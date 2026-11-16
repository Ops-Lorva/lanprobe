<script lang="ts">
  import { _ } from 'svelte-i18n';
  import type { FleetFilter } from '$lib/fleet-filter';
  import type { DisplayTotals } from '$lib/probe-state';

  interface Props {
    sites: number;
    total: number;
    totals: DisplayTotals;
    /** Le filtre en cours : la case qui le porte est marquée. */
    filter: FleetFilter;
    /** Une case se clique : elle filtre le parc sur ce qu'elle compte. */
    onpick: (f: FleetFilter) => void;
  }
  let { sites, total, totals, filter, onpick }: Props = $props();

  const attention = $derived(totals.degraded + totals.silent);
</script>

<!--
  Le relevé chiffré du parc. Les compteurs à zéro restent sans couleur : un
  « 0 hors ligne » en rouge apprend à l'œil à ignorer le rouge.
-->
<dl class="rail">
  <div><dt>{$_('fleet.rail_sites')}</dt><dd>{sites}</dd></div>
  <div>
    <dt>{$_('fleet.rail_total')}</dt>
    <dd><button onclick={() => onpick('all')}>{total}</button></dd>
  </div>
  <div class:ok={totals.online > 0}>
    <dt>{$_('fleet.rail_online')}</dt>
    <dd>
      <button class:on={filter === 'online'} onclick={() => onpick('online')}>
        {#if totals.online > 0}<svg viewBox="0 0 10 10" aria-hidden="true"><circle cx="5" cy="5" r="4.2" fill="currentColor" /></svg>{/if}
        {totals.online}
      </button>
    </dd>
  </div>
  <div class:warn={attention > 0}>
    <dt>{$_('fleet.rail_attention')}</dt>
    <dd>
      <button class:on={filter === 'attention'} onclick={() => onpick('attention')}>
        {#if attention > 0}<svg viewBox="0 0 10 10" aria-hidden="true"><circle cx="5" cy="5" r="3.9" fill="none" stroke="currentColor" stroke-width="1.5" /><path d="M5 1.1A3.9 3.9 0 0 1 5 8.9Z" fill="currentColor" /></svg>{/if}
        {attention}
      </button>
    </dd>
  </div>
  <div class:bad={totals.offline > 0}>
    <dt>{$_('fleet.rail_offline')}</dt>
    <dd>
      <button class:on={filter === 'offline'} onclick={() => onpick('offline')}>
        {#if totals.offline > 0}<svg viewBox="0 0 10 10" aria-hidden="true"><rect x="1" y="1" width="8" height="8" rx="1" fill="currentColor" /></svg>{/if}
        {totals.offline}
      </button>
    </dd>
  </div>
  <div>
    <dt>{$_('fleet.rail_never')}</dt>
    <dd>
      <button class:on={filter === 'never'} onclick={() => onpick('never')}>{totals.never}</button>
    </dd>
  </div>
</dl>

<style>
  .rail {
    display: flex;
    flex-wrap: wrap;
    border-block: 1px solid var(--color-rule);
    margin: 0 0 24px;
  }
  .rail > div {
    padding: 12px 24px 12px 0;
    margin-right: 24px;
    border-right: 1px solid var(--color-rule);
    display: grid;
    gap: 1px;
    min-width: 0;
  }
  .rail > div:last-child {
    border-right: 0;
  }
  dt {
    font-size: 0.8125rem;
    color: var(--color-muted);
  }
  dd {
    margin: 0;
    font-size: 1.375rem;
    font-weight: 500;
    font-variant-numeric: tabular-nums;
    letter-spacing: -0.01em;
  }
  dd button {
    all: unset;
    display: inline-flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
    border-radius: 3px;
  }
  dd button:hover {
    text-decoration: underline;
    text-decoration-thickness: 1px;
    text-underline-offset: 0.2em;
  }
  dd button.on {
    text-decoration: underline;
    text-decoration-color: var(--color-accent);
    text-decoration-thickness: 2px;
    text-underline-offset: 0.25em;
  }
  dd button:focus-visible {
    outline: 2px solid var(--color-focus);
    outline-offset: 2px;
  }
  dd svg {
    width: 10px;
    height: 10px;
  }
  .ok svg {
    color: var(--st-ok);
  }
  .warn svg {
    color: var(--st-warn);
  }
  .bad svg {
    color: var(--st-bad);
  }
  .bad dd {
    color: var(--st-bad);
  }
</style>
