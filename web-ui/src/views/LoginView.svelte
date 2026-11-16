<script lang="ts">
  import { _ } from 'svelte-i18n';
  import { api, ApiError, whoami, type Identity } from '$lib/api';
  import { passkeysSupported, getAssertion, ceremonyError } from '$lib/webauthn';
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

  /**
   * La connexion par clé d'accès.
   *
   * ⚠️ Le bouton n'apparaît que si le navigateur peut réellement l'utiliser :
   * proposer une clé sur un hub en HTTP donnerait un clic qui échoue sans
   * message, et enverrait chercher la panne du mauvais côté.
   */
  const canPasskey = passkeysSupported();
  let pkBusy = $state(false);

  async function signInWithPasskey() {
    const who = username.trim();
    if (!who) {
      error = $_('auth.passkey_need_username');
      return;
    }
    pkBusy = true;
    error = '';
    try {
      const challenge = await api.passkeyLoginStart(who);
      const assertion = await getAssertion(challenge as never);
      await api.passkeyLoginFinish(who, assertion);
      const identity = await whoami();
      if (!identity) {
        error = $_('auth.cookie_blocked');
        return;
      }
      onDone(identity);
    } catch (err) {
      if (err instanceof ApiError && err.isNetwork) error = $_('auth.unreachable');
      // 404 : ce compte n'a aucune clé sur ce hub. C'est une information que
      // l'utilisateur a demandée en cliquant, pas une fuite : il faut un nom
      // de compte pour l'obtenir, exactement comme pour un mot de passe.
      else if (err instanceof ApiError && err.status === 404) error = $_('auth.passkey_none');
      else if (err instanceof ApiError) error = err.message;
      else error = ceremonyError(err, (k) => $_(k));
    } finally {
      pkBusy = false;
    }
  }

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

<AuthFrame title={$_('auth.title')}>
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
        aria-invalid={!!error && !totpNeeded}
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

    <button class="lp-btn primary wide" type="submit" disabled={busy}>
      {busy ? $_('auth.submitting') : $_('auth.submit')}
    </button>

    {#if canPasskey && !totpNeeded}
      <!-- Sous le mot de passe et non au-dessus : c'est le chemin que
           connaissent ceux qui en ont une, pas celui qu'on découvre. Une clé
           d'accès remplace le mot de passe ET le second facteur — elle est
           déjà liée à l'appareil et déverrouillée par lui. -->
      <div class="or" aria-hidden="true"><span>{$_('auth.or')}</span></div>
      <button class="lp-btn wide" type="button" onclick={signInWithPasskey} disabled={pkBusy}>
        <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" aria-hidden="true"><circle cx="5" cy="11" r="2.5" /><path d="M6.8 9.2 13 3M11 5l1.5 1.5M9.5 6.5 11 8" /></svg>
        {pkBusy ? $_('auth.passkey_waiting') : $_('auth.passkey_submit')}
      </button>
    {/if}
  </form>
</AuthFrame>

<style>
  form {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .notice {
    font-size: 0.8125rem;
    color: var(--st-warn);
    margin: 0;
  }
  /* Le code se lit par groupes de chiffres : chasse fixe et lettres espacées,
     pour qu'on repère tout de suite une frappe de travers. */
  .code {
    font-family: var(--ep-font-mono);
    letter-spacing: 0.35em;
    text-align: center;
    font-size: 1.1rem;
  }
  .hint {
    font-size: 0.8125rem;
    color: var(--color-muted);
  }
  .wide {
    width: 100%;
    height: 40px;
    justify-content: center;
  }
  /* Séparateur « ou » : un filet coupé par le mot, pour que les deux chemins
     se lisent comme des alternatives et non comme une suite d'étapes. */
  .or {
    display: flex;
    align-items: center;
    gap: 12px;
    font-size: 0.8125rem;
    color: var(--color-muted);
  }
  .or::before,
  .or::after {
    content: '';
    flex: 1;
    height: 1px;
    background: var(--color-rule);
  }
  .err {
    font-size: 0.8125rem;
    color: var(--st-bad);
    margin: -8px 0 0;
  }
  :global(.lp-input[aria-invalid='true']) {
    border-color: var(--st-bad);
  }
</style>
