<script lang="ts">
  import { _ } from 'svelte-i18n';
  import { api, ApiError } from '$lib/api';
  import AuthFrame from './AuthFrame.svelte';
  import CopyLine from '$lib/components/CopyLine.svelte';

  const { onDone } = $props<{ onDone: () => void }>();

  let token = $state('');
  let username = $state('');
  let password = $state('');
  let confirm = $state('');
  let busy = $state(false);
  let error = $state('');

  const DOCKER_LOGS = 'docker logs lanprobe-web 2>&1 | grep -i "setup token"';
  const DOCKER_FILE = 'docker exec lanprobe-web cat /data/setup-token';

  async function submit(e: Event) {
    e.preventDefault();
    error = '';
    // Les trois refus les plus fréquents sont dits AVANT l'aller-retour réseau,
    // pour ne pas consommer une tentative sur une faute de frappe.
    if (!token.trim()) return void (error = $_('setup.err_token'));
    if (!username.trim()) return void (error = $_('setup.err_username'));
    if (password.length < 12) return void (error = $_('setup.err_password'));
    if (password !== confirm) return void (error = $_('setup.err_mismatch'));

    busy = true;
    try {
      await api.setup(token.trim(), username.trim(), password);
      // Le contrat ne dit pas que /api/setup ouvre une session : on enchaîne
      // explicitement sur /api/login pour arriver dans le parc connecté.
      await api.login(username.trim(), password);
      onDone();
    } catch (err) {
      error =
        err instanceof ApiError
          ? err.isNetwork
            ? $_('auth.unreachable')
            : err.message
          : String(err);
    } finally {
      busy = false;
    }
  }
</script>

<AuthFrame title={$_('setup.title')} lead={$_('setup.lead')}>
  {#snippet aside()}
    <!--
      L'aide au jeton n'est pas une note de bas de page : c'est le moment exact
      où les gens abandonnent. Elle occupe donc une colonne entière, à côté du
      champ, avec la commande prête à copier — pas « consultez les logs ».
    -->
    <div class="help lp-card">
      <h2 class="lp-title">{$_('setup.token_label')}</h2>
      <p>{$_('setup.token_where')}</p>
      <CopyLine value={DOCKER_LOGS} />
      <p class="alt">{$_('setup.token_alt')}</p>
      <CopyLine value={DOCKER_FILE} />
      <p class="note">{$_('setup.token_hint_name')}</p>
    </div>
  {/snippet}

  <form onsubmit={submit}>
    <label class="lp-field">
      {$_('setup.token_label')}
      <input
        class="lp-input"
        type="text"
        bind:value={token}
        autocomplete="off"
        spellcheck="false"
        placeholder={$_('setup.token_placeholder')}
      />
    </label>

    <label class="lp-field">
      {$_('setup.username')}
      <input class="lp-input" type="text" bind:value={username} autocomplete="username" />
    </label>

    <label class="lp-field">
      {$_('setup.password')}
      <input class="lp-input" type="password" bind:value={password} autocomplete="new-password" />
      <span class="hint">{$_('setup.password_hint')}</span>
    </label>

    <label class="lp-field">
      {$_('setup.password_confirm')}
      <input class="lp-input" type="password" bind:value={confirm} autocomplete="new-password" />
    </label>

    {#if error}<p class="err" role="alert">{error}</p>{/if}

    <button class="lp-btn primary" type="submit" disabled={busy}>
      {busy ? $_('setup.submitting') : $_('setup.submit')}
    </button>
  </form>
</AuthFrame>

<style>
  form {
    display: flex;
    flex-direction: column;
    gap: 13px;
  }
  .hint {
    font-size: 11px;
    color: var(--ep-text-muted);
    line-height: 1.5;
  }
  .err {
    font-size: 12px;
    color: var(--ep-danger);
    margin: 0;
    border-left: 2px solid var(--ep-danger);
    padding-left: 8px;
  }

  .help {
    padding: 18px;
    display: flex;
    flex-direction: column;
    gap: 9px;
    font-size: 12.5px;
    color: var(--ep-text-secondary);
    line-height: 1.6;
  }
  .help p {
    margin: 0;
  }
  .alt {
    margin-top: 4px !important;
  }
  .note {
    font-size: 11.5px;
    color: var(--ep-text-muted);
  }
</style>
