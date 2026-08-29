<script lang="ts">
  import { _ } from 'svelte-i18n';
  import { api, ApiError, type CommandKind } from '$lib/api';

  interface Props {
    probeId: string;
    kind: CommandKind;
    args?: Record<string, unknown>;
    label: string;
    /** Masqué quand le rôle ne permet pas de lancer une commande. */
    allowed?: boolean;
    onsent?: () => void;
  }
  let { probeId, kind, args = {}, label, allowed = true, onsent }: Props = $props();

  let busy = $state(false);
  let queued = $state<number | null>(null);
  let error = $state('');

  async function run() {
    busy = true;
    error = '';
    try {
      const res = await api.runCommand(probeId, kind, args);
      queued = res.heartbeat_interval_secs;
      onsent?.();
    } catch (e) {
      error = e instanceof ApiError ? e.message : String(e);
    } finally {
      busy = false;
    }
  }
</script>

{#if allowed}
  <!--
    ⚠️ Le retour passe SOUS le bouton, jamais à côté : à côté, il poussait le
    bouton au moment même où on venait de cliquer dessus — et un bouton qui
    bouge sous le curseur invite au double-clic, donc à une seconde commande.

    Le texte reste court. Le détail (« le hub n'a pas de route vers la sonde,
    la commande part au prochain battement ») est vrai mais se lit une fois ;
    répété sous chaque bouton, il devient du bruit.
  -->
  <span class="wrap">
    <button class="lp-btn primary sm" onclick={run} disabled={busy}>
      {busy ? $_('commands.sending') : label}
    </button>
    {#if error}
      <span class="note err">{error}</span>
    {:else if queued != null}
      <span class="note" title={$_('commands.queued_help', { values: { n: queued } })}>
        {$_('commands.queued')}
      </span>
    {/if}
  </span>
{/if}

<style>
  .wrap {
    display: inline-flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 3px;
  }
  .note {
    font-size: 10.5px;
    color: var(--ep-text-dim);
    max-width: 220px;
  }
  .err {
    color: var(--ep-danger);
    overflow-wrap: anywhere;
  }
</style>
