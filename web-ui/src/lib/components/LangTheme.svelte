<script lang="ts">
  import { _, locale } from 'svelte-i18n';
  import { setLang, LANGS, type Lang } from '$lib/i18n';
  import { theme, type Theme } from '$lib/theme';

  interface Props {
    /** Version horizontale pour les barres ; sinon empilée pour la sidebar. */
    compact?: boolean;
  }
  let { compact = false }: Props = $props();

  const THEMES: Theme[] = ['system', 'dark', 'light'];
</script>

<div class="lt" class:compact>
  <label class="ctl">
    <span class="lp-sr">{$_('common.language')}</span>
    <select
      value={($locale as Lang) ?? 'en'}
      onchange={(e) => setLang(e.currentTarget.value as Lang)}
      aria-label={$_('common.language')}
    >
      {#each LANGS as l (l)}
        <option value={l}>{l.toUpperCase()}</option>
      {/each}
    </select>
  </label>

  <label class="ctl">
    <span class="lp-sr">{$_('common.theme')}</span>
    <select
      value={$theme}
      onchange={(e) => theme.set(e.currentTarget.value as Theme)}
      aria-label={$_('common.theme')}
    >
      {#each THEMES as t (t)}
        <option value={t}>{$_(`common.theme_${t}`)}</option>
      {/each}
    </select>
  </label>
</div>

<style>
  .lt {
    display: flex;
    gap: 6px;
  }
  .lt:not(.compact) {
    display: grid;
    grid-template-columns: 58px 1fr;
  }
  select {
    font-size: 11px;
    padding: 4px 7px;
    width: 100%;
    /* En barre horizontale, ces sélecteurs sont les derniers de la ligne : sans
       cette garde, le flex les écrase jusqu'à masquer leur libellé. */
    flex-shrink: 0;
    min-width: 58px;
  }
  .compact select {
    width: auto;
  }
  .ctl {
    min-width: 0;
  }
</style>
