<script lang="ts">
  import { _ } from 'svelte-i18n';
  import type { AdvertiseTest } from '$lib/api';

  interface Props {
    result: AdvertiseTest | null;
    phase: 'idle' | 'testing' | 'done' | 'failed';
    /** Erreur de transport — distincte d'un `reachable: false`, qui est un verdict. */
    error?: string;
    ontest: () => void;
    /** Lien vers les réglages : affiché partout sauf sur l'écran des réglages. */
    showFix?: boolean;
  }
  let { result, phase, error = '', ontest, showFix = false }: Props = $props();

  /**
   * Rien n'a été réglé : le hub a *deviné* l'URL à partir de l'adresse par
   * laquelle on l'a joint. C'est une supposition, et elle doit se lire comme
   * telle — affichée comme les autres, elle passe pour une valeur confirmée,
   * alors que c'est exactement l'adresse que l'utilisateur vient de taper dans
   * sa barre d'URL, renvoyée en écho.
   */
  const deduced = $derived(result?.source === 'host_header');
</script>

<!--
  Trois informations, toujours ensemble : l'URL retenue, d'où elle sort, et le
  verdict. « Ça ne répond pas » sans savoir quelle valeur a servi n'aide
  personne — c'est l'origine qui dit s'il faut corriger le champ, la variable
  d'environnement, ou la publication du port.
-->
<div class="adv" class:bad={result && !result.reachable} class:good={result?.reachable}>
  <div class="facts">
    <div class="fact grow">
      <span class="lp-label">{$_('settings.tested_url')}</span>
      {#if result}
        <span class="lp-mono url" class:guess={deduced}>{result.url}</span>
      {:else}
        <span class="lp-mono url none">{$_('common.none')}</span>
      {/if}
    </div>
    <div class="fact">
      <span class="lp-label">{$_('settings.source')}</span>
      {#if result}
        <!-- Une pastille, pas une phrase : « réglée » et « déduite » doivent se
             distinguer d'un coup d'œil, sans être lues. -->
        <span class="origin" class:guess={deduced}>
          {$_(`settings.origin_${result.source}`)}
        </span>
      {:else}
        <span class="src">{$_('common.none')}</span>
      {/if}
    </div>
    <button class="lp-btn sm" onclick={ontest} disabled={phase === 'testing'}>
      {phase === 'testing' ? $_('settings.testing') : $_('settings.test')}
    </button>
  </div>

  {#if deduced}
    <p class="guess-note">{$_('settings.deduced_note')}</p>
  {/if}

  <div class="verdict">
    {#if phase === 'failed'}
      <p class="err" role="alert">{error || $_('settings.test_failed')}</p>
    {:else if phase === 'idle'}
      <p class="muted">{$_('settings.not_tested')}</p>
    {:else if result?.reachable}
      <p class="ok">
        <span class="glyph" aria-hidden="true">✓</span>
        {$_('settings.reachable')}
      </p>
    {:else if result}
      <!-- Le message dit quoi faire, pas seulement que ça a échoué. -->
      <div class="fail" role="alert">
        <strong>{$_('settings.unreachable_title')}</strong>
        <p>{$_('settings.unreachable_body')}</p>
        {#if showFix}
          <a class="lp-btn sm" href="#/settings">{$_('settings.fix_cta')}</a>
        {/if}
      </div>
    {/if}
  </div>
</div>

<style>
  .adv {
    border: 1px solid var(--ep-border);
    border-left-width: 3px;
    border-radius: var(--ep-radius-md);
    padding: 12px 14px;
    background: var(--ep-bg-secondary);
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .adv.good {
    border-left-color: var(--ep-success);
  }
  .adv.bad {
    border-left-color: var(--ep-danger);
    background: color-mix(in srgb, var(--ep-danger) 6%, var(--ep-bg-secondary));
  }

  .facts {
    display: flex;
    align-items: flex-end;
    gap: 18px;
    flex-wrap: wrap;
  }
  .fact {
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 0;
  }
  .grow {
    flex: 1 1 200px;
  }
  .url {
    font-size: 12px;
    color: var(--ep-text-primary);
    overflow-wrap: anywhere;
  }
  /* Une valeur devinée n'a pas le contraste d'une valeur confirmée, et porte le
     soulignement pointillé des termes qui demandent une vérification. */
  .url.guess {
    color: var(--ep-text-secondary);
    text-decoration: underline dotted color-mix(in srgb, var(--ep-warning) 70%, transparent);
    text-underline-offset: 3px;
  }
  .url.none {
    color: var(--ep-text-dim);
  }
  .src {
    font-size: 12px;
    color: var(--ep-text-secondary);
  }
  .origin {
    display: inline-flex;
    align-items: center;
    align-self: flex-start;
    font-size: 11px;
    font-weight: 600;
    border-radius: 999px;
    padding: 2px 8px;
    border: 1px solid var(--ep-border-strong);
    background: var(--ep-bg-tertiary);
    color: var(--ep-text-secondary);
    white-space: nowrap;
  }
  .origin.guess {
    color: var(--ep-warning);
    border-color: color-mix(in srgb, var(--ep-warning) 45%, var(--ep-border));
    background: color-mix(in srgb, var(--ep-warning) 12%, transparent);
  }
  .guess-note {
    margin: 0;
    font-size: 11.5px;
    line-height: 1.6;
    color: var(--ep-text-secondary);
    border-left: 2px solid color-mix(in srgb, var(--ep-warning) 55%, var(--ep-border));
    padding-left: 10px;
    max-width: 78ch;
  }
  .facts button {
    margin-left: auto;
  }

  .verdict p {
    margin: 0;
    font-size: 12px;
    line-height: 1.6;
  }
  .muted {
    color: var(--ep-text-muted);
  }
  .ok {
    color: var(--ep-success);
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .glyph {
    font-weight: 700;
  }
  .err {
    color: var(--ep-danger);
  }
  .fail strong {
    display: block;
    font-size: 12.5px;
    color: var(--ep-danger);
    margin-bottom: 4px;
  }
  .fail p {
    color: var(--ep-text-secondary);
    max-width: 78ch;
    margin: 0 0 8px;
  }
  .fail a {
    text-decoration: none;
  }
</style>
