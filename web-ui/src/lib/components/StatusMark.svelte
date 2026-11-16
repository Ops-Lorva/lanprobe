<script lang="ts">
  import { _ } from 'svelte-i18n';
  import type { ProbeStatus } from '$lib/api';
  import type { LivenessState } from '$lib/probe-liveness';
  import type { DisplayState } from '$lib/probe-state';

  interface Props {
    /**
     * ⚠️ `silent` n'est pas un statut du hub : c'est l'état calculé dans le
     * navigateur quand une sonde n'a plus battu depuis plus d'une cadence,
     * sans avoir atteint le seuil de « hors ligne ». Il se rend en NEUTRE et
     * non en avertissement — on ne sait pas, on ne prétend pas savoir.
     */
    status: ProbeStatus | LivenessState | DisplayState;
    size?: number;
    /** Le libellé est affiché par défaut : la couleur seule ne suffit jamais. */
    withLabel?: boolean;
  }
  let { status, size = 10, withLabel = true }: Props = $props();

  /**
   * Trois canaux, pas un.
   *
   * Une pastille verte et une pastille rouge sont le même gris pour un
   * daltonien deutan, et sur un parc de trente lignes personne ne lit le texte
   * de chaque statut. On encode donc l'état trois fois :
   *   — la FORME du glyphe (plein, demi, anneau, pointillé, carré),
   *   — la couleur d'état,
   *   — le libellé texte.
   * Le glyphe seul suffit à balayer la colonne, y compris en niveaux de gris.
   */
</script>

<span class="mark {status}" title={$_(`status.${status}_help`)}>
  <svg width={size} height={size} viewBox="0 0 10 10" aria-hidden="true">
    {#if status === 'online'}
      <!-- Plein : présente. -->
      <circle cx="5" cy="5" r="4.2" fill="currentColor" />
    {:else if status === 'degraded'}
      <!-- Demi : elle bat, mais son accès internet ne suit pas. -->
      <circle cx="5" cy="5" r="3.9" fill="none" stroke="currentColor" stroke-width="1.5" />
      <path d="M5 1.1A3.9 3.9 0 0 1 5 8.9Z" fill="currentColor" />
    {:else if status === 'silent' || status === 'stale'}
      <!-- Anneau : on ne sait pas, on dit depuis quand. -->
      <circle cx="5" cy="5" r="3.7" fill="none" stroke="currentColor" stroke-width="1.7" />
    {:else if status === 'never'}
      <!-- Pointillé : enrôlée, jamais vue. -->
      <circle
        cx="5"
        cy="5"
        r="3.7"
        fill="none"
        stroke="currentColor"
        stroke-width="1.4"
        stroke-dasharray="2 1.6"
      />
    {:else}
      <!-- Carré : absente. Le seul état qui appelle une action. -->
      <rect x="1" y="1" width="8" height="8" rx="1" fill="currentColor" />
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
    gap: 8px;
    font-size: 0.8125rem;
    font-weight: 500;
    white-space: nowrap;
    color: var(--color-ink);
  }
  .mark svg {
    flex-shrink: 0;
  }
  .online svg {
    color: var(--st-ok);
  }
  .degraded svg,
  .silent svg,
  .stale svg {
    color: var(--st-warn);
  }
  .never svg {
    color: var(--st-none);
  }
  .offline svg {
    color: var(--st-bad);
  }
  .txt {
    color: var(--color-ink);
  }
  /* Hors ligne : le seul état qui garde son libellé en couleur pleine, parce
     que c'est le seul qui appelle une action. */
  .offline .txt {
    color: var(--st-bad);
  }
</style>
