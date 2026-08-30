<script lang="ts">
  import { _ } from 'svelte-i18n';
  import { api, ApiError, whoami, type Identity } from '$lib/api';
  import AuthFrame from './AuthFrame.svelte';

  const { onDone, notice = '' } = $props<{
    onDone: (who: Identity) => void;
    notice?: string;
  }>();

  let username = $state('');
  let password = $state('');
  let busy = $state(false);
  let error = $state('');

  /**
   * Le champ du second facteur n'apparaît qu'une fois que le hub l'a réclamé.
   *
   * ⚠️ Jamais affiché d'emblée : le montrer à tout le monde apprendrait quels
   * comptes en ont un et lesquels n'en ont pas, avant même d'avoir tapé un
   * mot de passe.
   */
  let totpNeeded = $state(false);
  let code = $state('');

  async function submit(e: Event) {
    e.preventDefault();
    error = '';
    busy = true;
    try {
      await api.login(username.trim(), password, totpNeeded ? code.trim() : undefined);
      // Le hub a authentifié et posé le cookie — encore faut-il que le
      // navigateur l'ait accepté. Un cookie `Secure` laissé par une visite
      // en HTTPS sur le même hôte empêche une page HTTP de le remplacer, et
      // le navigateur le jette *sans rien dire* : la connexion réussit puis
      // chaque requête repart en 401. Sans cette vérification, l'utilisateur
      // ne voit qu'un « session expirée » qui désigne la mauvaise cause.
      //
      // La même requête rend le rôle : on le transmet plutôt que de le
      // redemander, et l'application démarre en le connaissant déjà.
      const who = await whoami();
      if (!who) {
        error = $_('auth.cookie_blocked');
        password = '';
        return;
      }
      onDone(who);
    } catch (err) {
      if (err instanceof ApiError && err.isNetwork) error = $_('auth.unreachable');
      else if (err instanceof ApiError && err.needsTotp) {
        // Le mot de passe était bon : on le GARDE. Le vider obligerait à le
        // retaper pour envoyer un code, ce qui ressemblerait à un échec alors
        // que c'est la moitié normale d'une connexion à deux facteurs.
        totpNeeded = true;
        code = '';
        error = '';
      } else if (err instanceof ApiError && err.isUnauthorized) {
        // 401 sur un formulaire de connexion, c'est « mauvais identifiants » :
        // on ne renvoie pas le message brut du serveur, qui parlerait de
        // session. Sauf quand le code vient d'être refusé — là, c'est le code
        // qui est en cause, et renvoyer « identifiants incorrects » enverrait
        // chercher au mauvais endroit.
        error = totpNeeded ? $_('auth.totp_bad') : $_('auth.failed');
        if (totpNeeded) code = '';
        else password = '';
      } else {
        error = err instanceof ApiError ? err.message : String(err);
        password = '';
      }
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

    {#if totpNeeded}
      <!-- `inputmode` numérique : sur téléphone, le clavier arrive avec les
           chiffres. `one-time-code` laisse iOS et Android proposer le code
           depuis l'application d'authentification. -->
      <label class="lp-field">
        {$_('auth.totp_label')}
        <input
          class="lp-input code"
          type="text"
          bind:value={code}
          inputmode="numeric"
          autocomplete="one-time-code"
          maxlength="6"
          spellcheck="false"
          placeholder="000000"
        />
        <span class="hint">{$_('auth.totp_hint')}</span>
      </label>
    {/if}

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
  /* Le code se lit par groupes de chiffres : chasse fixe et lettres espacées,
     pour qu'on repère tout de suite une frappe de travers. */
  .code {
    font-family: var(--ep-mono, ui-monospace, monospace);
    letter-spacing: 0.35em;
    text-align: center;
    font-size: 1.1rem;
  }
  .hint {
    font-size: 11px;
    color: var(--ep-text-secondary);
  }
  .err {
    font-size: 12px;
    color: var(--ep-danger);
    margin: 0;
    border-left: 2px solid var(--ep-danger);
    padding-left: 8px;
  }
</style>
