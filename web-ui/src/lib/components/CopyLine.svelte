<script lang="ts">
  import { _ } from 'svelte-i18n';

  interface Props {
    value: string;
    /** Style terminal : commande à coller telle quelle. */
    shell?: boolean;
    /** La valeur est déjà affichée à côté : on ne montre que le bouton. */
    hideValue?: boolean;
    /** Bouton au gabarit d'une ligne de tableau (22 px) plutôt que 28 px. */
    compact?: boolean;
    /**
     * Élément à sélectionner si le presse-papiers est refusé, quand la valeur
     * est rendue par l'appelant (`hideValue`). Sans lui, le repli n'a rien à
     * sélectionner et le clic n'a aucun effet visible — cas courant, le hub
     * étant souvent servi en HTTP où `navigator.clipboard` n'existe pas.
     */
    selectTarget?: HTMLElement | null;
  }
  let {
    value,
    shell = true,
    hideValue = false,
    compact = false,
    selectTarget = null,
  }: Props = $props();

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
      const node = selectTarget ?? document.getElementById(`cl-${id}`);
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

<div class="line" class:compact>
  {#if !hideValue}<code id="cl-{id}" class:shell>{value}</code>{/if}
  <button
    class="lp-btn sm"
    onclick={copy}
    title={$_('common.copy')}
    aria-label={$_('common.copy')}
  >
    {#if compact}
      <!-- Icône seule : le mot « Copier » ferait grossir la ligne du tableau. -->
      <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        {#if copied}
          <polyline points="3,8.5 6.5,12 13,4" />
        {:else}
          <rect x="5.5" y="5.5" width="8" height="8" rx="1.5" />
          <path d="M10.5 3.5H3.5a1 1 0 0 0-1 1v7" />
        {/if}
      </svg>
    {:else}
      {copied ? $_('common.copied') : $_('common.copy')}
    {/if}
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

  /* Gabarit « ligne de tableau » : le bouton ne doit pas dicter la hauteur. */
  .compact button {
    min-height: 22px;
    width: 22px;
    padding: 0;
    align-self: center;
    border-color: transparent;
    background: transparent;
    color: var(--ep-text-muted);
  }
  .compact button:hover:not(:disabled) {
    border-color: var(--ep-border-strong);
    background: var(--ep-bg-tertiary);
  }
</style>
