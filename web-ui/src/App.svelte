<script lang="ts">
  import { onMount } from 'svelte';
  import { isLoading, _ } from 'svelte-i18n';
  import { api, ApiError, whoami, type Identity } from '$lib/api';
  import { forgetIdentity, setIdentity } from '$lib/session';
  import SetupView from './views/SetupView.svelte';
  import LoginView from './views/LoginView.svelte';
  import Shell from './views/Shell.svelte';
  import StateBlock from '$lib/components/StateBlock.svelte';

  type Phase = 'boot' | 'setup' | 'login' | 'app' | 'unreachable';

  let phase = $state<Phase>('boot');
  let version = $state('');
  let notice = $state('');

  async function bootstrap() {
    phase = 'boot';
    notice = '';
    try {
      const s = await api.status();
      version = s.version ?? '';
      if (s.needs_setup) {
        phase = 'setup';
        return;
      }
      // `/api/status` est publique et ne dit pas si la session tient. `/api/me`
      // répond aux deux questions d'un coup : la session, et le rôle. Le rôle
      // est donc connu AVANT le premier écran — l'interface n'a plus à
      // l'apprendre en se faisant refuser une route, ce qui déposait au journal
      // d'audit un `access.denied` par ouverture de session d'observateur.
      const who = await whoami();
      if (!who) {
        phase = 'login';
        return;
      }
      setIdentity(who);
      phase = 'app';
    } catch (e) {
      if (e instanceof ApiError && e.isExpired) phase = 'login';
      else phase = 'unreachable';
    }
  }

  onMount(bootstrap);

  /**
   * Entrée dans l'application. La connexion connaît déjà l'identité — elle
   * vient de la lire pour vérifier que le cookie a été accepté — et l'apporte
   * plutôt que de faire redemander la même chose. L'écran d'installation, lui,
   * n'a rien à apporter : on repasse par le démarrage complet.
   */
  async function enter(who?: Identity) {
    if (who) {
      setIdentity(who);
      notice = '';
      phase = 'app';
      return;
    }
    await bootstrap();
  }

  /** Appelé par n'importe quelle vue qui se prend un 401 en cours de route. */
  function onExpired() {
    notice = $_('auth.expired');
    phase = 'login';
  }

  async function logout() {
    try {
      await api.logout();
    } catch {
      // Même si le hub refuse, on quitte l'écran : garder l'utilisateur devant
      // un parc auquel il n'a plus accès serait pire.
    }
    // Le compte suivant n'a aucune raison d'hériter de l'identité du précédent.
    forgetIdentity();
    notice = '';
    phase = 'login';
  }
</script>

{#if !$isLoading}
  {#if phase === 'boot'}
    <div class="center">
      <StateBlock tone="loading" title={$_('common.loading')} />
    </div>
  {:else if phase === 'unreachable'}
    <div class="center">
      <StateBlock tone="error" title={$_('fleet.error_title')} body={$_('auth.unreachable')}>
        <button class="lp-btn primary" onclick={bootstrap}>{$_('common.retry')}</button>
      </StateBlock>
    </div>
  {:else if phase === 'setup'}
    <SetupView onDone={() => enter()} />
  {:else if phase === 'login'}
    <LoginView {notice} onDone={enter} />
  {:else}
    <Shell {version} {onExpired} {logout} />
  {/if}
{/if}

<style>
  .center {
    min-height: 100dvh;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 24px;
  }
  .center :global(.block) {
    border: none;
    background: none;
  }
</style>
