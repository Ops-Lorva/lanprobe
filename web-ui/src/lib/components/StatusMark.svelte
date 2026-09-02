<script lang="ts">
  import { _ } from 'svelte-i18n';
  import type { ProbeStatus } from '$lib/api';
  import type { LivenessState } from '$lib/probe-liveness';

  interface Props {
    /**
     * ⚠️ `silent` n'est pas un statut du hub : c'est l'état calculé dans le
     * navigateur quand une sonde n'a plus battu depuis plus d'une cadence,
     * sans avoir atteint le seuil de « hors ligne ». Il se rend en NEUTRE et
     * non en avertissement — on ne sait pas, on ne prétend pas savoir.
     */
    status: ProbeStatus | LivenessState;
    size?: number;
    /** Le libellé est affiché par défaut : la couleur seule ne suffit jamais. */
    withLabel?: boolean;
  }
  let { status, size = 12, withLabel = true }: Props = $props();

  /**
   * Trois canaux, pas un.
   *
   * Une pastille verte et une pastille rouge sont le même gris pour un
   * daltonien deutan, et sur un parc de trente lignes personne ne lit le texte
   * de chaque statut. On encode donc l'état trois fois :
   *   — la FORME du glyphe (disque plein / anneau vide / disque barré),
   *   — la couleur d'état,
   *   — le libellé texte.
   * Le glyphe seul suffit à balayer la colonne, y compris en niveaux de gris.
   */
</script>

<span class="mark {status}" title={$_(`status.${status}_help`)}>
  <svg width={size} height={size} viewBox="0 0 12 12" aria-hidden="true">
    {#if status === 'online'}
      <circle cx="6" cy="6" r="4" fill="currentColor" />
    {:else if status === 'stale'}
      <circle cx="6" cy="6" r="3.7" fill="none" stroke="currentColor" stroke-width="2" />
    {:else if status === 'silent'}
      <!-- Anneau fin et pointillé : le disque plein dit « présente », le
           disque barré dit « absente ». Ici on ne dit ni l'un ni l'autre, et
           la forme doit se lire comme telle en niveaux de gris. -->
      <circle
        cx="6"
        cy="6"
        r="3.7"
        fill="none"
        stroke="currentColor"
        stroke-width="1.6"
        stroke-dasharray="2.6 2.2"
      />
    {:else}
      <circle cx="6" cy="6" r="4.4" fill="none" stroke="currentColor" stroke-width="1.4" />
      <line
        x1="3"
        y1="9"
        x2="9"
        y2="3"
        stroke="currentColor"
        stroke-width="1.8"
        stroke-linecap="round"
      />
    {/if}
  </svg>
  {#if withLabel}
    <span class="txt">{$_(`status.${status}`)}</span>
  {:else}
    <span class="lp-sr">{$_(`status.${status}`)}</span>
  {/if}
</span>

<style>
  .mark {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 11.5px;
    font-weight: 600;
    white-space: nowrap;
  }
  .mark svg {
    flex-shrink: 0;
  }
  .online {
    color: var(--ep-success);
  }
  .stale {
    color: var(--ep-warning);
  }
  /* Neutre, jamais vert ni orange : le vert affirmerait une disponibilité
     inconnue, l'orange affirmerait une panne tout aussi inconnue. */
  .silent {
    color: var(--ep-text-muted);
  }
  .offline {
    color: var(--ep-danger);
  }
  .txt {
    color: var(--ep-text-secondary);
    font-weight: 500;
  }
  /* Hors ligne : le seul état qui garde son libellé en couleur pleine, parce
     que c'est le seul qui appelle une action. */
  .offline .txt {
    color: var(--ep-danger);
    font-weight: 600;
  }
</style>
