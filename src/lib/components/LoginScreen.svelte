<script lang="ts">
  import LogoMark from './LogoMark.svelte';
  const { onDone } = $props<{ onDone: () => void }>();

  let username = $state('');
  let password = $state('');
  let busy = $state(false);
  let error = $state('');

  async function submit(e: Event) {
    e.preventDefault();
    error = '';
    busy = true;
    try {
      const r = await fetch('/api/auth/login', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        credentials: 'same-origin',
        body: JSON.stringify({ username: username.trim(), password }),
      });
      if (!r.ok) throw await r.text();
      onDone();
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }
</script>

<div class="overlay">
  <form class="card" onsubmit={submit}>
    <div class="brand-head"><LogoMark size={44} /><h1>LanProbe</h1></div>
    <p class="sub">Connexion</p>
    <label>
      Nom d'utilisateur
      <input type="text" bind:value={username} autocomplete="username" />
    </label>
    <label>
      Mot de passe
      <input type="password" bind:value={password} autocomplete="current-password" />
    </label>
    {#if error}<p class="error">{error}</p>{/if}
    <button type="submit" disabled={busy}>{busy ? 'Connexion…' : 'Se connecter'}</button>
  </form>
</div>

<style>
  .overlay { position: fixed; inset: 0; display: flex; align-items: center; justify-content: center; background: var(--ep-bg-primary, #0b0d11); }
  .card { background: var(--ep-bg-secondary, #141821); border: 1px solid var(--ep-border, #232832); border-radius: 12px; padding: 32px; width: 340px; display: flex; flex-direction: column; gap: 14px; color: var(--ep-text-primary, #e6e9ef); }
  h1 { font-size: 22px; font-weight: 700; margin: 0; }
  .brand-head { display: flex; align-items: center; gap: 12px; }
  .sub { font-size: 13px; color: var(--ep-text-secondary, #9aa2b1); margin: -4px 0 8px; }
  label { display: flex; flex-direction: column; gap: 4px; font-size: 13px; color: var(--ep-text-secondary, #9aa2b1); }
  input { background: var(--ep-bg-tertiary, #1b2029); border: 1px solid var(--ep-border, #232832); border-radius: 6px; padding: 9px 11px; color: var(--ep-text-primary, #e6e9ef); font-family: var(--ep-font-mono, monospace); font-size: 13px; }
  button { margin-top: 6px; padding: 10px 16px; border-radius: 6px; border: 1px solid var(--ep-accent, #5b8df0); background: var(--ep-accent, #5b8df0); color: #fff; font-size: 13px; font-weight: 600; cursor: pointer; }
  button:disabled { opacity: .6; cursor: default; }
  .error { color: var(--ep-danger, #e25c5c); font-size: 12px; margin: 0; }
</style>
