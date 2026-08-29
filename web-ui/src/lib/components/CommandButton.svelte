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
  <div class="wrap">
    <button class="lp-btn primary sm" onclick={run} disabled={busy}>
      {busy ? $_('commands.sending') : label}
    </button>
    {#if error}
      <span class="err">{error}</span>
    {:else if queued != null}
      <!--
        ⚠️ Le dire explicitement. Le hub n'a aucune route vers la sonde : la
        commande part dans la réponse au prochain battement de cœur. Sans ce
        message, on clique, rien ne bouge, et on reclique — en empilant des
        commandes qui s'exécuteront toutes.
      -->
      <span class="queued">{$_('commands.queued', { values: { n: queued } })}</span>
    {/if}
  </div>
{/if}

<style>
  .wrap {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .queued {
    font-size: 11px;
    color: var(--ep-text-dim);
  }
  .err {
    font-size: 11px;
    color: var(--ep-danger);
  }
</style>
