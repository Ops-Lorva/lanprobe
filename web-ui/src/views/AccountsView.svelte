<script lang="ts">
  import { onMount } from 'svelte';
  import { _, locale } from 'svelte-i18n';
  import { api, ApiError, type Account, type Role, type Site } from '$lib/api';
  import ActionMenu from '$lib/components/ActionMenu.svelte';
  import StateBlock from '$lib/components/StateBlock.svelte';
  import Modal from '$lib/components/Modal.svelte';
  import RoleChoice from '$lib/components/RoleChoice.svelte';
  import { dateOnly, relativeTime, now } from '$lib/time';
  import { devicesApi, deviceRows, parseDevices, type DeviceRow } from '$lib/devices';
  import { initialScope, scopePatch, scopeSummary, type ScopeMode } from '$lib/account-scope';

  const { onExpired } = $props<{ onExpired: () => void }>();
  const lang = $derived($locale ?? 'en');

  let loading = $state(true);
  let loadError = $state('');
  let denied = $state('');
  let accounts = $state<Account[]>([]);
  /** Refus du hub sur une action lancée hors dialogue (réactivation). */
  let rowError = $state('');
  /** Les sites du parc, pour composer une portée (contrat § 17). */
  let sites = $state<Site[]>([]);

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
    return e instanceof ApiError ? e.message : String(e);
  }

  async function load() {
    loading = true;
    loadError = '';
    denied = '';
    rowError = '';
    try {
      accounts = (await api.users()) ?? [];
      // Chargés avec les comptes : le sélecteur de portée doit pouvoir
      // s'ouvrir sans attendre, et un échec ici ne doit pas priver l'écran
      // de ce qu'il sait déjà afficher.
      sites = await api.sites().catch(() => []);
    } catch (e) {
      const msg = handle(e);
      if (e instanceof ApiError && e.isForbidden) denied = msg;
      else loadError = msg;
    } finally {
      loading = false;
    }
  }

  // ── Appareils de tous les comptes ────────────────────────────────────────
  // Un opérateur parti de l'entreprise ne révoque pas lui-même son téléphone.
  let devices = $state<DeviceRow[]>([]);
  let devicesError = $state('');
  let showHidden = $state(false);
  let devBusy = $state('');
  let revokeTarget = $state<DeviceRow | null>(null);

  async function loadDevices() {
    devicesError = '';
    try {
      devices = deviceRows(parseDevices(await devicesApi.allDevices(showHidden)));
    } catch (e) {
      devicesError = handle(e);
    }
  }

  async function confirmRevoke() {
    if (!revokeTarget) return;
    devBusy = revokeTarget.device.device_id;
    try {
      await devicesApi.revokeDevice(revokeTarget.device.device_id);
      revokeTarget = null;
      await loadDevices();
    } catch (e) {
      devicesError = handle(e);
    } finally {
      devBusy = '';
    }
  }

  /** ⚠️ Seulement un appareil révoqué : le hub refuse les autres, l'écran n'offre pas le geste. */
  async function setHidden(r: DeviceRow, archived: boolean) {
    devBusy = r.device.device_id;
    try {
      await devicesApi.archiveDevice(r.device.device_id, archived);
      await loadDevices();
    } catch (e) {
      devicesError = handle(e);
    } finally {
      devBusy = '';
    }
  }

  onMount(() => {
    void load();
    void loadDevices();
  });

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

  // ── Portée par site (contrat § 17) ───────────────────────────────────────
  //
  // 🔴 « Tous les sites » est un CHOIX, plus une absence de case (audit RSSI,
  // F1) : une liste vide valait « tout », et supprimer le dernier site d'un
  // compte restreint lui ouvrait le parc entier. Aucune case cochée veut
  // désormais dire « rien », et le dialogue le dit en toutes lettres.
  let scopeTarget = $state<Account | null>(null);
  let scopeMode = $state<ScopeMode>('all');
  let scopePicked = $state<string[]>([]);
  let scopeError = $state('');
  let scopeBusy = $state(false);

  function openScope(a: Account) {
    scopeTarget = a;
    const initial = initialScope({ ...a, sites: a.sites ?? [] });
    scopeMode = initial.mode;
    scopePicked = initial.picked;
    scopeError = '';
  }

  function toggled(picked: string[], siteId: string): string[] {
    return picked.includes(siteId) ? picked.filter((s) => s !== siteId) : [...picked, siteId];
  }

  async function submitScope() {
    if (!scopeTarget) return;
    scopeBusy = true;
    scopeError = '';
    try {
      await api.patchUser(scopeTarget.username, scopePatch(scopeMode, scopePicked));
      scopeTarget = null;
      await load();
    } catch (e) {
      scopeError = handle(e);
    } finally {
      scopeBusy = false;
    }
  }

  /** Ce que voit un compte, en une phrase, pour la colonne du tableau. */
  function scopeLabel(a: Account): string {
    const { key, values } = scopeSummary({ ...a, sites: a.sites ?? [] });
    return $_(key, { values });
  }

  // ── Création ─────────────────────────────────────────────────────────────
  let createOpen = $state(false);
  let newName = $state('');
  let newPassword = $state('');
  let newRole = $state<Role>('viewer');
  /** Portée posée à la création : jamais un instant en « tous » par défaut. */
  let newScopeMode = $state<ScopeMode>('all');
  let newScopePicked = $state<string[]>([]);
  let createError = $state('');
  let busy = $state(false);

  function openCreate() {
    newName = '';
    newPassword = '';
    // Le rôle le moins puissant par défaut : un oubli doit coûter un droit
    // manquant, jamais un droit accordé sans qu'on l'ait voulu.
    newRole = 'viewer';
    newScopeMode = 'all';
    newScopePicked = [];
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
      await api.createUser(
        newName.trim(),
        newPassword,
        newRole,
        // Un `admin` voit tout : le hub refuse de le restreindre.
        newRole === 'admin' ? undefined : scopePatch(newScopeMode, newScopePicked),
      );
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
  async function toggleDemo(a: Account) {
    rowError = '';
    busy = true;
    try {
      await api.patchUser(a.username, { demo: !a.demo });
      await load();
    } catch (e) {
      rowError = handle(e);
    } finally {
      busy = false;
    }
  }

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

<header class="lp-head">
  <div>
    <h1>{$_('nav.accounts')}</h1>
    {#if !loading && !denied && !loadError}
      <p class="lp-sub">
        {$_('accounts.count', { values: { active: activeCount, total: accounts.length } })}
      </p>
    {/if}
  </div>
  {#if !denied && !loadError}
    <div class="lp-acts">
      <button class="lp-btn primary" onclick={openCreate}>{$_('accounts.new')}</button>
    </div>
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

  <div class="lp-tbl-wrap">
    <table class="lp-t">
      <thead>
        <tr>
          <th>{$_('accounts.col_account')}</th>
          <th>{$_('accounts.col_role')}</th>
          <th>{$_('accounts.col_scope')}</th>
          <th>{$_('accounts.col_created')}</th>
          <th class="act">{$_('accounts.col_actions')}</th>
        </tr>
      </thead>
      <tbody>
        {#each sorted as a (a.username)}
          <tr class:off={a.disabled_at != null}>
            <td>
              <span class="nm">{a.username}</span>
              {#if a.demo}<span class="lp-tag">{$_('accounts.demo_pill')}</span>{/if}
              {#if a.disabled_at != null}
                <span class="sub2">
                  {$_('accounts.disabled_on', { values: { date: dateOnly(a.disabled_at, lang) } })}
                </span>
              {/if}
            </td>
            <td>{$_(`accounts.role_${a.role}`)}</td>
            <td class="scope">{scopeLabel(a)}</td>
            <td class="num">{dateOnly(a.created_at, lang)}</td>
            <!--
              Aucun bouton n'est masqué ni grisé pour anticiper un refus : le hub
              protège le dernier administrateur actif et interdit de se
              désactiver soi-même, et son message explique la règle.
            -->
            <td class="act">
              <button class="lp-btn ghost sm" onclick={() => openRole(a)}>{$_('accounts.change_role')}</button>
              <!-- Masqué pour un `admin` : le hub refuse de le restreindre. -->
              {#if a.role !== 'admin'}
                <button class="lp-btn ghost sm" onclick={() => openScope(a)}>
                  {$_('accounts.change_scope')}
                </button>
              {/if}
              {#if a.disabled_at == null}
                <button class="lp-btn ghost sm dis" onclick={() => (offTarget = a)}>
                  {$_('accounts.disable')}
                </button>
              {:else}
                <button class="lp-btn ghost sm" disabled={busy} onclick={() => enable(a)}>
                  {$_('accounts.enable')}
                </button>
              {/if}
              <ActionMenu icon="more" label={$_('accounts.more', { values: { name: a.username } })}>
                {#snippet children(close)}
                  <button class="lp-mi" role="menuitem" onclick={() => (close(), openPassword(a))}>
                    {$_('accounts.change_password')}
                  </button>
                  <!-- Mode démo : un compte partagé entre inconnus, verrouillé
                       par le hub (contrat § 27). Jamais sur un `admin`. -->
                  {#if a.role !== 'admin'}
                    <button class="lp-mi" role="menuitem" disabled={busy} onclick={() => (close(), toggleDemo(a))}>
                      {a.demo ? $_('accounts.demo_off') : $_('accounts.demo_on')}
                    </button>
                  {/if}
                {/snippet}
              </ActionMenu>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>

  <div class="lp-sec-head">
    <h2>{$_('account.devices_title')} <span class="lp-sub">{$_('accounts.devices_all_scope')}</span></h2>
    <div class="lp-acts">
      <button class="lp-btn ghost sm" onclick={() => ((showHidden = !showHidden), loadDevices())}>
        {showHidden ? $_('account.devices_hide_hidden') : $_('account.devices_show_hidden')}
      </button>
      <a class="lp-btn sm" href="#/account">{$_('account.devices_pair_cta')}</a>
    </div>
  </div>
  {#if devicesError}<p class="err" role="alert">{devicesError}</p>{/if}
  {#if devices.length === 0 && !devicesError}
    <p class="empty">{$_('account.devices_empty')}</p>
  {:else if devices.length > 0}
    <div class="lp-tbl-wrap">
      <table class="lp-t">
        <thead>
          <tr>
            <th>{$_('account.devices_col_name')}</th>
            <th>{$_('account.devices_owner')}</th>
            <th>{$_('account.devices_col_platform')}</th>
            <th class="num">{$_('account.devices_col_paired')}</th>
            <th class="num">{$_('account.devices_col_last_seen')}</th>
            <th>{$_('account.devices_col_last_ip')}</th>
            <th class="act">{$_('accounts.col_actions')}</th>
          </tr>
        </thead>
        <tbody>
          {#each devices as r (r.device.device_id)}
            {@const d = r.device}
            <tr class:off={r.revoked}>
              <td>
                <span class="nm">{d.name}</span>
                {#if r.revoked && d.revoked_at}
                  <span class="sub2">{$_('account.devices_revoked', { values: { date: dateOnly(d.revoked_at, lang) } })}</span>
                {/if}
              </td>
              <td>{d.username}</td>
              <td>{[d.platform, d.app_version].filter(Boolean).join(' ') || '—'}</td>
              <td class="num">{dateOnly(d.created_at, lang)}</td>
              <!-- ⚠️ Affichée telle quelle, jamais convertie en verdict : il n'y a
                   aucune révocation par inactivité. -->
              <td class="num">
                {#if d.last_seen == null}{$_('account.devices_never_seen')}
                {:else if r.revoked}{dateOnly(d.last_seen, lang)}
                {:else}{relativeTime(d.last_seen, lang, Math.max($now, d.last_seen * 1000))}{/if}
              </td>
              <td><span class="lp-mono ip">{d.last_ip ?? ''}</span></td>
              <td class="act">
                {#if !r.revoked}
                  <button class="lp-btn ghost sm dis" disabled={devBusy !== ''} onclick={() => (revokeTarget = r)}>
                    {$_('account.devices_revoke')}
                  </button>
                {:else if r.archived}
                  <button class="lp-btn ghost sm" disabled={devBusy !== ''} title={$_('account.devices_unhide_hint')} onclick={() => setHidden(r, false)}>
                    {$_('account.devices_unhide')}
                  </button>
                {:else}
                  <button class="lp-btn ghost sm" disabled={devBusy !== ''} title={$_('account.devices_hide_hint')} onclick={() => setHidden(r, true)}>
                    {$_('account.devices_hide')}
                  </button>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
{/if}

<Modal
  open={revokeTarget !== null}
  title={$_('account.devices_revoke')}
  onclose={() => (revokeTarget = null)}
>
  <p>{$_('account.devices_revoke_confirm', { values: { name: revokeTarget?.device.name ?? '' } })}</p>
  {#snippet footer()}
    <button class="lp-btn" onclick={() => (revokeTarget = null)}>{$_('common.cancel')}</button>
    <button class="lp-btn danger" onclick={confirmRevoke} disabled={devBusy !== ''}>
      {$_('account.devices_revoke')}
    </button>
  {/snippet}
</Modal>

<!--
  Le choix de portée, partagé par la création et la modification. Deux
  boutons radio et non une case « tout » implicite : « aucune case = tous »
  était le contresens que l'audit RSSI a transformé en faille (F1).
-->
{#snippet scopePicker(
  group: string,
  mode: ScopeMode,
  picked: string[],
  onmode: (m: ScopeMode) => void,
  ontoggle: (id: string) => void,
)}
  <div class="scope-modes">
    <label class="scope-row">
      <input type="radio" name={group} checked={mode === 'all'} onchange={() => onmode('all')} />
      <span class="scope-name">{$_('accounts.scope_mode_all')}</span>
    </label>
    <label class="scope-row">
      <input
        type="radio"
        name={group}
        checked={mode === 'sites'}
        onchange={() => onmode('sites')}
      />
      <span class="scope-name">{$_('accounts.scope_mode_sites')}</span>
    </label>
  </div>
  {#if mode === 'sites'}
    {#if sites.length === 0}
      <p class="dlg-lead">{$_('accounts.scope_no_site')}</p>
    {:else}
      <div class="scope-list">
        {#each sites as s (s.site_id)}
          <label class="scope-row">
            <input
              type="checkbox"
              checked={picked.includes(s.site_id)}
              onchange={() => ontoggle(s.site_id)}
            />
            <span class="scope-name">{s.name}</span>
            <span class="scope-count">
              {$_('accounts.scope_probe_count', { values: { n: s.probe_count } })}
            </span>
          </label>
        {/each}
      </div>
    {/if}
    <!-- ⚠️ La phrase qui compte : aucune case cochée veut dire « ne voit
         RIEN ». Elle se voit quand c'est l'état courant. -->
    <p class="dlg-note" class:accent={picked.length === 0}>
      {picked.length === 0
        ? $_('accounts.scope_empty_means_nothing')
        : $_('accounts.scope_selected', { values: { n: picked.length } })}
    </p>
  {/if}
{/snippet}

<Modal
  open={scopeTarget !== null}
  title={$_('accounts.scope_title', { values: { name: scopeTarget?.username ?? '' } })}
  onclose={() => (scopeTarget = null)}
>
  <p class="dlg-lead">{$_('accounts.scope_lead')}</p>
  {@render scopePicker(
    'edit-scope',
    scopeMode,
    scopePicked,
    (m) => (scopeMode = m),
    (id) => (scopePicked = toggled(scopePicked, id)),
  )}

  {#if scopeError}<p class="dlg-err" role="alert">{scopeError}</p>{/if}
  {#snippet footer()}
    <button class="lp-btn" onclick={() => (scopeTarget = null)}>{$_('common.cancel')}</button>
    <button class="lp-btn primary" onclick={submitScope} disabled={scopeBusy}>
      {scopeBusy ? $_('common.saving') : $_('common.save')}
    </button>
  {/snippet}
</Modal>

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
  {#if newRole !== 'admin'}
    <div class="lp-field">
      {$_('accounts.scope')}
      {@render scopePicker(
        'new-scope',
        newScopeMode,
        newScopePicked,
        (m) => (newScopeMode = m),
        (id) => (newScopePicked = toggled(newScopePicked, id)),
      )}
    </div>
  {/if}
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
  .lp-sec-head {
    margin-top: 40px;
  }
  .lp-sec-head h2 .lp-sub {
    font-weight: 400;
    margin-left: 8px;
  }
  .ip {
    font-size: 0.8125rem;
    color: var(--color-ink-2);
  }
  .empty {
    color: var(--color-muted);
    font-size: 0.875rem;
  }
  .nm {
    font-weight: 500;
  }
  .lp-tag {
    margin-left: 8px;
  }
  tr.off .nm {
    color: var(--color-muted);
  }
  .scope {
    white-space: normal;
  }
  .act {
    white-space: nowrap;
  }
  .act > :global(*) {
    vertical-align: middle;
  }
  .act :global(.lp-btn.dis) {
    color: var(--st-bad);
  }
  /* Le décompte à gauche, l'action à droite, et rien entre les deux : la ligne
     tient sur une seule à 390 px, et se replie proprement si la traduction
     s'allonge. */


  /* Grille et non <table> : c'est le gabarit déjà utilisé par le parc, et il
     laisse une colonne se replier au lieu de pousser la page en défilement
     horizontal. La colonne d'actions est à droite parce que l'œil finit sa
     lecture là — et parce que c'est le seul endroit où elle ne coupe pas le
     balayage vertical des noms de comptes. */
  /* Largeurs fixes partout sauf le nom : l'en-tête et les lignes sont des
     grilles indépendantes, et une colonne `auto` s'y dimensionnerait sur son
     propre contenu — les intitulés se seraient posés à côté de leurs valeurs
     au lieu d'au-dessus. */
  .scope-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
    max-height: 260px;
    overflow-y: auto;
    margin: 8px 0;
  }
  .scope-row {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    align-items: center;
    gap: 8px;
    padding: 6px 4px;
    border-bottom: 1px solid var(--ep-border);
    cursor: pointer;
    font-size: 0.875rem;
  }
  .scope-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .scope-count {
    font-size: 0.8125rem;
    color: var(--ep-text-secondary);
  }
  /* Les dialogues de cet écran n'avaient encore ni texte d'introduction ni
     ligne d'erreur : les deux classes viennent d'ailleurs, elles se posent
     ici avec la même forme que dans le parc. */
  .dlg-lead {
    margin: 0 0 8px;
    font-size: 0.8125rem;
    color: var(--ep-text-secondary);
    line-height: 1.5;
  }
  .dlg-err {
    margin: 8px 0 0;
    font-size: 0.8125rem;
    color: var(--ep-danger, #c0392b);
  }
  .dlg-note {
    margin: 6px 0 0;
    font-size: 0.8125rem;
    color: var(--ep-text-secondary);
  }
  /* « Aucune case = rien » doit se voir quand c'est l'état courant. */
  .scope-modes {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin-bottom: 6px;
  }
  .dlg-note.accent {
    color: var(--ep-accent, #b8860b);
    font-weight: 600;
  }
  /* Atténué, jamais barré : un texte rayé se lit « supprimé », et c'est
     précisément ce qui n'arrive pas ici. */


  /* Seul `admin` est saturé. La question qu'on vient poser à cet écran est
     « qui peut tout faire ? » : si les trois rôles étaient colorés, il faudrait
     lire les libellés pour compter. Là, la réponse se compte à l'œil. */


  .hint {
    font-size: 0.8125rem;
    color: var(--ep-text-muted);
    line-height: 1.5;
  }
  .err {
    font-size: 0.8125rem;
    color: var(--ep-danger);
    margin: 0 0 10px;
    border-left: 2px solid var(--ep-danger);
    padding-left: 8px;
  }
  .ok {
    font-size: 0.8125rem;
    color: var(--ep-success);
    margin: 0 0 10px;
  }
  .keeps {
    color: var(--ep-text-primary);
    border-left: 2px solid var(--ep-accent);
    padding-left: 10px;
    margin: 0;
  }

  /* Sous 760 px, la date de création part la première : c'est du contexte,
     pas une décision. Le tableau défile ensuite plutôt que de comprimer les
     boutons. */
  @media (max-width: 760px) {
    .lp-t th:nth-child(4),
    .lp-t td:nth-child(4) {
      display: none;
    }
  }
</style>
