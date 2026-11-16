<script lang="ts">
  import type { Snippet } from 'svelte';
  import LogoMark from '$desktop/components/LogoMark.svelte';
  import { _ } from 'svelte-i18n';
  import LangTheme from '$lib/components/LangTheme.svelte';

  interface Props {
    title: string;
    lead?: string;
    /** Écran large : la colonne d'aide occupe la moitié droite. */
    aside?: Snippet;
    children: Snippet;
  }
  let { title, lead, aside, children }: Props = $props();
</script>

<!--
  Cadre commun aux écrans hors session. Il reprend la marque de l'app desktop
  (LogoMark importé, pas recopié) pour que la première page du hub soit
  reconnaissable par quelqu'un qui vient de l'application.
-->
<div class="page">
  <main class="grid" class:with-aside={!!aside}>
    <section class="panel">
      <div class="brand">
        <LogoMark size={32} />
        <span class="wordmark">{$_('common.app_name')}</span>
        <span class="host">{typeof location === 'undefined' ? '' : location.host}</span>
      </div>
      <h1>{title}</h1>
      {#if lead}<p class="lead">{lead}</p>{/if}
      {@render children()}
      <div class="foot"><LangTheme compact /></div>
    </section>

    {#if aside}
      <aside class="aside">{@render aside()}</aside>
    {/if}
  </main>
</div>

<style>
  .page {
    min-height: 100dvh;
    display: flex;
    flex-direction: column;
    background: var(--color-paper);
    padding: 24px;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 16px;
  }
  .wordmark {
    font-size: 1.125rem;
    font-weight: 600;
    letter-spacing: -0.01em;
  }
  .host {
    font-size: 0.8125rem;
    color: var(--color-muted);
    margin-top: 2px;
  }

  .grid {
    flex: 1;
    display: grid;
    justify-content: center;
    align-content: center;
    gap: 48px;
    width: 100%;
    max-width: 1040px;
    margin: 0 auto;
    padding-bottom: 32px;
  }
  .grid.with-aside {
    grid-template-columns: minmax(0, 380px) minmax(0, 460px);
    align-items: start;
  }
  .panel {
    display: flex;
    flex-direction: column;
    gap: 16px;
    width: min(380px, 100%);
  }
  .grid.with-aside .panel {
    width: auto;
  }
  h1 {
    font-size: 1.375rem;
    font-weight: 600;
    letter-spacing: -0.02em;
    margin: 0;
  }
  .lead {
    font-size: 0.875rem;
    line-height: 1.5;
    color: var(--color-ink-2);
    margin: -8px 0 0;
  }
  .foot {
    display: flex;
    justify-content: flex-end;
    margin-top: 8px;
  }
  .aside {
    align-self: start;
  }

  @media (max-width: 900px) {
    .grid.with-aside {
      grid-template-columns: minmax(0, 1fr);
      max-width: 520px;
    }
  }
</style>
