<script lang="ts">
  import { _ } from 'svelte-i18n';
  import { canOperate } from '$lib/session';
  import ProbeRow from './ProbeRow.svelte';
  import ActionMenu from './ActionMenu.svelte';
  import { livenessCounts } from '$lib/fleet-filter';
  import { heartbeatCells, gatewayLatency, gatewayLoss, STRIP_SPAN_MS } from '$lib/heartbeat';
  import type { ProbeBeats } from '$lib/fleet-beats';
  import type { Probe } from '$lib/api';

  interface Props {
    siteId: string;
    siteName: string;
    probes: Probe[];
    /** La version de référence — pour signaler celles qui traînent. */
    newest?: string | null;
    now: number;
    /** Bande de battement et taux 24 h, lus à part et lentement (`fleet-beats`). */
    beats: Record<string, ProbeBeats>;
    /** Nombre de colonnes du tableau : l'en-tête de site les couvre toutes. */
    cols: number;
    expanded: boolean;
    ontoggle: () => void;
    onrename: () => void;
    /** Ouvre les alertes DU SITE : c'est ici qu'on décide qui réveille qui. */
    onalerts: () => void;
    /** Site retiré du parc, mais rien n'a été supprimé. */
    archived?: boolean;
    onarchive: () => void;
    onfocusSite: () => void;
    /** Ouvre le rapport SLA du site — la sélection des sondes se fait là. */
    onexportSla: () => void;
    /** Vrai quand un filtre est actif : le vide « site sans sonde » ne s'applique plus. */
    filtered: boolean;
  }
  let {
    siteId,
    siteName,
    probes,
    newest = null,
    now,
    beats,
    cols,
    expanded,
    ontoggle,
    onrename,
    onalerts,
    archived = false,
    onarchive,
    onfocusSite,
    onexportSla,
    filtered,
  }: Props = $props();

  /**
   * Compteurs de l'en-tête, calculés comme les lignes et par le même module.
   * ⚠️ Pas `p.status` : l'en-tête compterait « en ligne » des sondes que ses
   * propres lignes montrent silencieuses.
   */
  const counts = $derived(livenessCounts(probes, now));
  const bodyId = $derived(`site-body-${siteId}`);

  const health = $derived(
    probes.length === 0
      ? $_('fleet.site_empty_short')
      : counts.online === probes.length
        ? $_('fleet.site_healthy')
        : $_('fleet.site_health', { values: { online: counts.online, total: probes.length } }),
  );

  // Les bandes partagent la MÊME fenêtre, arrondie au quart d'heure : c'est ce
  // qui aligne les colonnes d'une ligne à l'autre.
  const to = $derived(Math.ceil(now / 900_000) * 900_000);

  function cellsOf(p: Probe) {
    return heartbeatCells({
      points: beats[p.probe_id]?.states ?? [],
      from: to - STRIP_SPAN_MS,
      to,
      createdAt: p.created_at,
      lastSeen: p.last_seen,
      atMs: now,
      realtimeUntil: p.realtime_until,
    });
  }
</script>

<tbody>
  <tr class="site">
    <!-- Une seule cellule sur toute la largeur : les colonnes masquées sur
         écran étroit ne décalent donc jamais l'en-tête de site. -->
    <td colspan={cols}><div class="line">
      <button
        class="toggle"
        onclick={ontoggle}
        aria-expanded={expanded}
        aria-controls={bodyId}
        aria-label={expanded
          ? $_('fleet.site_collapse', { values: { name: siteName } })
          : $_('fleet.site_expand', { values: { name: siteName } })}
      >
        <svg class="chev" class:open={expanded} width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <polyline points="6,3 11,8 6,13" />
        </svg>
        <span class="sname">{siteName}</span>
      </button>
      <span class="shealth">{health}</span>
      {#if archived}
        <span class="lp-tag">{$_('site.archived_badge')}</span>
      {/if}
      <span class="acts">
      {#if $canOperate}
        <a
          class="lp-btn ghost sm icon"
          href="#/add/{encodeURIComponent(siteId)}"
          title={$_('fleet.site_add', { values: { name: siteName } })}
          aria-label={$_('fleet.site_add', { values: { name: siteName } })}
        >
          <svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" aria-hidden="true"><path d="M8 3v10M3 8h10" /></svg>
        </a>
      {/if}
      <ActionMenu icon="more" label={$_('fleet.site_menu', { values: { name: siteName } })}>
        {#snippet children(close)}
          <button class="mi" role="menuitem" onclick={() => (close(), onfocusSite())}>
            {$_('fleet.site_open')}
          </button>
          <!-- Le rapport se demande depuis le SITE : c'est l'unité qu'on remet
               à un client. Il reste ouvert à un lecteur : le produire ne
               change rien au parc. -->
          <button class="mi" role="menuitem" onclick={() => (close(), onexportSla())}>
            {$_('sla.export')}
          </button>
          {#if $canOperate}
            <!-- ⚠️ Masquage ET vérification serveur (§ 18). -->
            <button class="mi" role="menuitem" onclick={() => (close(), onalerts())}>
              {$_('notify.site_alerts')}
            </button>
            <button class="mi" role="menuitem" onclick={() => (close(), onrename())}>
              {$_('fleet.site_rename')}
            </button>
            <button class="mi" role="menuitem" onclick={() => (close(), onarchive())}>
              {archived ? $_('site.unarchive') : $_('site.archive')}
            </button>
          {/if}
        {/snippet}
      </ActionMenu>
      </span>
    </div></td>
  </tr>
</tbody>

{#if expanded}
  <tbody id={bodyId}>
    {#if probes.length === 0 && !filtered}
      <tr class="empty">
        <td colspan={cols}>
          {$_('fleet.empty_site_title')}
          {#if $canOperate}
            <a class="lp-link" href="#/add/{encodeURIComponent(siteId)}">
              {$_('fleet.empty_site_cta')}
            </a>
          {/if}
        </td>
      </tr>
    {:else}
      {#each probes as p (p.probe_id)}
        <ProbeRow
          probe={p}
          {now}
          {newest}
          cells={cellsOf(p)}
          latency={gatewayLatency(p)}
          loss={gatewayLoss(p, beats[p.probe_id]?.uptime ?? {})}
        />
      {/each}
    {/if}
  </tbody>
{/if}

<style>
  tr.site td {
    height: 44px;
    background: var(--color-paper-3);
    border-bottom: 1px solid var(--color-rule-2);
  }
  /* L'en-tête de site ne réagit pas au survol des lignes. */
  tr.site:hover td {
    background: var(--color-paper-3);
  }
  .toggle {
    all: unset;
    display: inline-flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
    border-radius: 3px;
  }
  .toggle:focus-visible {
    outline: 2px solid var(--color-focus);
    outline-offset: 2px;
  }
  .chev {
    color: var(--color-muted);
    transition: transform 220ms cubic-bezier(0.16, 1, 0.3, 1);
  }
  .chev.open {
    transform: rotate(90deg);
  }
  .sname {
    font-weight: 600;
    font-size: 1rem;
  }
  .shealth {
    color: var(--color-muted);
    font-size: 0.8125rem;
    margin: 0 8px 0 12px;
  }
    .line {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    min-height: 44px;
  }
  .acts {
    margin-left: auto;
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  tr.empty td {
    color: var(--color-muted);
    font-size: 0.875rem;
  }
  tr.empty a {
    margin-left: 8px;
  }
  .mi {
    display: block;
    width: 100%;
    text-align: left;
    padding: 8px 10px;
    border: 0;
    border-radius: 5px;
    background: transparent;
    color: var(--color-ink);
    font: 500 0.875rem/1.3 var(--ep-font-sans);
    cursor: pointer;
  }
  .mi:hover,
  .mi:focus-visible {
    background: var(--color-paper-3);
    outline: none;
  }
</style>
