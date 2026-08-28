<script lang="ts">
  import { _ } from 'svelte-i18n';
  import { api, ApiError, probeSession } from '$lib/api';
  import AuthFrame from './AuthFrame.svelte';

  const { onDone, notice = '' } = $props<{ onDone: () => void; notice?: string }>();

  let username = $state('');
  let password = $state('');
  let busy = $state(false);
  let error = $state('');

  async function submit(e: Event) {
    e.preventDefault();
    error = '';
    busy = true;
    try {
      await api.login(username.trim(), password);
      // Le hub a authentifié et posé le cookie — encore faut-il que le
      // navigateur l'ait accepté. Un cookie `Secure` laissé par une visite
      // en HTTPS sur le même hôte empêche une page HTTP de le remplacer, et
      // le navigateur le jette *sans rien dire* : la connexion réussit puis
      // chaque requête repart en 401. Sans cette vérification, l'utilisateur
      // ne voit qu'un « session expirée » qui désigne la mauvaise cause.
      if (!(await probeSession())) {
        error = $_('auth.cookie_blocked');
        password = '';
        return;
      }
      onDone();
    } catch (err) {
      if (err instanceof ApiError && err.isNetwork) error = $_('auth.unreachable');
      // 401 sur un formulaire de connexion, c'est « mauvais identifiants » :
      // on ne renvoie pas le message brut du serveur, qui parlerait de session.
      else if (err instanceof ApiError && err.isUnauthorized) error = $_('auth.failed');
      else error = err instanceof ApiError ? err.message : String(err);
      password = '';
    } finally {
      busy = false;
    }
  }
</script>

<AuthFrame title={$_('auth.title')} lead={$_('auth.lead')}>
  <form onsubmit={submit}>
    {#if notice}<p class="notice" role="status">{notice}</p>{/if}

    <label class="lp-field">
      {$_('auth.username')}
      <input class="lp-input" type="text" bind:value={username} autocomplete="username" />
    </label>

    <label class="lp-field">
      {$_('auth.password')}
      <input
        class="lp-input"
        type="password"
        bind:value={password}
        autocomplete="current-password"
      />
    </label>

    {#if error}<p class="err" role="alert">{error}</p>{/if}

    <button class="lp-btn primary" type="submit" disabled={busy}>
      {busy ? $_('auth.submitting') : $_('auth.submit')}
    </button>
  </form>
</AuthFrame>

<style>
  form {
    display: flex;
    flex-direction: column;
    gap: 13px;
  }
  .notice {
    font-size: 12px;
    color: var(--ep-warning);
    margin: 0;
    border-left: 2px solid var(--ep-warning);
    padding-left: 8px;
  }
  .err {
    font-size: 12px;
    color: var(--ep-danger);
    margin: 0;
    border-left: 2px solid var(--ep-danger);
    padding-left: 8px;
  }
</style>
