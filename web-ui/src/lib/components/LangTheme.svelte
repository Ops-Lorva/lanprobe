<script lang="ts">
  import { _, locale } from 'svelte-i18n';
  import { setLang, LANGS, type Lang } from '$lib/i18n';
  import { theme, type Theme } from '$lib/theme';

  interface Props {
    /** Version horizontale pour les barres ; sinon empilée. */
    compact?: boolean;
    /**
     * Libellés visibles, pour l'écran des réglages : deux sélecteurs nus dans
     * un panneau de formulaire ne disent pas ce qu'ils règlent. En barre, la
     * valeur affichée (« FR », « Sombre ») suffit et le libellé reste lu par
     * les lecteurs d'écran.
     */
    labelled?: boolean;
  }
  let { compact = false, labelled = false }: Props = $props();

  const THEMES: Theme[] = ['system', 'dark', 'light'];
</script>

<div class="lt" class:compact class:labelled>
  <label class="ctl">
    {#if labelled}
      <span class="cap">{$_('common.language')}</span>
    {:else}
      <span class="lp-sr">{$_('common.language')}</span>
    {/if}
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
    {#if labelled}
      <span class="cap">{$_('common.theme')}</span>
    {:else}
      <span class="lp-sr">{$_('common.theme')}</span>
    {/if}
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
  .lt:not(.compact):not(.labelled) {
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

  /* Dans les réglages : deux champs côte à côte, au gabarit des autres champs
     de l'écran, qui repassent l'un sous l'autre quand la place manque. */
  .lt.labelled {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: 12px;
  }
  .labelled .ctl {
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .cap {
    font-size: 12px;
    color: var(--ep-text-secondary);
  }
  .labelled select {
    font-size: 13px;
    padding: 8px 10px;
    min-height: 36px;
  }
</style>
