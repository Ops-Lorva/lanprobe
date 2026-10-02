<script lang="ts">
  import { _ } from 'svelte-i18n';
  import { api, ApiError, type PortscanProfile } from '$lib/api';
  import { canOperate } from '$lib/session';
  import { formatPorts, parsePorts, profileLabel } from '$lib/portscan-profiles';
  import StateBlock from './StateBlock.svelte';

  interface Props {
    onExpired: () => void;
  }

  let { onExpired }: Props = $props();

  let profiles = $state<PortscanProfile[] | null>(null);
  let loadError = $state('');
  let actionError = $state('');
  let busy = $state(false);

  /** Le profil en cours d'édition, `''` = aucun. */
  let editing = $state('');
  let editName = $state('');
  let editPorts = $state('');
  let editUdpPorts = $state('');

  let newName = $state('');
  let newPorts = $state('');
  let newUdpPorts = $state('');

  async function load() {
    loadError = '';
    try {
      profiles = (await api.portscanProfiles()).profiles;
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      loadError = e instanceof ApiError ? e.message : String(e);
    }
  }
  void load();

  /** Un seul chemin de sortie pour les trois gestes : ils échouent pareil. */
  async function run(action: () => Promise<unknown>) {
    busy = true;
    actionError = '';
    try {
      await action();
      await load();
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      actionError = e instanceof ApiError ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  function startEdit(p: PortscanProfile) {
    editing = p.profile_id;
    editName = p.name;
    editPorts = formatPorts(p.ports);
    editUdpPorts = formatPorts(p.udp_ports);
  }

  function saveEdit() {
    const id = editing;
    const name = editName.trim();
    const ports = parsePorts(editPorts);
    const udpPorts = parsePorts(editUdpPorts);
    editing = '';
    void run(() => api.updatePortscanProfile(id, name, ports, udpPorts));
  }

  function create() {
    const name = newName.trim();
    const ports = parsePorts(newPorts);
    const udpPorts = parsePorts(newUdpPorts);
    newName = '';
    newPorts = '';
    newUdpPorts = '';
    void run(() => api.createPortscanProfile(name, ports, udpPorts));
  }

  function remove(p: PortscanProfile) {
    void run(() => api.deletePortscanProfile(p.profile_id));
  }
</script>

<!--
  Un seul écran pour tout le hub : la portée d'un profil de scan est le parc
  entier (contrat § 25, décision 4). C'est le profil RÉSEAU qui décrit un site,
  et lui reste sur la sonde.
-->
<section class="card lp-card">
  <h2 class="lp-title">{$_('portscan.title')}</h2>
  <p class="hint">{$_('portscan.intro')}</p>

  {#if profiles === null}
    {#if loadError}
      <StateBlock tone="error" title={$_('portscan.load_error')} body={loadError}>
        <button class="lp-btn" onclick={load}>{$_('common.retry')}</button>
      </StateBlock>
    {:else}
      <p class="hint">{$_('charts.loading')}</p>
    {/if}
  {:else}
    {#if actionError}
      <p class="err">{actionError}</p>
    {/if}

    <!--
      🔴 **Le formulaire est AVANT la liste, et c'est une règle.** Placé après,
      il s'éloigne à mesure que la liste grandit : le geste le plus courant
      devient le plus coûteux, et il empire tout seul. À trois mille profils, il
      faudrait défiler pour en créer un.
    -->
    {#if $canOperate}
      <div class="row new">
        <input
          class="lp-input"
          bind:value={newName}
          placeholder={$_('portscan.name')}
          aria-label={$_('portscan.name')}
        />
        <input
          class="lp-input"
          bind:value={newPorts}
          spellcheck="false"
          placeholder={$_('portscan.ports_placeholder')}
          aria-label={$_('portscan.ports')}
        />
        <input
          class="lp-input"
          bind:value={newUdpPorts}
          spellcheck="false"
          placeholder={$_('portscan.udp_ports_placeholder')}
          aria-label={$_('portscan.udp_ports')}
        />
        <div class="acts">
          <button class="lp-btn primary" disabled={busy || !newName.trim()} onclick={create}>
            {$_('portscan.add')}
          </button>
        </div>
      </div>
      <!--
        ⚠️ Le hub ne joint JAMAIS une sonde : c'est elle qui l'appelle. On dit
        donc « c'est écrit côté hub, les sondes suivront », pas « c'est
        appliqué » — ce serait affirmer ce que le hub ne peut pas savoir.
      -->
      <p class="hint">{$_('portscan.applies_next_beat')}</p>
    {/if}

    <div class="rows">
      {#each profiles as p (p.profile_id)}
        <!-- Quatre colonnes en édition : nom, TCP, UDP, actions. -->
        <div class="row" class:editing={editing === p.profile_id}>
          {#if editing === p.profile_id}
            <input class="lp-input" bind:value={editName} aria-label={$_('portscan.name')} />
            <input
              class="lp-input"
              bind:value={editPorts}
              spellcheck="false"
              placeholder={$_('portscan.ports_placeholder')}
              aria-label={$_('portscan.ports')}
            />
            <input
              class="lp-input"
              bind:value={editUdpPorts}
              spellcheck="false"
              placeholder={$_('portscan.udp_ports_placeholder')}
              aria-label={$_('portscan.udp_ports')}
            />
            <div class="acts">
              <button class="lp-btn primary" disabled={busy || !editName.trim()} onclick={saveEdit}>
                {$_('common.save')}
              </button>
              <button class="lp-btn" onclick={() => (editing = '')}>{$_('common.cancel')}</button>
            </div>
          {:else}
            <div class="name">{profileLabel(p, $_)}</div>
            <!--
              ⚠️ « La liste de la sonde » et « aucun port » ne se disent pas
              pareil : un profil sans ports laisse la sonde employer la sienne,
              il ne restreint rien. Afficher « 0 port » ferait croire à un
              profil qui ne scanne rien.
            -->
            <div class="ports">
              {#if p.ports.length === 0}
                <span class="muted">{$_('portscan.probe_default')}</span>
              {:else}
                {formatPorts(p.ports)}
                <span class="muted">· {$_('portscan.count', { values: { n: p.ports.length } })}</span>
              {/if}
              <!--
                ⚠️ L'UDP ne se dit que s'il y en a : « 0 UDP » n'apprendrait
                rien, et la plupart des profils n'en portent pas.
              -->
              {#if p.udp_ports.length > 0}
                <span class="muted">· {$_('portscan.udp_count', { values: { n: p.udp_ports.length } })}</span>
              {/if}
            </div>
            <div class="acts">
              {#if p.origin_probe}
                <span class="tag" title={$_('portscan.from_probe_hint')}>
                  {$_('portscan.from_probe')}
                </span>
              {/if}
              {#if $canOperate}
                <button class="lp-btn" disabled={busy} onclick={() => startEdit(p)}>
                  {$_('portscan.edit')}
                </button>
                <button class="lp-btn danger" disabled={busy} onclick={() => remove(p)}>
                  {$_('portscan.delete')}
                </button>
              {/if}
            </div>
          {/if}
        </div>
      {/each}
      {#if profiles.length === 0}
        <p class="hint">{$_('portscan.empty')}</p>
      {/if}
    </div>
  {/if}
</section>

<style>
  .hint {
    font-size: 11px;
    color: var(--ep-text-muted);
    line-height: 1.55;
    max-width: 74ch;
  }
  .err {
    font-size: 11.5px;
    color: var(--ep-danger, #f87171);
  }
  .rows {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin: 12px 0;
  }
  .row {
    display: grid;
    grid-template-columns: minmax(120px, 1fr) minmax(160px, 2fr) auto;
    align-items: center;
    gap: 10px;
    padding: 8px 10px;
    border: 1px solid var(--ep-border);
    border-radius: 8px;
  }
  .row.new {
    border-style: dashed;
  }
  /* Saisie : le nom, les deux listes de ports, les actions. */
  .row.new,
  .row.editing {
    grid-template-columns: minmax(110px, 1fr) minmax(140px, 1.4fr) minmax(140px, 1.4fr) auto;
  }
  .name {
    font-size: 13px;
    font-weight: 600;
  }
  .ports {
    font-size: 11.5px;
    color: var(--ep-text-secondary);
    font-variant-numeric: tabular-nums;
    overflow-wrap: anywhere;
  }
  .muted {
    color: var(--ep-text-muted);
  }
  .tag {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    color: var(--ep-text-dim);
  }
  .acts {
    display: flex;
    align-items: center;
    gap: 6px;
    justify-content: flex-end;
  }
</style>
