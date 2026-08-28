<script lang="ts">
  import { onMount } from 'svelte';
  import { isLoading, _ } from 'svelte-i18n';
  import { api, ApiError, probeSession } from '$lib/api';
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
      // `/api/status` ne dit pas si la session est valide (le contrat ne prévoit
      // pas ce champ) : on interroge une route protégée pour trancher.
      phase = (await probeSession()) ? 'app' : 'login';
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) phase = 'login';
      else phase = 'unreachable';
    }
  }

  onMount(bootstrap);

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
    <SetupView onDone={() => (phase = 'app')} />
  {:else if phase === 'login'}
    <LoginView {notice} onDone={() => ((notice = ''), (phase = 'app'))} />
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
