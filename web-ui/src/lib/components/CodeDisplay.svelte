<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { _ } from 'svelte-i18n';
  import CopyLine from './CopyLine.svelte';

  interface Props {
    code: string;
    /** Epoch en secondes. */
    expiresAt: number;
    hubUrl: string;
    /** Phrase à dicter — propre à l'écran appelant. */
    dictate: string;
    onregenerate: () => void;
    busy?: boolean;
    regenerateLabel: string;
  }
  let { code, expiresAt, hubUrl, dictate, onregenerate, busy = false, regenerateLabel }: Props =
    $props();

  // Compte à rebours à la seconde : un code qui expire sous les yeux doit se
  // voir descendre, sinon l'échec arrive sans prévenir.
  let tick = $state(Date.now());
  let timer: ReturnType<typeof setInterval> | null = null;
  onMount(() => {
    timer = setInterval(() => (tick = Date.now()), 1000);
  });
  onDestroy(() => {
    if (timer) clearInterval(timer);
  });

  const remaining = $derived(expiresAt * 1000 - tick);
  const expired = $derived(remaining <= 0);
  const countdown = $derived.by(() => {
    const s = Math.max(0, Math.floor(remaining / 1000));
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
  });

  /**
   * Regroupement par quatre. Ce code sera dicté au téléphone : on le découpe en
   * paquets courts, on l'affiche très grand, et on demande le zéro barré à la
   * fonte pour que 0 et O ne se ressemblent pas à l'oreille comme à l'œil.
   */
  const groups = $derived.by(() => {
    const raw = code.replace(/[^A-Za-z0-9]/g, '').toUpperCase();
    return raw.match(/.{1,4}/g) ?? [raw];
  });
</script>

<div class="cd" class:expired>
  <div class="pair">
    <div class="box">
      <span class="lp-label">{$_('code.label_url')}</span>
      <span class="url">{hubUrl}</span>
      <CopyLine value={hubUrl} shell={false} hideValue />
    </div>

    <div class="box">
      <span class="lp-label">{$_('code.label_code')}</span>
      <span class="code" aria-label={groups.join(' ').split('').join(' ')}>
        {#each groups as g, i (i)}
          {#if i > 0}<span class="sep" aria-hidden="true">–</span>{/if}<span class="grp">{g}</span>
        {/each}
      </span>
      <CopyLine value={code} shell={false} hideValue />
    </div>
  </div>

  {#if expired}
    <div class="gone" role="alert">
      <strong>{$_('code.expired_title')}</strong>
      <p>{$_('code.expired_body')}</p>
      <button class="lp-btn primary" onclick={onregenerate} disabled={busy}>
        {busy ? $_('code.generating') : regenerateLabel}
      </button>
    </div>
  {:else}
    <div class="live">
      <span class="ttl lp-mono">{$_('code.expires_in', { values: { time: countdown } })}</span>
      <button class="lp-btn sm" onclick={onregenerate} disabled={busy}>{regenerateLabel}</button>
    </div>

    <!-- La phrase à lire telle quelle. C'est le seul contenu de l'application
         destiné à être transmis oralement : il est écrit pour être dit. -->
    <p class="dictate">{dictate}</p>
  {/if}
</div>

<style>
  .cd {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .pair {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(250px, 1fr));
    gap: 12px;
  }
  .box {
    display: flex;
    flex-direction: column;
    gap: 7px;
    padding: 14px;
    border: 1px solid var(--ep-border);
    border-radius: var(--ep-radius-md);
    background: var(--ep-bg-tertiary);
    min-width: 0;
  }
  .url {
    font-family: var(--ep-font-mono);
    font-size: 16px;
    font-weight: 700;
    color: var(--ep-text-primary);
    overflow-wrap: anywhere;
    line-height: 1.35;
  }
  .code {
    font-family: var(--ep-font-mono);
    font-size: 30px;
    font-weight: 700;
    line-height: 1.15;
    color: var(--ep-accent-bright);
    display: flex;
    align-items: baseline;
    gap: 6px;
    flex-wrap: wrap;
    /* Zéro barré et alternats désambiguïsants quand la fonte les propose. */
    font-variant-numeric: slashed-zero tabular-nums;
    font-feature-settings: 'zero' 1, 'ss01' 1;
  }
  .grp {
    letter-spacing: 4px;
  }
  .sep {
    color: var(--ep-text-dim);
    font-weight: 400;
  }
  .expired .code {
    color: var(--ep-text-dim);
    text-decoration: line-through;
  }

  .live {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }
  .ttl {
    font-size: 12px;
    font-weight: 700;
    color: var(--ep-warning);
    border: 1px solid color-mix(in srgb, var(--ep-warning) 45%, var(--ep-border));
    border-radius: 999px;
    padding: 3px 9px;
  }
  .live button {
    margin-left: auto;
  }

  .dictate {
    margin: 0;
    font-size: 13px;
    line-height: 1.6;
    color: var(--ep-text-primary);
    background: var(--ep-accent-dim);
    border: 1px solid var(--ep-accent-glow);
    border-radius: var(--ep-radius-md);
    padding: 10px 12px;
    max-width: 78ch;
  }

  .gone {
    border: 1px solid color-mix(in srgb, var(--ep-warning) 50%, var(--ep-border));
    border-left-width: 3px;
    border-radius: var(--ep-radius-md);
    background: color-mix(in srgb, var(--ep-warning) 8%, transparent);
    padding: 12px 14px;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 8px;
  }
  .gone strong {
    font-size: 12.5px;
  }
  .gone p {
    font-size: 11.5px;
    color: var(--ep-text-secondary);
    margin: 0;
    line-height: 1.6;
    max-width: 74ch;
  }
</style>
