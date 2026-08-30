<script lang="ts">
  import { onMount } from 'svelte';
  import { _ } from 'svelte-i18n';
  import { api, ApiError, type ProbeSubscription, type Subscriptions } from '$lib/api';
  import { canOperate } from '$lib/session';
  import StateBlock from './StateBlock.svelte';

  interface Props {
    siteId: string;
    /** Remonte l'échec d'authentification : c'est la vue qui sait où renvoyer. */
    onExpired: () => void;
  }
  let { siteId, onExpired }: Props = $props();

  let subs = $state<Subscriptions | null>(null);
  let loading = $state(true);
  let error = $state('');
  let busy = $state('');

  function handle(e: unknown): string {
    if (e instanceof ApiError && e.isExpired) {
      onExpired();
      return '';
    }
    return e instanceof ApiError ? e.message : String(e);
  }

  async function load() {
    loading = true;
    error = '';
    try {
      subs = await api.subscriptions();
    } catch (e) {
      subs = null;
      error = handle(e);
    } finally {
      loading = false;
    }
  }
  onMount(load);

  const site = $derived(subs?.sites.find((s) => s.site_id === siteId) ?? null);
  const probes = $derived<ProbeSubscription[]>(
    subs?.probes.filter((p) => p.site_id === siteId) ?? [],
  );

  /**
   * Après chaque écriture on relit tout.
   *
   * ⚠️ L'héritage est résolu par le hub : `enabled` est ce qui s'applique,
   * `exception` ce qui est réglé sur la sonde. Le recalculer en JS après un
   * clic serait la deuxième implémentation de la règle, donc l'endroit exact
   * où les deux se mettraient à diverger.
   */
  async function setSub(scope: 'site' | 'probe', id: string, enabled: boolean | null, key: string) {
    busy = key;
    error = '';
    let refused = '';
    try {
      await api.setSubscription(scope, id, enabled);
    } catch (e) {
      refused = handle(e);
    }
    // On relit même après un refus : le navigateur a déjà déplacé la case ou la
    // liste, et un contrôle qui affiche un choix que le hub n'a pas retenu est
    // pire qu'un refus visible.
    await load();
    if (refused) error = refused;
    busy = '';
  }

  /** `null` → « suit son site », `true` → toujours, `false` → jamais. */
  function excToValue(e: boolean | null): string {
    return e == null ? 'inherit' : e ? 'on' : 'off';
  }
  function valueToExc(v: string): boolean | null {
    return v === 'inherit' ? null : v === 'on';
  }
</script>

{#if loading}
  <StateBlock tone="loading" title={$_('common.loading')} />
{:else if !site}
  <StateBlock tone="error" title={$_('notify.error_title')} body={error}>
    <button class="lp-btn primary" onclick={load}>{$_('common.retry')}</button>
  </StateBlock>
{:else}
  {#if error}<p class="err" role="alert">{error}</p>{/if}

  <p class="lead">{$_('notify.site_panel_lead')}</p>

  <label class="switch big">
    <input
      type="checkbox"
      checked={site.enabled}
      disabled={!$canOperate || busy === `site:${siteId}`}
      onchange={(ev) =>
        setSub('site', siteId, (ev.currentTarget as HTMLInputElement).checked, `site:${siteId}`)}
    />
    <span>{$_('notify.site_alert')}</span>
  </label>

  {#if probes.length === 0}
    <p class="muted">{$_('notify.site_no_probe')}</p>
  {:else}
    <div class="probes">
      {#each probes as p (p.probe_id)}
        <div class="prow">
          <span class="pname">{p.name}</span>

          <!--
            Liste déroulante et non trois boutons : la phrase entière tient
            dans une liste à n'importe quelle largeur, et sur un site de
            quarante sondes quarante segments côte à côte feraient un mur.
          -->
          <select
            class="lp-input sel"
            value={excToValue(p.exception)}
            disabled={!$canOperate || busy === `probe:${p.probe_id}`}
            aria-label={$_('notify.probe_rule_for', { values: { name: p.name } })}
            onchange={(ev) =>
              setSub(
                'probe',
                p.probe_id,
                valueToExc((ev.currentTarget as HTMLSelectElement).value),
                `probe:${p.probe_id}`,
              )}
          >
            <option value="inherit">{$_('notify.rule_inherit')}</option>
            <option value="on">{$_('notify.rule_always')}</option>
            <option value="off">{$_('notify.rule_never')}</option>
          </select>

          <!--
            Ce qui s'applique VRAIMENT, tel que le hub l'a calculé. Sans cette
            colonne, « comme le site » n'apprend rien : il faudrait remonter au
            réglage du site pour savoir si la sonde alerte.
          -->
          <span class="eff" class:on={p.enabled}>
            {p.enabled ? $_('notify.eff_on') : $_('notify.eff_off')}
            {#if p.exception == null}
              <span class="from">{$_('notify.eff_from_site')}</span>
            {/if}
          </span>
        </div>
      {/each}
    </div>
  {/if}
{/if}

<style>
  .lead {
    margin: 0 0 0.9rem;
    font-size: 0.82rem;
    color: var(--lp-muted);
    line-height: 1.5;
  }
  .switch {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.86rem;
    cursor: pointer;
  }
  .switch.big {
    padding: 0.55rem 0.7rem;
    border: 1px solid var(--lp-border);
    border-radius: 8px;
    background: var(--lp-surface-2, transparent);
    font-weight: 500;
  }
  .probes {
    margin-top: 0.9rem;
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }
  .prow {
    display: grid;
    grid-template-columns: 1fr auto auto;
    align-items: center;
    gap: 0.6rem;
    padding: 0.4rem 0;
    border-top: 1px solid var(--lp-border);
  }
  .pname {
    font-size: 0.86rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sel {
    font-size: 0.8rem;
    padding: 0.25rem 0.4rem;
  }
  .eff {
    font-size: 0.74rem;
    color: var(--lp-muted);
    text-align: right;
    min-width: 6.5rem;
  }
  .eff.on {
    color: var(--lp-ok, #2f9e5f);
  }
  .from {
    display: block;
    font-size: 0.68rem;
    opacity: 0.75;
  }
  .muted {
    margin: 0.9rem 0 0;
    font-size: 0.82rem;
    color: var(--lp-muted);
  }
  .err {
    margin: 0 0 0.6rem;
    font-size: 0.82rem;
    color: var(--lp-danger, #c0392b);
  }
  /* Sur écran étroit la ligne à trois colonnes déborde : le nom prend toute
     la largeur et les deux contrôles passent dessous. */
  @media (max-width: 520px) {
    .prow {
      grid-template-columns: 1fr auto;
    }
    .pname {
      grid-column: 1 / -1;
    }
  }
</style>
