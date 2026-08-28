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
</script>

<!--
  Trois informations, toujours ensemble : l'URL testée, le verdict, et l'origine
  de la valeur. « Ça ne répond pas » sans savoir quelle valeur a servi n'aide
  personne — c'est l'origine qui dit s'il faut corriger le champ, la variable
  d'environnement, ou la publication du port.
-->
<div class="adv" class:bad={result && !result.reachable} class:good={result?.reachable}>
  <div class="facts">
    <div class="fact">
      <span class="lp-label">{$_('settings.tested_url')}</span>
      <span class="lp-mono url">{result?.url ?? '—'}</span>
    </div>
    <div class="fact">
      <span class="lp-label">{$_('settings.source')}</span>
      <span class="src">
        {result ? $_(`settings.source_${result.source}`) : '—'}
      </span>
    </div>
    <button class="lp-btn sm" onclick={ontest} disabled={phase === 'testing'}>
      {phase === 'testing' ? $_('settings.testing') : $_('settings.test')}
    </button>
  </div>

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
  .url {
    font-size: 12px;
    color: var(--ep-text-primary);
    overflow-wrap: anywhere;
  }
  .src {
    font-size: 12px;
    color: var(--ep-text-secondary);
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
