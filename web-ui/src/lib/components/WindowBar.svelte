<script lang="ts">
  import { untrack } from 'svelte';
  import { _, locale } from 'svelte-i18n';
  import { RANGES } from '$lib/metrics';
  import { dateOnly } from '$lib/time';
  import {
    MAX_RANGE_DAYS,
    parseLocalDate,
    validateAbsoluteWindow,
    type WindowChoice,
    type WindowIssue,
  } from '$lib/time-window';

  interface Props {
    choice: WindowChoice;
    onpick: (choice: WindowChoice) => void;
    /** Actions propres à l'onglet (l'export SLA du tableau de bord). */
    actions?: import('svelte').Snippet;
  }
  let { choice, onpick, actions }: Props = $props();

  const lang = $derived($locale ?? 'en');

  // ── Plage de dates ────────────────────────────────────────────────────────
  //
  // ⚠️ Repliée par défaut. Le cas courant reste la fenêtre glissante ; deux
  // champs de date toujours ouverts au-dessus de chaque onglet occuperaient une
  // ligne entière pour un usage occasionnel.
  //
  // ⚠️ `untrack` sur la valeur initiale, et c'est voulu : le panneau ne doit
  // se rouvrir QUE si la fenêtre était déjà une plage à l'arrivée. Sans lui,
  // le refermer pendant qu'une plage est active le rouvrirait aussitôt, à
  // chaque recalcul — un panneau qu'on ne peut pas fermer.
  let customOpen = $state(untrack(() => choice.kind === 'absolute'));
  /** Champs `YYYY-MM-DD`, préremplis si une plage est déjà appliquée. */
  let from = $state(untrack(() => (choice.kind === 'absolute' ? isoDay(choice.startMs) : '')));
  let to = $state(untrack(() => (choice.kind === 'absolute' ? isoDay(choice.stopMs) : '')));

  /** `YYYY-MM-DD` en heure LOCALE — `toISOString()` décalerait d'un fuseau. */
  function isoDay(ms: number): string {
    const d = new Date(ms);
    const pad = (n: number) => String(n).padStart(2, '0');
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
  }

  const startMs = $derived(parseLocalDate(from, 'start'));
  const stopMs = $derived(parseLocalDate(to, 'stop'));

  /**
   * Verdict sur la saisie en cours.
   *
   * ⚠️ Tant que l'un des deux champs est vide, on ne dit RIEN : afficher
   * « début après la fin » sur un formulaire à moitié rempli fait corriger une
   * erreur qui n'existe pas encore.
   */
  const issue = $derived<WindowIssue | null>(
    from === '' || to === '' ? null : validateAbsoluteWindow(startMs, stopMs, Date.now()),
  );
  const ready = $derived(from !== '' && to !== '' && issue === null);

  const ISSUE_KEY: Record<WindowIssue, string> = {
    incomplete: 'probe.window_incomplete',
    reversed: 'probe.window_reversed',
    future: 'probe.window_future',
    too_wide: 'probe.window_too_wide',
  };

  function applyCustom() {
    if (!ready) return;
    onpick({ kind: 'absolute', startMs, stopMs });
  }

  /** Libellé d'une fenêtre glissante. Les cinq proposées ont le leur. */
  function rangeLabel(range: string): string {
    const key = `probe.range_${range.replace('-', '')}`;
    const label = $_(key);
    // ⚠️ `svelte-i18n` rend la CLÉ quand elle manque. Sans ce repli, une
    // fenêtre imposée par le hub (`realtime_window_min` réglé à 7) afficherait
    // « probe.range_7m » sur le bouton actif.
    if (label !== key) return label;
    const m = /^-(\d+)m$/.exec(range);
    return m ? $_('probe.range_minutes', { values: { n: Number(m[1]) } }) : range;
  }

  const activeRange = $derived(choice.kind === 'relative' ? choice.range : null);
  // Fenêtre glissante imposée par le hub et absente de la barre : elle a son
  // propre bouton, sinon l'état actif n'est visible nulle part.
  const extraRange = $derived(
    activeRange && !RANGES.includes(activeRange as (typeof RANGES)[number]) ? activeRange : null,
  );

  const customSummary = $derived(
    choice.kind === 'absolute'
      ? `${dateOnly(Math.floor(choice.startMs / 1000), lang)} → ${dateOnly(
          Math.floor(choice.stopMs / 1000),
          lang,
        )}`
      : $_('sla.custom'),
  );
</script>

<!--
  La barre est rendue sur TOUS les onglets qui montrent des données datées.
  Avant, elle n'existait que sur le tableau de bord : Surveillance, Débit,
  Ports et Découverte affichaient la fenêtre choisie ailleurs sans le dire, et
  « aucune cible surveillée » se lisait comme un fait alors que c'était une
  conséquence de la fenêtre.
-->
<div class="rangebar">
  <span class="lp-label">{$_('probe.range')}</span>
  <div class="chips" role="group" aria-label={$_('probe.range')}>
    {#each RANGES as r (r)}
      <button
        class="chip"
        class:on={activeRange === r}
        aria-pressed={activeRange === r}
        onclick={() => onpick({ kind: 'relative', range: r })}
      >
        {rangeLabel(r)}
      </button>
    {/each}
    {#if extraRange}
      <button class="chip on" aria-pressed="true">{rangeLabel(extraRange)}</button>
    {/if}
    <button
      class="chip custom"
      class:on={choice.kind === 'absolute'}
      aria-pressed={choice.kind === 'absolute'}
      aria-expanded={customOpen}
      onclick={() => (customOpen = !customOpen)}
    >
      {customSummary}
    </button>
  </div>

  {#if actions}
    <div class="acts">{@render actions()}</div>
  {/if}
</div>

{#if customOpen}
  <!--
    ⚠️ 90 jours au maximum, et le plafond est écrit AVANT la faute. À un ping
    par seconde, une plage plus large met InfluxDB à genoux — et c'est
    l'interface qui aura l'air en panne, pas la base.
  -->
  <div class="custom-row">
    <label class="lp-field">
      {$_('sla.from')}
      <input class="lp-input" type="date" bind:value={from} max={to || undefined} />
    </label>
    <label class="lp-field">
      {$_('sla.to')}
      <input class="lp-input" type="date" bind:value={to} min={from || undefined} />
    </label>
    <button class="lp-btn primary sm" onclick={applyCustom} disabled={!ready}>
      {$_('probe.window_apply')}
    </button>
    {#if issue}
      <!-- Un refus qui dit POURQUOI. Une requête vide se lirait comme une
           panne du dispositif, pas comme une saisie à corriger. -->
      <p class="werr" role="alert">
        {$_(ISSUE_KEY[issue], { values: { n: MAX_RANGE_DAYS } })}
      </p>
    {:else}
      <p class="whint">{$_('probe.window_max_hint', { values: { n: MAX_RANGE_DAYS } })}</p>
    {/if}
  </div>
{/if}

<style>
  .rangebar {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
    margin-bottom: 10px;
  }
  .chips {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }
  .chip {
    padding: 5px 10px;
    border-radius: 999px;
    border: 1px solid var(--ep-border);
    background: var(--ep-bg-secondary);
    color: var(--ep-text-secondary);
    font-family: var(--ep-font-sans);
    font-size: 11.5px;
    cursor: pointer;
  }
  .chip:hover {
    border-color: var(--ep-border-strong);
    color: var(--ep-text-primary);
  }
  .chip.on {
    border-color: var(--ep-accent);
    background: var(--ep-accent-dim);
    color: var(--ep-accent-bright);
    font-weight: 600;
  }
  .acts {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .custom-row {
    display: flex;
    align-items: flex-end;
    gap: 10px;
    flex-wrap: wrap;
    padding: 10px 12px;
    margin-bottom: 10px;
    border: 1px solid var(--ep-border);
    border-radius: var(--ep-radius-md);
    background: var(--ep-bg-secondary);
  }
  .werr {
    margin: 0;
    font-size: 11.5px;
    color: var(--ep-danger);
    flex: 1 1 220px;
  }
  .whint {
    margin: 0;
    font-size: 11.5px;
    color: var(--ep-text-muted);
    flex: 1 1 220px;
  }
</style>
