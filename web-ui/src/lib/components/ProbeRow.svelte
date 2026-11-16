<script lang="ts">
  import { _, locale } from 'svelte-i18n';
  import { platformLabel } from '$lib/format';
  import StatusMark from './StatusMark.svelte';
  import BufferBadge from './BufferBadge.svelte';
  import HeartbeatStrip from './HeartbeatStrip.svelte';
  import { relativeTime, absoluteTime, secondsLeft, countdown, elapsed, contactTime } from '$lib/time';
  import { probeDisplay } from '$lib/probe-state';
  import { isBehind } from '$lib/probe-version';
  import { go } from '$lib/router';
  import type { Beat } from '$lib/heartbeat';
  import type { Probe } from '$lib/api';

  interface Props {
    probe: Probe;
    /** Horloge partagée : le délai relatif doit vieillir sans rechargement. */
    now: number;
    /**
     * La version de référence : la dernière publiée quand le hub sait la lire,
     * la plus récente du parc sinon (voir `FleetView`).
     */
    newest?: string | null;
    /** Les 96 quarts d'heure de la bande de battement. */
    cells: Beat[];
    /** Latence passerelle en ms, `null` quand on ne sait pas. */
    latency: number | null;
    /** Perte 24 h de la passerelle en %, `null` quand on ne sait pas. */
    loss: number | null;
  }
  let { probe, now, newest = null, cells, latency, loss }: Props = $props();

  /** 🔴 Ni une version illisible ni une sonde EN AVANCE ne sont « en retard » :
      colorer sur un doute enverrait mettre à jour une machine qui n'en a pas
      besoin — et sur un parc de client, c'est un déplacement. */
  const behind = $derived(isBehind(probe.version, newest));

  const lang = $derived($locale ?? 'en');

  /**
   * L'état affiché, recalculé à chaque battement de l'horloge partagée.
   *
   * ⚠️ On n'affiche PAS `probe.status` : il est daté du dernier chargement, et
   * son « en ligne » couvre trois battements manqués.
   */
  const shown = $derived(probeDisplay(probe, now));
  const since = $derived(
    shown.since != null ? elapsed(Math.max(0, now / 1000 - shown.since), lang) : '',
  );

  /**
   * Temps restant de mode temps réel, `0` quand il ne tourne pas.
   *
   * ⚠️ C'est le garde-fou contre l'oubli : ce qui tourne doit se voir depuis le
   * parc, avec sa fin.
   */
  const realtimeLeft = $derived(secondsLeft(probe.realtime_until, now));

  /** Une sonde muette n'a plus de mesure fraîche : sa dernière latence est figée. */
  const fresh = $derived(shown.state === 'online' || shown.state === 'degraded');

  const href = $derived(`#/probes/${encodeURIComponent(probe.probe_id)}`);
  const one = $derived(
    new Intl.NumberFormat(lang, { minimumFractionDigits: 1, maximumFractionDigits: 1 }),
  );

  /** Ce que dit la bande, en mots. */
  const stripLabel = $derived.by(() => {
    const bad = cells.filter((c) => c === 'b' || c === 'w').length;
    return bad
      ? $_('fleet.strip_aria_bad', { values: { name: probe.name, n: bad } })
      : $_('fleet.strip_aria_ok', { values: { name: probe.name } });
  });

  /** La ligne entière s'ouvre, sauf quand on vise un contrôle qu'elle porte. */
  function open(e: MouseEvent) {
    if ((e.target as HTMLElement).closest('a, button')) return;
    if (e.ctrlKey || e.metaKey) window.open(href, '_blank');
    else go(href);
  }
</script>

<tr
  class:trouble={shown.state === 'offline'}
  class:degr={shown.state === 'degraded' || shown.state === 'silent'}
  onclick={open}
>
  <td>
    <a class="nm" {href}>{probe.name}</a>
    <span class="sub2">{platformLabel(probe.platform, $_('common.none'))}</span>
  </td>

  <td class="status">
    <StatusMark status={shown.state} />
    {#if shown.state === 'degraded' && shown.internet}
      <span class="since">
        {since
          ? $_('status.degraded_since', {
              values: { net: $_(`status.net_${shown.internet}`), when: since },
            })
          : $_(`status.net_${shown.internet}`)}
      </span>
    {:else if shown.state === 'never'}
      <span class="since">
        {$_('status.enrolled_at', { values: { when: contactTime(shown.since, lang, now) } })}
      </span>
    {:else if since}
      <span class="since" title={absoluteTime(probe.last_seen, lang)}>
        {$_('status.since', { values: { when: since } })}
      </span>
    {/if}
    {#if realtimeLeft > 0}
      <span
        class="lp-armed"
        title={$_('fleet.realtime_help', { values: { left: countdown(realtimeLeft, lang) } })}
      >
        <svg width="11" height="11" viewBox="0 0 12 12" aria-hidden="true"><path d="M6.8 1 2.5 7h3l-.6 4L9.5 5h-3z" fill="currentColor" /></svg>
        {$_('fleet.realtime_flag', { values: { left: countdown(realtimeLeft, lang) } })}
      </span>
    {/if}
    {#if probe.buffered_points > 0}
      <BufferBadge points={probe.buffered_points} live={shown.state === 'online' || shown.state === 'degraded'} />
    {/if}
  </td>

  <td class="opt-s"><HeartbeatStrip {cells} label={stripLabel} /></td>

  <td class="num opt">
    {#if fresh && latency != null}{one.format(latency)}<span class="lp-u">ms</span>{/if}
  </td>
  <td class="num opt">
    {#if fresh && loss != null}{loss === 0 ? '0' : one.format(loss)}<span class="lp-u">%</span>{/if}
  </td>

  <td class="num" title={probe.last_seen ? absoluteTime(probe.last_seen, lang) : $_('status.never_seen')}>
    {#if probe.last_seen == null}
      {$_('fleet.never')}
    {:else if shown.state === 'online' || shown.state === 'degraded'}
      {relativeTime(probe.last_seen, lang, now)}
    {:else}
      {contactTime(probe.last_seen, lang, now)}
    {/if}
  </td>

  <td class="opt-l"><span class="lp-mono ip">{probe.public_ip || ''}</span></td>

  <td class="num">
    {probe.version || ''}
    {#if behind}
      <span class="lp-tag upd" title={$_('fleet.version_behind', { values: { version: newest } })}>
        {$_('fleet.update_available')}
      </span>
    {/if}
  </td>

  <td class="act" aria-hidden="true">
    <svg class="go" width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><polyline points="6,3 11,8 6,13" /></svg>
  </td>
</tr>

<style>
  tr {
    cursor: pointer;
  }
  td:first-child {
    white-space: nowrap;
  }
  .status {
    white-space: nowrap;
  }
  .status > :global(*) {
    vertical-align: middle;
  }
  .status > :global(* + *) {
    margin-left: 8px;
  }
  .since {
    color: var(--color-muted);
    font-size: 0.8125rem;
  }
  .ip {
    font-size: 0.8125rem;
    color: var(--color-ink-2);
  }
  .lp-tag.upd {
    margin-left: 4px;
  }
  .go {
    color: var(--color-muted);
    vertical-align: middle;
  }
  tr.trouble td:first-child {
    box-shadow: inset 2px 0 0 var(--st-bad);
  }
  tr.degr td:first-child {
    box-shadow: inset 2px 0 0 var(--st-warn);
  }
  @media (max-width: 90rem) {
    .opt-l {
      display: none;
    }
  }
  @media (max-width: 48rem) {
    .opt,
    .opt-s {
      display: none;
    }
  }
</style>
