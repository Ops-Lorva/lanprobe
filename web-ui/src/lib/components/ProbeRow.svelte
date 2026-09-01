<script lang="ts">
  import { _, locale } from 'svelte-i18n';
  import { platformLabel } from '$lib/format';
  import StatusMark from './StatusMark.svelte';
  import BufferBadge from './BufferBadge.svelte';
  import { relativeTime, absoluteTime, secondsLeft, countdown } from '$lib/time';
  import type { Probe } from '$lib/api';

  interface Props {
    probe: Probe;
    /** Horloge partagée : le délai relatif doit vieillir sans rechargement. */
    now: number;
    /** Affiche le site sous le nom — utile hors du regroupement par site. */
    showSite?: boolean;
  }
  let { probe, now, showSite = false }: Props = $props();

  const lang = $derived($locale ?? 'en');
  const seen = $derived(relativeTime(probe.last_seen, lang, now));

  /**
   * Temps restant de mode temps réel, `0` quand il ne tourne pas.
   *
   * ⚠️ C'est le garde-fou contre l'oubli, pas une décoration : on redescend de
   * l'échelle, la caméra marche, on passe à la suivante — on ne revient pas
   * éteindre le mode. Ce qui tourne doit se voir depuis le parc, avec sa fin.
   */
  const realtimeLeft = $derived(secondsLeft(probe.realtime_until, now));

  /**
   * `linux` → `Linux`, `macos` → `macOS`. Une simple capitalisation donnerait
   * « Macos », qui est faux : les noms de plateformes ont une graphie, autant
   * la respecter.
   */
</script>

<!--
  La ligne entière est un lien : cible large, ouverture en nouvel onglet, et
  navigation clavier sans piège. Les métadonnées restent en chasse fixe pour
  que versions et plateformes s'alignent verticalement — c'est ce qui permet
  de repérer la sonde restée en 1.1.5 sans lire les autres.
-->
<a
  class="row {probe.status}"
  class:buffering={probe.buffered_points > 0}
  href="#/probes/{encodeURIComponent(probe.probe_id)}"
  aria-label={$_('fleet.open', { values: { name: probe.name } })}
>
  <span class="name">
    <span class="nm">{probe.name}</span>
    {#if showSite}<span class="site">{probe.site}</span>{/if}
  </span>

  <!-- ⚠️ Le repère vit DANS la cellule de statut, il n'ajoute pas de colonne :
       une cellule conditionnelle décalerait toute la grille quand elle
       apparaît, et l'en-tête ne correspondrait plus aux lignes.

       Une sonde peut battre parfaitement alors que son lien mesuré est mort —
       battement et mesures ne passent pas par la même requête. Un voyant vert
       seul cacherait exactement ce cas. Le repère ne s'affiche que sur une
       sonde EN LIGNE : sur une sonde muette, l'état internet est une valeur
       figée qui n'apprend rien de plus que le statut. -->
  <span class="status">
    <StatusMark status={probe.status} />
    {#if probe.status === 'online' && probe.internet_state && probe.internet_state !== 'online'}
      <span
        class="netflag"
        class:limited={probe.internet_state === 'limited'}
        title={$_('fleet.net_help')}
      >{$_(`charts.internet_${probe.internet_state}`)}</span>
    {/if}
    <!-- ⚠️ Le repère s'affiche quel que soit le statut, contrairement à celui
         d'internet : une sonde muette peut très bien avoir été armée juste
         avant de se taire, et c'est précisément le cas qu'on doit voir. -->
    {#if realtimeLeft > 0}
      <span
        class="rtflag"
        title={$_('fleet.realtime_help', { values: { left: countdown(realtimeLeft, lang) } })}
      >{$_('fleet.realtime_flag', { values: { left: countdown(realtimeLeft, lang) } })}</span>
    {/if}
  </span>

  <span
    class="seen lp-mono"
    title={probe.last_seen ? absoluteTime(probe.last_seen, lang) : $_('status.never_seen')}
  >
    {probe.last_seen ? seen : $_('common.none')}
  </span>

  <span class="pubip lp-mono" title={probe.public_ip ?? ''}>{probe.public_ip || '—'}</span>
  <span class="platform lp-mono">{platformLabel(probe.platform, $_('common.none'))}</span>
  <span class="version lp-mono">{probe.version || $_('common.none')}</span>
  <span class="buffer"><BufferBadge points={probe.buffered_points} live={probe.status === 'online'} /></span>

  <span class="chev" aria-hidden="true">
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
      <polyline points="6,3 11,8 6,13" />
    </svg>
  </span>
</a>

<style>
  .row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 118px 120px 118px 92px 84px 78px 18px;
    align-items: center;
    gap: 10px;
    padding: 9px 12px 9px 9px;
    border-left: 3px solid transparent;
    border-bottom: 1px solid var(--ep-border);
    color: var(--ep-text-primary);
    text-decoration: none;
    font-size: 12px;
    transition: background 0.12s;
  }
  .row:last-child {
    border-bottom: none;
  }
  .row:hover {
    background: var(--ep-bg-hover);
  }

  /* Rail de statut : la couleur touche le bord de la ligne, donc toutes les
     lignes s'alignent sur la même colonne d'un pixel. Sur trente lignes, les
     hors ligne dessinent une trame verticale repérable sans rien lire. */
  .row.online {
    border-left-color: color-mix(in srgb, var(--ep-success) 55%, transparent);
  }
  .row.stale {
    border-left-color: var(--ep-warning);
  }
  .row.offline {
    border-left-color: var(--ep-danger);
    /* La teinte de fond est le second canal : elle porte le bloc entier, pas
       un point de 6 px, et survit à un écran mal calibré. */
    background: color-mix(in srgb, var(--ep-danger) 6%, transparent);
  }
  .row.offline:hover {
    background: color-mix(in srgb, var(--ep-danger) 11%, transparent);
  }

  .name {
    display: flex;
    flex-direction: column;
    min-width: 0;
    gap: 1px;
  }
  .nm {
    font-weight: 600;
    font-size: 12.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .site {
    font-size: 10px;
    color: var(--ep-text-muted);
  }

  .seen,
  .status {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    min-width: 0;
  }
  /* La cellule de statut peut porter deux repères ; elle passe à la ligne
     plutôt que de déborder sur la colonne voisine. */
  .status {
    flex-wrap: wrap;
    row-gap: 2px;
  }
  .netflag {
    padding: 1px 5px;
    border-radius: 3px;
    font-size: 9.5px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    white-space: nowrap;
    color: var(--ep-danger);
    border: 1px solid var(--ep-danger);
  }
  .netflag.limited {
    color: var(--ep-warning, #f59e0b);
    border-color: var(--ep-warning, #f59e0b);
  }
  /* Le temps restant est DANS le repère, pas seulement en infobulle : sur
     tactile il n'y a pas de survol, et « ça tourne encore longtemps ? » est
     exactement la question qui empêche l'oubli. */
  .rtflag {
    padding: 1px 5px;
    border-radius: 3px;
    font-size: 9.5px;
    white-space: nowrap;
    color: var(--ep-accent-bright);
    border: 1px solid var(--ep-accent);
    background: var(--ep-accent-dim);
    cursor: help;
  }
  .pubip,
  .platform,
  .version {
    color: var(--ep-text-secondary);
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .platform,
  .version {
    color: var(--ep-text-muted);
  }
  .offline .seen {
    color: var(--ep-danger);
  }

  .chev {
    color: var(--ep-text-dim);
    display: flex;
    justify-content: flex-end;
  }
  .row:hover .chev {
    color: var(--ep-accent-bright);
  }

  /* Les colonnes de contexte tombent avant les colonnes de décision : on perd
     la plateforme et la version bien avant le statut et le tampon. */
  @media (max-width: 1080px) {
    .row {
      grid-template-columns: minmax(0, 1fr) 118px 120px 118px 84px 78px 18px;
    }
    .platform {
      display: none;
    }
  }
  @media (max-width: 900px) {
    .row {
      grid-template-columns: minmax(0, 1fr) 118px 110px 78px 18px;
    .pubip { display: none; }
    }
    .version {
      display: none;
    }
  }
  @media (max-width: 640px) {
    .row {
      grid-template-columns: minmax(0, 1fr) auto;
      grid-template-areas:
        'name status'
        'meta buffer';
      row-gap: 4px;
      padding: 11px 12px 11px 9px;
    }
    .name {
      grid-area: name;
    }
    .status {
      grid-area: status;
      justify-self: end;
    }
    .seen {
      grid-area: meta;
    }
    .buffer {
      grid-area: buffer;
      justify-self: end;
    }
    .chev {
      display: none;
    }
  }
</style>
