<script lang="ts">
  import { _ } from 'svelte-i18n';

  interface Props {
    value: string;
    /** Style terminal : commande à coller telle quelle. */
    shell?: boolean;
    /** La valeur est déjà affichée à côté : on ne montre que le bouton. */
    hideValue?: boolean;
  }
  let { value, shell = true, hideValue = false }: Props = $props();

  const id = Math.random().toString(36).slice(2, 8);
  let copied = $state(false);
  let timer: ReturnType<typeof setTimeout> | null = null;

  async function copy() {
    try {
      await navigator.clipboard.writeText(value);
    } catch {
      // Clipboard refusé (HTTP non sécurisé, permission) : on sélectionne le
      // texte pour que l'utilisateur copie à la main plutôt que d'échouer
      // silencieusement.
      const node = document.getElementById(`cl-${id}`);
      if (node) {
        const r = document.createRange();
        r.selectNodeContents(node);
        getSelection()?.removeAllRanges();
        getSelection()?.addRange(r);
      }
      return;
    }
    copied = true;
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => (copied = false), 1800);
  }
</script>

<div class="line">
  {#if !hideValue}<code id="cl-{id}" class:shell>{value}</code>{/if}
  <button class="lp-btn sm" onclick={copy} aria-label={$_('common.copy')}>
    {copied ? $_('common.copied') : $_('common.copy')}
  </button>
</div>

<style>
  .line {
    display: flex;
    align-items: stretch;
    gap: 6px;
  }
  code {
    flex: 1;
    min-width: 0;
    font-family: var(--ep-font-mono);
    font-size: 11.5px;
    line-height: 1.5;
    background: var(--ep-bg-primary);
    border: 1px solid var(--ep-border);
    border-radius: var(--ep-radius-md);
    padding: 8px 10px;
    color: var(--ep-text-primary);
    /* Une commande longue s'enroule au lieu de pousser une barre horizontale
       à travers toute la page. */
    overflow-wrap: anywhere;
    white-space: pre-wrap;
    user-select: all;
  }
  code.shell::before {
    content: '$ ';
    color: var(--ep-text-dim);
  }
  button {
    flex-shrink: 0;
    align-self: flex-start;
  }
</style>
