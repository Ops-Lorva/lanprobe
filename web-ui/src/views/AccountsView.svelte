<script lang="ts">
  import { onMount } from 'svelte';
  import { _, locale } from 'svelte-i18n';
  import { api, ApiError, type Account, type Role } from '$lib/api';
  import { noteAdmin } from '$lib/session';
  import StateBlock from '$lib/components/StateBlock.svelte';
  import Modal from '$lib/components/Modal.svelte';
  import RoleChoice from '$lib/components/RoleChoice.svelte';
  import { dateOnly } from '$lib/time';

  const { onExpired } = $props<{ onExpired: () => void }>();
  const lang = $derived($locale ?? 'en');

  let loading = $state(true);
  let loadError = $state('');
  let denied = $state('');
  let accounts = $state<Account[]>([]);
  /** Refus du hub sur une action lancée hors dialogue (réactivation). */
  let rowError = $state('');

  /**
   * Toute réponse du hub passe par ici. Un 401 renvoie à la connexion ; un 403
   * n'y renvoie pas — se reconnecter avec les mêmes identifiants donnerait le
   * même refus (contrat § 11).
   */
  function handle(e: unknown): string {
    if (e instanceof ApiError && e.isExpired) {
      onExpired();
      return '';
    }
    if (e instanceof ApiError && e.isForbidden) noteAdmin(false);
    return e instanceof ApiError ? e.message : String(e);
  }

  async function load() {
    loading = true;
    loadError = '';
    denied = '';
    rowError = '';
    try {
      accounts = (await api.users()) ?? [];
      noteAdmin(true);
    } catch (e) {
      const msg = handle(e);
      if (e instanceof ApiError && e.isForbidden) denied = msg;
      else loadError = msg;
    } finally {
      loading = false;
    }
  }

  onMount(load);

  // Actifs d'abord : ce sont les comptes qui peuvent entrer, donc ceux qu'on
  // vient vérifier. Les désactivés restent visibles en dessous — ils ne sont
  // pas supprimés, les cacher reviendrait à faire croire qu'ils l'ont été.
  const sorted = $derived(
    [...accounts].sort(
      (a, b) =>
        Number(a.disabled_at != null) - Number(b.disabled_at != null) ||
        a.username.localeCompare(b.username, lang),
    ),
  );
  const activeCount = $derived(accounts.filter((a) => a.disabled_at == null).length);

  // ── Création ─────────────────────────────────────────────────────────────
  let createOpen = $state(false);
  let newName = $state('');
  let newPassword = $state('');
  let newRole = $state<Role>('viewer');
  let createError = $state('');
  let busy = $state(false);

  function openCreate() {
    newName = '';
    newPassword = '';
    // Le rôle le moins puissant par défaut : un oubli doit coûter un droit
    // manquant, jamais un droit accordé sans qu'on l'ait voulu.
    newRole = 'viewer';
    createError = '';
    createOpen = true;
  }

  async function submitCreate() {
    createError = '';
    if (!newName.trim()) {
      createError = $_('accounts.err_username');
      return;
    }
    busy = true;
    try {
      await api.createUser(newName.trim(), newPassword, newRole);
      createOpen = false;
      await load();
    } catch (e) {
      createError = handle(e);
    } finally {
      busy = false;
    }
  }

  // ── Changement de rôle ───────────────────────────────────────────────────
  let roleTarget = $state<Account | null>(null);
  let roleValue = $state<Role>('viewer');
  let roleError = $state('');

  function openRole(a: Account) {
    roleTarget = a;
    roleValue = a.role;
    roleError = '';
  }

  async function submitRole() {
    if (!roleTarget) return;
    roleError = '';
    busy = true;
    try {
      await api.patchUser(roleTarget.username, { role: roleValue });
      roleTarget = null;
      await load();
    } catch (e) {
      // Le hub refuse (409) de rétrograder le dernier administrateur actif et
      // de se rétrograder soi-même. On affiche SA phrase : elle nomme la règle,
      // là où un bouton grisé aurait laissé chercher.
      roleError = handle(e);
    } finally {
      busy = false;
    }
  }

  // ── Mot de passe ─────────────────────────────────────────────────────────
  let pwTarget = $state<Account | null>(null);
  let pwValue = $state('');
  let pwError = $state('');
  let pwDone = $state('');

  function openPassword(a: Account) {
    pwTarget = a;
    pwValue = '';
    pwError = '';
    pwDone = '';
  }

  async function submitPassword() {
    if (!pwTarget) return;
    pwError = '';
    busy = true;
    try {
      await api.setUserPassword(pwTarget.username, pwValue);
      pwDone = $_('accounts.pw_done', { values: { name: pwTarget.username } });
      pwTarget = null;
    } catch (e) {
      pwError = handle(e);
    } finally {
      busy = false;
    }
  }

  // ── Désactivation / réactivation ─────────────────────────────────────────
  let offTarget = $state<Account | null>(null);
  let offError = $state('');

  async function confirmDisable() {
    if (!offTarget) return;
    offError = '';
    busy = true;
    try {
      await api.patchUser(offTarget.username, { disabled: true });
      offTarget = null;
      await load();
    } catch (e) {
      offError = handle(e);
    } finally {
      busy = false;
    }
  }

  /** Réactiver ne coupe l'accès de personne : pas de dialogue à franchir. */
  async function enable(a: Account) {
    rowError = '';
    busy = true;
    try {
      await api.patchUser(a.username, { disabled: false });
      await load();
    } catch (e) {
      rowError = handle(e);
    } finally {
      busy = false;
    }
  }
</script>

<header class="head">
  <div class="titles">
    <h1>{$_('accounts.title')}</h1>
    {#if !loading && !denied && !loadError}
      <p class="count">
        {$_('accounts.count', { values: { active: activeCount, total: accounts.length } })}
      </p>
    {/if}
  </div>
  {#if !denied && !loadError}
    <button class="lp-btn primary" onclick={openCreate}>{$_('accounts.new')}</button>
  {/if}
</header>

{#if loading}
  <StateBlock tone="loading" title={$_('common.loading')} />
{:else if denied}
  <!-- 403 : le rôle ne suffit pas. On montre la phrase du hub plutôt qu'une
       formule maison — c'est elle qui nomme la règle appliquée. -->
  <StateBlock tone="error" title={$_('common.forbidden_title')} body={denied} />
{:else if loadError}
  <StateBlock tone="error" title={$_('accounts.error_title')} body={loadError}>
    <button class="lp-btn primary" onclick={load}>{$_('common.retry')}</button>
  </StateBlock>
{:else}
  {#if rowError}<p class="err" role="alert">{rowError}</p>{/if}
  {#if pwDone}<p class="ok" role="status">{pwDone}</p>{/if}

  <div class="list lp-card">
    <div class="row header" aria-hidden="true">
      <span>{$_('accounts.col_account')}</span>
      <span>{$_('accounts.col_role')}</span>
      <span>{$_('accounts.col_created')}</span>
      <span class="right">{$_('accounts.col_actions')}</span>
    </div>

    {#each sorted as a (a.username)}
      <div class="row" class:off={a.disabled_at != null}>
        <span class="who">
          <span class="nm">{a.username}</span>
          {#if a.disabled_at != null}
            <span class="pill off-pill">
              {$_('accounts.disabled_on', { values: { date: dateOnly(a.disabled_at, lang) } })}
            </span>
          {/if}
        </span>

        <span class="role">
          <span class="pill role-{a.role}">{$_(`accounts.role_${a.role}`)}</span>
        </span>

        <span class="created lp-mono">{dateOnly(a.created_at, lang)}</span>

        <!--
          Aucun bouton n'est masqué ni grisé pour anticiper un refus : le hub
          protège le dernier administrateur actif et interdit de se désactiver
          soi-même, et son message explique la règle. Un bouton absent aurait
          laissé chercher pourquoi.
        -->
        <span class="acts">
          <button class="lp-btn sm" onclick={() => openRole(a)}>{$_('accounts.change_role')}</button>
          <button class="lp-btn sm" onclick={() => openPassword(a)}>
            {$_('accounts.change_password')}
          </button>
          {#if a.disabled_at == null}
            <button class="lp-btn danger sm" onclick={() => (offTarget = a)}>
              {$_('accounts.disable')}
            </button>
          {:else}
            <button class="lp-btn accent sm" disabled={busy} onclick={() => enable(a)}>
              {$_('accounts.enable')}
            </button>
          {/if}
        </span>
      </div>
    {/each}
  </div>
{/if}

<Modal open={createOpen} title={$_('accounts.new_title')} onclose={() => (createOpen = false)}>
  <label class="lp-field">
    {$_('accounts.username')}
    <input
      class="lp-input"
      bind:value={newName}
      autocomplete="off"
      spellcheck="false"
      placeholder={$_('accounts.username_placeholder')}
    />
  </label>
  <label class="lp-field">
    {$_('accounts.password')}
    <input class="lp-input" type="password" bind:value={newPassword} autocomplete="new-password" />
    <span class="hint">{$_('accounts.password_hint')}</span>
  </label>
  <div class="lp-field">
    {$_('accounts.role')}
    <RoleChoice group="new-role" value={newRole} onchange={(r) => (newRole = r)} />
  </div>
  {#if createError}<p class="err" role="alert">{createError}</p>{/if}
  {#snippet footer()}
    <button class="lp-btn" onclick={() => (createOpen = false)}>{$_('common.cancel')}</button>
    <button class="lp-btn primary" onclick={submitCreate} disabled={busy}>
      {busy ? $_('accounts.creating') : $_('accounts.create')}
    </button>
  {/snippet}
</Modal>

<Modal
  open={roleTarget !== null}
  title={$_('accounts.role_title', { values: { name: roleTarget?.username ?? '' } })}
  onclose={() => (roleTarget = null)}
>
  <RoleChoice group="edit-role" value={roleValue} onchange={(r) => (roleValue = r)} />
  {#if roleError}<p class="err" role="alert">{roleError}</p>{/if}
  {#snippet footer()}
    <button class="lp-btn" onclick={() => (roleTarget = null)}>{$_('common.cancel')}</button>
    <button
      class="lp-btn primary"
      onclick={submitRole}
      disabled={busy || roleValue === roleTarget?.role}
    >
      {busy ? $_('common.saving') : $_('common.save')}
    </button>
  {/snippet}
</Modal>

<Modal
  open={pwTarget !== null}
  title={$_('accounts.pw_title', { values: { name: pwTarget?.username ?? '' } })}
  onclose={() => (pwTarget = null)}
>
  <label class="lp-field">
    {$_('accounts.new_password')}
    <input class="lp-input" type="password" bind:value={pwValue} autocomplete="new-password" />
    <span class="hint">{$_('accounts.password_hint')}</span>
  </label>
  {#if pwError}<p class="err" role="alert">{pwError}</p>{/if}
  {#snippet footer()}
    <button class="lp-btn" onclick={() => (pwTarget = null)}>{$_('common.cancel')}</button>
    <button class="lp-btn primary" onclick={submitPassword} disabled={busy}>
      {busy ? $_('common.saving') : $_('common.save')}
    </button>
  {/snippet}
</Modal>

<!--
  Désactiver, jamais supprimer. Le dialogue garde ses phrases : c'est une action
  qui coupe un accès, et la raison du choix — le journal nomme ce compte — est
  ce qui empêche de le réclamer à nouveau dans six mois.
-->
<Modal
  open={offTarget !== null}
  title={$_('accounts.disable_title', { values: { name: offTarget?.username ?? '' } })}
  onclose={() => (offTarget = null)}
>
  <p>{$_('accounts.disable_body')}</p>
  <p class="keeps">{$_('accounts.disable_keeps')}</p>
  {#if offError}<p class="err" role="alert">{offError}</p>{/if}
  {#snippet footer()}
    <button class="lp-btn" onclick={() => (offTarget = null)}>{$_('common.cancel')}</button>
    <button class="lp-btn danger" onclick={confirmDisable} disabled={busy}>
      {busy ? $_('accounts.disabling') : $_('accounts.disable_confirm')}
    </button>
  {/snippet}
</Modal>

<style>
  .head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
    margin-bottom: 14px;
  }
  h1 {
    font-size: 20px;
    font-weight: 800;
    letter-spacing: -0.02em;
    margin: 0;
  }
  .count {
    font-size: 12px;
    color: var(--ep-text-secondary);
    margin: 3px 0 0;
  }

  .list {
    overflow: hidden;
  }

  /* Grille et non <table> : c'est le gabarit déjà utilisé par le parc, et il
     laisse une colonne se replier au lieu de pousser la page en défilement
     horizontal. La colonne d'actions est à droite parce que l'œil finit sa
     lecture là — et parce que c'est le seul endroit où elle ne coupe pas le
     balayage vertical des noms de comptes. */
  /* Largeurs fixes partout sauf le nom : l'en-tête et les lignes sont des
     grilles indépendantes, et une colonne `auto` s'y dimensionnerait sur son
     propre contenu — les intitulés se seraient posés à côté de leurs valeurs
     au lieu d'au-dessus. */
  .row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 120px 108px 300px;
    align-items: center;
    gap: 10px;
    padding: 10px 14px;
    border-bottom: 1px solid var(--ep-border);
    font-size: 12.5px;
  }
  .row:last-child {
    border-bottom: none;
  }
  .row.header {
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.8px;
    color: var(--ep-text-dim);
    background: var(--ep-bg-tertiary);
    padding: 7px 14px;
  }
  /* Atténué, jamais barré : un texte rayé se lit « supprimé », et c'est
     précisément ce qui n'arrive pas ici. */
  .row.off .nm,
  .row.off .created {
    color: var(--ep-text-muted);
  }

  .who {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    min-width: 0;
  }
  .nm {
    font-weight: 600;
    color: var(--ep-text-primary);
    overflow-wrap: anywhere;
  }
  .created {
    color: var(--ep-text-secondary);
    font-size: 11.5px;
  }
  .right {
    text-align: right;
  }

  .pill {
    display: inline-flex;
    align-items: center;
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.3px;
    padding: 2px 7px;
    border-radius: 999px;
    border: 1px solid var(--ep-border-strong);
    color: var(--ep-text-secondary);
    white-space: nowrap;
  }
  /* Seul `admin` est saturé. La question qu'on vient poser à cet écran est
     « qui peut tout faire ? » : si les trois rôles étaient colorés, il faudrait
     lire les libellés pour compter. Là, la réponse se compte à l'œil. */
  .pill.role-admin {
    border-color: var(--ep-accent);
    background: var(--ep-accent-dim);
    color: var(--ep-accent-bright);
  }
  .pill.role-operator {
    color: var(--ep-text-primary);
  }
  .pill.role-viewer {
    border-color: var(--ep-border);
    color: var(--ep-text-muted);
  }
  .pill.off-pill {
    border-color: color-mix(in srgb, var(--ep-danger) 45%, var(--ep-border));
    color: var(--ep-danger);
    background: color-mix(in srgb, var(--ep-danger) 8%, transparent);
  }

  .acts {
    display: flex;
    gap: 6px;
    justify-content: flex-end;
    flex-wrap: wrap;
  }

  .hint {
    font-size: 11px;
    color: var(--ep-text-muted);
    line-height: 1.5;
  }
  .err {
    font-size: 12px;
    color: var(--ep-danger);
    margin: 0 0 10px;
    border-left: 2px solid var(--ep-danger);
    padding-left: 8px;
  }
  .ok {
    font-size: 12px;
    color: var(--ep-success);
    margin: 0 0 10px;
  }
  .keeps {
    color: var(--ep-text-primary);
    border-left: 2px solid var(--ep-accent);
    padding-left: 10px;
    margin: 0;
  }

  /* Sous 760 px, la date de création part la première : c'est du contexte, pas
     une décision. Sous 560 px, la ligne se replie en deux étages plutôt que de
     comprimer les boutons jusqu'à l'illisible — à 390 px, « Désactiver » doit
     rester lisible en entier. */
  @media (max-width: 760px) {
    .row {
      grid-template-columns: minmax(0, 1fr) 300px;
    }
    .created,
    .row.header span:nth-child(3) {
      display: none;
    }
  }
  @media (max-width: 560px) {
    .row {
      grid-template-columns: minmax(0, 1fr) auto;
      row-gap: 8px;
    }
    .row.header {
      display: none;
    }
    .acts {
      grid-column: 1 / -1;
      justify-content: flex-start;
    }
  }
</style>
