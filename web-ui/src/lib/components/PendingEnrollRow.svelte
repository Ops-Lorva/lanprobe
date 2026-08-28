<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { _ } from 'svelte-i18n';
  import CodeDisplay from './CodeDisplay.svelte';
  import type { PendingRow } from '$lib/enroll';

  interface Props {
    row: PendingRow;
    busy?: boolean;
    error?: string;
    onregenerate: () => void;
    onhide: () => void;
  }
  let { row, busy = false, error = '', onregenerate, onhide }: Props = $props();

  const hubUrl = `${location.protocol}//${location.host}`;

  /** Ré-enrôlement = réparation d'une sonde existante. Ce n'est pas le même
      geste qu'ajouter une machine, et ça ne doit pas s'y confondre. */
  const repair = $derived(row.probe_id != null);

  // Le compte à rebours de l'état « code perdu » : CodeDisplay porte le sien
  // quand le code est là, mais cet état-ci n'a pas de CodeDisplay à afficher.
  let tick = $state(Date.now());
  let timer: ReturnType<typeof setInterval> | null = null;
  onMount(() => {
    timer = setInterval(() => (tick = Date.now()), 1000);
  });
  onDestroy(() => {
    if (timer) clearInterval(timer);
  });

  const remaining = $derived(row.expires_at * 1000 - tick);
  const expired = $derived(remaining <= 0);
  const countdown = $derived.by(() => {
    const s = Math.max(0, Math.floor(remaining / 1000));
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
  });
</script>

<!--
  Un emplacement réservé, pas un équipement. Le cadre en pointillés et le fond
  teinté disent « rien n'est encore là » sans qu'il faille lire l'étiquette :
  dans une liste de sondes, une ligne en attente qui ressemblerait à une vraie
  ligne se compterait comme une sonde installée.
-->
<div class="pending" class:repair class:expired>
  <div class="top">
    <span class="badge">
      <span class="pip" aria-hidden="true"></span>
      {repair ? $_('pending.badge_reenroll') : $_('pending.badge_new')}
    </span>
    <span class="ctx">
      {repair
        ? $_('pending.ctx_reenroll', { values: { name: row.probe ?? '' } })
        : $_('pending.ctx_new', { values: { site: row.site } })}
    </span>
    <!-- « Masquer » et non « Annuler » : le hub garde le code valable, cette
         action ne fait que retirer la ligne de l'écran. -->
    <button class="lp-btn ghost sm hide" onclick={onhide} title={$_('pending.hide_hint')}>
      {$_('pending.hide')}
    </button>
  </div>

  {#if row.code}
    <CodeDisplay
      code={row.code}
      expiresAt={row.expires_at}
      {hubUrl}
      dictate={repair ? $_('code.dictate_reenroll') : $_('code.dictate_enroll')}
      onregenerate={onregenerate}
      {busy}
      regenerateLabel={$_('code.regenerate')}
    />
  {:else}
    <!-- Le hub sert le code tant qu'il vaut quelque chose. Son absence dit donc
         qu'il ne vaut plus rien — consommé, ou expiré. Une ligne muette laisserait
         croire à une panne d'affichage. -->
    <div class="lost">
      <strong>{$_('pending.lost_title')}</strong>
      <p>{$_('pending.lost_body')}</p>
      <div class="lost-acts">
        <span class="ttl lp-mono" class:dead={expired}>
          {expired
            ? $_('pending.expired')
            : $_('code.expires_in', { values: { time: countdown } })}
        </span>
        <button class="lp-btn primary sm" onclick={onregenerate} disabled={busy}>
          {busy ? $_('code.generating') : $_('code.regenerate')}
        </button>
      </div>
    </div>
  {/if}

  {#if error}<p class="err" role="alert">{error}</p>{/if}
</div>

<style>
  .pending {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 12px 14px;
    margin: 10px 12px;
    border: 1px dashed var(--ep-accent-glow);
    border-radius: var(--ep-radius-md);
    background: color-mix(in srgb, var(--ep-accent) 6%, transparent);
  }
  /* Réparer une sonde compromise n'est pas ajouter une machine : la teinte
     change avec l'opération, pas seulement le libellé. */
  .pending.repair {
    border-color: color-mix(in srgb, var(--ep-warning) 55%, var(--ep-border));
    background: color-mix(in srgb, var(--ep-warning) 7%, transparent);
  }
  .pending.expired {
    border-color: var(--ep-border-strong);
    background: var(--ep-bg-tertiary);
  }

  .top {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }
  .badge {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 10px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.7px;
    color: var(--ep-accent-bright);
    border: 1px solid var(--ep-accent-glow);
    background: var(--ep-accent-dim);
    border-radius: 999px;
    padding: 3px 9px;
  }
  .repair .badge {
    color: var(--ep-warning);
    border-color: color-mix(in srgb, var(--ep-warning) 45%, var(--ep-border));
    background: color-mix(in srgb, var(--ep-warning) 12%, transparent);
  }
  .pip {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: currentColor;
    flex-shrink: 0;
  }
  .ctx {
    font-size: 12px;
    color: var(--ep-text-secondary);
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .hide {
    margin-left: auto;
    flex-shrink: 0;
  }

  .lost {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .lost strong {
    font-size: 12.5px;
    color: var(--ep-text-primary);
  }
  .lost p {
    margin: 0;
    font-size: 11.5px;
    line-height: 1.6;
    color: var(--ep-text-secondary);
    max-width: 78ch;
  }
  .lost-acts {
    display: flex;
    align-items: center;
    gap: 10px;
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
  .ttl.dead {
    color: var(--ep-text-muted);
    border-color: var(--ep-border-strong);
  }

  .err {
    margin: 0;
    font-size: 11.5px;
    color: var(--ep-danger);
    border-left: 2px solid var(--ep-danger);
    padding-left: 8px;
    overflow-wrap: anywhere;
  }
</style>
