<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { _, locale } from 'svelte-i18n';
  import { fleet } from '$lib/fleet';
  import { flash } from '$lib/flash';
  import { go } from '$lib/router';
  import { now, relativeTime, absoluteTime, dateOnly, tooltipTime } from '$lib/time';
  import { api, ApiError } from '$lib/api';
  import {
    MEASUREMENT,
    RANGES,
    DEFAULT_RANGE,
    MetricsShapeError,
    normalizeCurves,
    numeric,
    type Range,
    type Curve,
  } from '$lib/metrics';
  import { SERIES, seriesColor, normalizeState } from '$lib/charts';
  import StatusMark from '$lib/components/StatusMark.svelte';
  import BufferBadge from '$lib/components/BufferBadge.svelte';
  import ChartCard from '$lib/components/ChartCard.svelte';
  import TimeSeriesChart from '$lib/components/TimeSeriesChart.svelte';
  import StatusBand from '$lib/components/StatusBand.svelte';
  import ValueTable from '$lib/components/ValueTable.svelte';
  import StateBlock from '$lib/components/StateBlock.svelte';
  import Modal from '$lib/components/Modal.svelte';
  import ActionMenu from '$lib/components/ActionMenu.svelte';
  import CodeDisplay from '$lib/components/CodeDisplay.svelte';
  import { enrollments } from '$lib/enroll';
  import type { EnrollCode } from '$lib/api';

  const { id, onExpired } = $props<{ id: string; onExpired: () => void }>();

  const lang = $derived($locale ?? 'en');
  const probe = $derived($fleet.probes.find((p) => p.probe_id === id) ?? null);

  onMount(() => {
    // Accès direct par URL : le parc n'est peut-être pas encore chargé.
    if ($fleet.probes.length === 0) void fleet.load(onExpired);
  });

  // ── Renommage en place ───────────────────────────────────────────────────
  let editing = $state(false);
  let draft = $state('');
  let nameBusy = $state(false);
  let nameError = $state('');

  function startEdit() {
    draft = probe?.name ?? '';
    nameError = '';
    editing = true;
  }

  async function saveName() {
    if (!probe) return;
    const name = draft.trim();
    if (!name) return void (nameError = $_('probe.rename_empty'));
    if (name === probe.name) return void (editing = false);
    nameBusy = true;
    nameError = '';
    try {
      await api.patchProbe(probe.probe_id, { name });
      fleet.patchLocal({ ...probe, name });
      editing = false;
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      nameError =
        e instanceof ApiError && e.isConflict
          ? $_('probe.rename_conflict', { values: { name, site: probe.site } })
          : e instanceof ApiError
            ? e.message
            : String(e);
    } finally {
      nameBusy = false;
    }
  }

  // ── Déplacement de site ──────────────────────────────────────────────────
  let moveOpen = $state(false);
  let moveTo = $state('');
  let moveBusy = $state(false);
  let moveError = $state('');

  function openMove() {
    moveTo = probe?.site_id ?? '';
    moveError = '';
    moveOpen = true;
  }

  async function submitMove() {
    if (!probe || !moveTo || moveTo === probe.site_id) return void (moveOpen = false);
    moveBusy = true;
    moveError = '';
    const target = $fleet.sites.find((s) => s.site_id === moveTo);
    try {
      await api.patchProbe(probe.probe_id, { site_id: moveTo });
      moveOpen = false;
      await fleet.load(onExpired, { quiet: true });
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      moveError =
        e instanceof ApiError && e.isConflict
          ? $_('probe.move_conflict', {
              values: { site: target?.name ?? '', name: probe.name },
            })
          : e instanceof ApiError
            ? e.message
            : String(e);
    } finally {
      moveBusy = false;
    }
  }

  // ── Révocation ───────────────────────────────────────────────────────────
  let revokeOpen = $state(false);
  let revokeBusy = $state(false);
  let revokeError = $state('');

  async function submitRevoke() {
    if (!probe) return;
    revokeBusy = true;
    revokeError = '';
    try {
      await api.revoke(probe.probe_id);
      flash.set($_('revoke.done', { values: { name: probe.name } }));
      fleet.dropLocal(probe.probe_id);
      revokeOpen = false;
      go('/');
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      revokeError = e instanceof ApiError ? e.message : String(e);
    } finally {
      revokeBusy = false;
    }
  }

  // ── Clé d'écriture : trois gestes, trois conséquences ────────────────────
  const hubUrl = `${location.protocol}//${location.host}`;

  // Rotation d'hygiène : sans conséquence immédiate, donc sans confirmation.
  // Le drapeau local rend l'état intermédiaire visible tout de suite, même si
  // `GET /api/probes` ne le renvoie pas encore.
  let rotateBusy = $state(false);
  let rotateDone = $state(false);
  let keyError = $state('');

  const pending = $derived(rotateDone || probe?.token_rotation_pending === true);

  async function rotate() {
    if (!probe) return;
    rotateBusy = true;
    keyError = '';
    try {
      await api.rotateToken(probe.probe_id);
      rotateDone = true;
      await fleet.load(onExpired, { quiet: true });
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      keyError = e instanceof ApiError ? e.message : String(e);
    } finally {
      rotateBusy = false;
    }
  }

  // Révocation immédiate : c'est la seule des trois qui coupe une sonde, donc
  // la seule à demander une confirmation. Tout confirmer reviendrait à ne
  // rien faire lire.
  let killOpen = $state(false);
  let killBusy = $state(false);
  let killDone = $state('');

  async function killToken() {
    if (!probe) return;
    killBusy = true;
    keyError = '';
    try {
      await api.revokeToken(probe.probe_id);
      killOpen = false;
      killDone = $_('probe.revoke_now_done');
      await fleet.load(onExpired, { quiet: true });
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      keyError = e instanceof ApiError ? e.message : String(e);
      killOpen = false;
    } finally {
      killBusy = false;
    }
  }

  // Ré-enrôlement : émet un code, ne casse rien — pas de confirmation.
  let reOpen = $state(false);
  let reBusy = $state(false);
  let reCode = $state<EnrollCode | null>(null);
  let reError = $state('');

  async function reenroll() {
    if (!probe) return;
    reBusy = true;
    reError = '';
    reOpen = true;
    try {
      reCode = await api.reenrollCode(probe.probe_id);
      // La même attente s'affiche aussi dans le parc, sur la ligne du site :
      // sans ça, on quitterait cet écran et le code paraîtrait perdu.
      enrollments.remember(
        {
          site_id: probe.site_id,
          site: probe.site,
          probe_id: probe.probe_id,
          probe: probe.name,
          created_at: Math.floor(Date.now() / 1000),
          expires_at: reCode.expires_at,
        },
        reCode.code,
      );
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) return onExpired();
      reError = e instanceof ApiError ? e.message : String(e);
    } finally {
      reBusy = false;
    }
  }

  // ── Mesures ──────────────────────────────────────────────────────────────
  interface ChartState {
    tone: 'loading' | 'error' | 'empty' | 'ready';
    error?: string;
    curves: Curve[];
  }
  const blank: ChartState = { tone: 'loading', curves: [] };

  let range = $state<Range>(DEFAULT_RANGE);
  let ping = $state<ChartState>({ ...blank });
  let inet = $state<ChartState>({ ...blank });
  let speed = $state<ChartState>({ ...blank });
  let refreshing = $state(false);

  async function fetchOne(measurement: string, fallbackField: string): Promise<ChartState> {
    try {
      const raw = await api.metrics(id, measurement, range);
      const curves = normalizeCurves(raw, fallbackField);
      const empty = curves.every((c) => c.points.length === 0);
      return { tone: empty ? 'empty' : 'ready', curves };
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized) {
        onExpired();
        return { tone: 'error', curves: [], error: $_('auth.expired') };
      }
      if (e instanceof MetricsShapeError) {
        // On nomme les clés reçues : le message doit permettre de corriger le
        // backend, pas seulement de constater l'échec.
        return {
          tone: 'error',
          curves: [],
          error: $_('charts.error_shape', { values: { keys: e.observed } }),
        };
      }
      return {
        tone: 'error',
        curves: [],
        error: e instanceof ApiError ? e.message : String(e),
      };
    }
  }

  async function loadMetrics() {
    const hadData = ping.tone === 'ready' || inet.tone === 'ready' || speed.tone === 'ready';
    refreshing = hadData;
    if (!hadData) {
      ping = { ...blank };
      inet = { ...blank };
      speed = { ...blank };
    }
    const [p, i, s] = await Promise.all([
      fetchOne(MEASUREMENT.ping, 'latency_ms'),
      fetchOne(MEASUREMENT.internet, 'state'),
      fetchOne(MEASUREMENT.speedtest, 'download_mbps'),
    ]);
    ping = p;
    inet = i;
    speed = s;
    refreshing = false;
  }

  // Recharge à chaque changement de fenêtre ou de sonde — la barre de filtres
  // cadre les trois graphiques à la fois, jamais un seul.
  // `untrack` est indispensable : loadMetrics lit ping/inet/speed pour savoir
  // s'il doit garder le rendu précédent, et les réécrit ensuite. Sans lui,
  // l'effet se redéclencherait sur sa propre écriture.
  $effect(() => {
    void range;
    void id;
    untrack(() => void loadMetrics());
  });

  /** Points d'un champ donné, toutes étiquettes confondues. */
  function pointsOf(state: ChartState, field: string) {
    return numeric(state.curves.find((c) => c.field === field)?.points);
  }

  // Une courbe par cible pinguée : le hub les sépare déjà, les rassembler
  // dessinerait une latence moyenne qu'aucune machine n'a jamais mesurée.
  const pingCurves = $derived(
    ping.curves
      .filter((c) => c.field === 'latency_ms' && c.points.length > 0)
      .map((c, i) => ({
        key: c.label,
        // Une seule cible : pas la peine d'afficher son adresse en légende.
        label: ping.curves.filter((o) => o.field === 'latency_ms').length > 1
          ? c.label.replace(/^latency_ms\s*·\s*/, '')
          : $_('charts.latency'),
        color: seriesColor(i),
        points: numeric(c.points),
      })),
  );
  const pingPoints = $derived(pingCurves.flatMap((c) => c.points));
  const inetPoints = $derived(inet.curves.find((c) => c.field === 'state')?.points ?? []);
  const dlPoints = $derived(pointsOf(speed, 'download_mbps'));
  const ulPoints = $derived(pointsOf(speed, 'upload_mbps'));

  const speedTone = $derived(
    speed.tone === 'ready' && dlPoints.length === 0 && ulPoints.length === 0 ? 'empty' : speed.tone,
  );
  const pingTone = $derived(
    ping.tone === 'ready' && pingPoints.length === 0 ? 'empty' : ping.tone,
  );
  const inetTone = $derived(
    inet.tone === 'ready' && inetPoints.length === 0 ? 'empty' : inet.tone,
  );

  const speedLegend = $derived([
    { label: $_('charts.download'), color: SERIES.one },
    { label: $_('charts.upload'), color: SERIES.two },
  ]);
</script>

<a class="back" href="#/">
  <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
    <polyline points="10,3 5,8 10,13" />
  </svg>
  {$_('probe.back')}
</a>

{#if !probe}
  {#if $fleet.loading}
    <StateBlock tone="loading" title={$_('fleet.loading')} />
  {:else}
    <StateBlock
      tone="error"
      title={$_('probe.not_found_title')}
      body={$_('probe.not_found_body')}
    >
      <button class="lp-btn primary" onclick={() => go('/')}>{$_('probe.back')}</button>
    </StateBlock>
  {/if}
{:else}
  <header class="head">
    <div class="ident">
      {#if editing}
        <div class="edit">
          <input
            class="lp-input"
            bind:value={draft}
            aria-label={$_('probe.rename_label')}
            onkeydown={(e) => {
              if (e.key === 'Enter') saveName();
              if (e.key === 'Escape') editing = false;
            }}
          />
          <button class="lp-btn primary sm" onclick={saveName} disabled={nameBusy}>
            {$_('common.save')}
          </button>
          <button class="lp-btn sm" onclick={() => (editing = false)}>{$_('common.cancel')}</button>
        </div>
        {#if nameError}<p class="err" role="alert">{nameError}</p>{/if}
      {:else}
        <div class="title-row">
          <h1>{probe.name}</h1>
          <button class="lp-btn ghost sm" onclick={startEdit}>{$_('probe.rename')}</button>
        </div>
      {/if}

      <div class="badges">
        <StatusMark status={probe.status} />
        <span class="dot" aria-hidden="true">·</span>
        <span class="site">{probe.site}</span>
        <span class="dot" aria-hidden="true">·</span>
        <span
          class="seen lp-mono"
          title={probe.last_seen ? absoluteTime(probe.last_seen, lang) : $_('status.never_seen')}
        >
          {probe.last_seen ? relativeTime(probe.last_seen, lang, $now) : $_('status.never_seen')}
        </span>
      </div>
    </div>

    <!--
      Toutes les actions de la fiche sont derrière un seul engrenage. Aucune
      n'est courante — on vient ici pour lire des courbes, pas pour révoquer une
      clé — et posées en clair dans la page elles étaient atteignables par un
      simple défilement. Les explications ne disparaissent pas : chaque item
      porte la sienne, en toutes lettres, avant le clic.
    -->
    <div class="acts">
      <ActionMenu label={$_('probe.actions')}>
        {#snippet children(close)}
          <p class="mgroup">{$_('probe.menu_probe')}</p>
          <button
            class="mi"
            role="menuitem"
            onclick={() => {
              close();
              openMove();
            }}
          >
            <span class="mi-label">{$_('probe.move')}</span>
            <span class="mi-hint">{$_('probe.move_hint')}</span>
          </button>

          <p class="mgroup">{$_('probe.keys_title')}</p>
          <p class="mintro">{$_('probe.keys_intro')}</p>

          <button
            class="mi"
            role="menuitem"
            disabled={rotateBusy}
            onclick={() => {
              close();
              void rotate();
            }}
          >
            <span class="mi-label">{rotateBusy ? $_('probe.rotating') : $_('probe.rotate')}</span>
            <span class="mi-hint">{$_('probe.rotate_hint')}</span>
          </button>

          <button
            class="mi accent"
            role="menuitem"
            disabled={reBusy}
            onclick={() => {
              close();
              void reenroll();
            }}
          >
            <span class="mi-label">
              {reBusy ? $_('probe.reenroll_working') : $_('probe.reenroll')}
            </span>
            <span class="mi-hint">{$_('probe.reenroll_hint')}</span>
          </button>

          <div class="mdanger">
            <button
              class="mi danger"
              role="menuitem"
              onclick={() => {
                close();
                killOpen = true;
              }}
            >
              <span class="mi-label">{$_('probe.revoke_now')}</span>
              <span class="mi-hint">{$_('probe.revoke_now_hint')}</span>
            </button>

            <button
              class="mi danger"
              role="menuitem"
              onclick={() => {
                close();
                revokeOpen = true;
              }}
            >
              <span class="mi-label">{$_('revoke.action')}</span>
              <span class="mi-hint">{$_('revoke.keeps_data')}</span>
            </button>
          </div>
        {/snippet}
      </ActionMenu>
    </div>
  </header>

  <!--
    Ce que les actions du menu ont laissé derrière elles. C'est un état de la
    sonde, pas une commande : sa place est près de l'identité, avec l'alerte de
    tampon, et non dans un bloc d'administration en bas de page.
  -->
  {#if pending}
    <div class="pending" role="status">
      <strong>{$_('probe.pending_title')}</strong>
      <p>{$_('probe.pending_body')}</p>
    </div>
  {/if}
  {#if killDone}
    <p class="done" role="status">{killDone}</p>
  {/if}
  {#if keyError}
    <p class="err top" role="alert">{keyError}</p>
  {/if}

  <dl class="meta">
    <div><dt>{$_('probe.meta_id')}</dt><dd class="lp-mono id">{probe.probe_id}</dd></div>
    <div><dt>{$_('probe.meta_site')}</dt><dd>{probe.site}</dd></div>
    <div><dt>{$_('probe.meta_platform')}</dt><dd class="lp-mono">{probe.platform || '—'}</dd></div>
    <div><dt>{$_('probe.meta_version')}</dt><dd class="lp-mono">{probe.version || '—'}</dd></div>
    <div>
      <dt>{$_('probe.meta_created')}</dt>
      <dd class="lp-mono">{dateOnly(probe.created_at, lang)}</dd>
    </div>
    <div>
      <dt>{$_('buffer.label')}</dt>
      <dd><BufferBadge points={probe.buffered_points} /></dd>
    </div>
  </dl>

  {#if probe.buffered_points > 0}
    <div class="buffer-alert" role="status">
      <strong>{$_('buffer.pending', { values: { n: probe.buffered_points } })}</strong>
      <p>{$_('buffer.warn_body')}</p>
    </div>
  {/if}

  <div class="rangebar">
    <span class="lp-label">{$_('probe.range')}</span>
    <div class="chips" role="group" aria-label={$_('probe.range')}>
      {#each RANGES as r (r)}
        <button class="chip" class:on={range === r} onclick={() => (range = r)}>
          {$_(`probe.range_${r.replace('-', '')}`)}
        </button>
      {/each}
    </div>
  </div>

  <div class="charts">
    <ChartCard
      title={$_('charts.latency_title')}
      sub={$_('charts.latency_sub')}
      tone={pingTone}
      errorText={ping.error}
      {refreshing}
    >
      <TimeSeriesChart
        series={pingCurves}
        unit={$_('charts.unit_ms')}
      />
      {#snippet table()}
        <ValueTable
          columns={[$_('charts.table_time'), `${$_('charts.latency')} (${$_('charts.unit_ms')})`]}
          rows={pingPoints.map((p) => [tooltipTime(p.t, lang), p.v.toFixed(0)])}
        />
      {/snippet}
    </ChartCard>

    <ChartCard
      title={$_('charts.internet_title')}
      sub={$_('charts.internet_sub')}
      tone={inetTone}
      errorText={inet.error}
      {refreshing}
    >
      <StatusBand points={inetPoints} />
      {#snippet table()}
        <ValueTable
          columns={[$_('charts.table_time'), $_('charts.table_state')]}
          rows={inetPoints.map((p) => [
            tooltipTime(p.t, lang),
            $_(`charts.internet_${normalizeState(p.v)}`),
          ])}
        />
      {/snippet}
    </ChartCard>

    <ChartCard
      title={$_('charts.speed_title')}
      sub={$_('charts.speed_sub')}
      tone={speedTone}
      errorText={speed.error}
      legend={speedLegend}
      {refreshing}
    >
      <TimeSeriesChart
        series={[
          { key: 'dl', label: $_('charts.download'), color: SERIES.one, points: dlPoints },
          { key: 'ul', label: $_('charts.upload'), color: SERIES.two, points: ulPoints },
        ]}
        unit={$_('charts.unit_mbps')}
        decimals={1}
      />
      {#snippet table()}
        <ValueTable
          columns={[
            $_('charts.table_time'),
            `${$_('charts.download')} (${$_('charts.unit_mbps')})`,
            `${$_('charts.upload')} (${$_('charts.unit_mbps')})`,
          ]}
          rows={dlPoints.map((p, i) => [
            tooltipTime(p.t, lang),
            p.v.toFixed(1),
            ulPoints[i] ? ulPoints[i].v.toFixed(1) : '—',
          ])}
        />
      {/snippet}
    </ChartCard>
  </div>

  <Modal
    open={killOpen}
    title={$_('probe.revoke_now_title', { values: { name: probe.name } })}
    onclose={() => (killOpen = false)}
  >
    <!--
      Le tri « fuite de clé ou machine compromise ? » est posé ici et pas
      ailleurs : c'est le dernier instant où les deux issues sont encore
      ouvertes. Quelqu'un qui reconnaît le second cas annule et ré-enrôle.
    -->
    <div class="triage">
      <h3>{$_('probe.which_title')}</h3>
      <ul>
        <li>{$_('probe.which_leak')}</li>
        <li>{$_('probe.which_host')}</li>
      </ul>
    </div>
    <p>{$_('probe.revoke_now_body')}</p>
    <p class="keep"><strong>{$_('probe.revoke_now_keeps')}</strong></p>
    {#snippet footer()}
      <button class="lp-btn" onclick={() => (killOpen = false)}>{$_('common.cancel')}</button>
      <button class="lp-btn danger" onclick={killToken} disabled={killBusy}>
        {killBusy ? $_('probe.revoke_now_working') : $_('probe.revoke_now_confirm')}
      </button>
    {/snippet}
  </Modal>

  <Modal
    open={reOpen}
    wide
    title={$_('probe.reenroll_title', { values: { name: probe.name } })}
    onclose={() => (reOpen = false)}
  >
    <p class="keep"><strong>{$_('probe.reenroll_keeps')}</strong></p>
    {#if reError}
      <p class="err" role="alert">{reError}</p>
    {:else if reCode}
      <CodeDisplay
        code={reCode.code}
        expiresAt={reCode.expires_at}
        {hubUrl}
        dictate={$_('code.dictate_reenroll')}
        onregenerate={reenroll}
        busy={reBusy}
        regenerateLabel={$_('code.regenerate')}
      />
    {:else}
      <p class="muted">{$_('probe.reenroll_working')}</p>
    {/if}
    {#snippet footer()}
      <button class="lp-btn" onclick={() => (reOpen = false)}>{$_('common.close')}</button>
    {/snippet}
  </Modal>

  <Modal
    open={moveOpen}
    title={$_('probe.move_title', { values: { name: probe.name } })}
    onclose={() => (moveOpen = false)}
  >
    <label class="lp-field">
      {$_('probe.move_label')}
      <select bind:value={moveTo}>
        {#each $fleet.sites as s (s.site_id)}
          <option value={s.site_id}>{s.name}</option>
        {/each}
      </select>
    </label>
    <p>{$_('probe.move_hint')}</p>
    {#if moveError}<p class="err" role="alert">{moveError}</p>{/if}
    {#snippet footer()}
      <button class="lp-btn" onclick={() => (moveOpen = false)}>{$_('common.cancel')}</button>
      <button class="lp-btn primary" onclick={submitMove} disabled={moveBusy}>
        {moveBusy ? $_('probe.moving') : $_('probe.move_submit')}
      </button>
    {/snippet}
  </Modal>

  <Modal
    open={revokeOpen}
    title={$_('revoke.title', { values: { name: probe.name } })}
    onclose={() => (revokeOpen = false)}
  >
    <!--
      La phrase qui compte est la première et elle est en évidence : une
      confirmation qui laisse planer un doute sur l'historique fait renoncer au
      ménage, et le parc se remplit de sondes mortes.
    -->
    <p class="keep"><strong>{$_('revoke.keeps_data')}</strong></p>
    <p>{$_('revoke.body')}</p>
    <p class="muted">{$_('revoke.rejoin')}</p>
    {#if revokeError}<p class="err" role="alert">{revokeError}</p>{/if}
    {#snippet footer()}
      <button class="lp-btn" onclick={() => (revokeOpen = false)}>{$_('common.cancel')}</button>
      <button class="lp-btn danger" onclick={submitRevoke} disabled={revokeBusy}>
        {revokeBusy ? $_('revoke.working') : $_('revoke.confirm')}
      </button>
    {/snippet}
  </Modal>
{/if}

<style>
  .back {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-size: 12px;
    color: var(--ep-text-secondary);
    text-decoration: none;
    margin-bottom: 12px;
  }
  .back:hover {
    color: var(--ep-accent-bright);
  }

  .head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 14px;
    flex-wrap: wrap;
    margin-bottom: 14px;
  }
  .ident {
    min-width: 0;
    flex: 1 1 260px;
  }
  .title-row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  h1 {
    font-size: 21px;
    font-weight: 800;
    letter-spacing: -0.02em;
    margin: 0;
    overflow-wrap: anywhere;
  }
  .edit {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .edit input {
    flex: 1 1 200px;
    font-family: var(--ep-font-sans);
    font-size: 15px;
    font-weight: 700;
  }
  .badges {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    margin-top: 7px;
    font-size: 11.5px;
  }
  .site {
    color: var(--ep-text-secondary);
    font-weight: 600;
  }
  .seen {
    color: var(--ep-text-muted);
    font-size: 11px;
  }
  .dot {
    color: var(--ep-text-dim);
  }
  .acts {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }

  .meta {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: 6px;
    margin: 0 0 14px;
  }
  .meta div {
    background: var(--ep-bg-secondary);
    border: 1px solid var(--ep-border);
    border-radius: var(--ep-radius-md);
    padding: 9px 11px;
    min-width: 0;
  }
  dt {
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.8px;
    color: var(--ep-text-dim);
    margin-bottom: 3px;
  }
  dd {
    margin: 0;
    font-size: 12px;
    color: var(--ep-text-primary);
  }
  /* Un UUID ne doit jamais élargir la page : il se coupe, il ne pousse pas. */
  dd.id {
    font-size: 10.5px;
    color: var(--ep-text-secondary);
    overflow-wrap: anywhere;
  }

  .buffer-alert {
    padding: 11px 13px;
    margin-bottom: 14px;
    border: 1px solid color-mix(in srgb, var(--ep-warning) 50%, var(--ep-border));
    border-left-width: 3px;
    border-radius: var(--ep-radius-md);
    background: color-mix(in srgb, var(--ep-warning) 8%, transparent);
  }
  .buffer-alert strong {
    font-size: 12.5px;
  }
  .buffer-alert p {
    font-size: 11.5px;
    color: var(--ep-text-secondary);
    margin: 4px 0 0;
    line-height: 1.55;
    max-width: 82ch;
  }

  .rangebar {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 12px;
    flex-wrap: wrap;
  }
  .chips {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }
  .chip {
    padding: 5px 11px;
    border-radius: 999px;
    border: 1px solid var(--ep-border);
    background: var(--ep-bg-secondary);
    color: var(--ep-text-secondary);
    font-family: var(--ep-font-sans);
    font-size: 11.5px;
    cursor: pointer;
  }
  .chip.on {
    border-color: var(--ep-accent);
    background: var(--ep-accent-dim);
    color: var(--ep-accent-bright);
    font-weight: 600;
  }

  .charts {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  /* ── Contenu du menu d'actions ─────────────────────────────────────────────
     Chaque item porte son libellé ET sa conséquence : le texte n'a pas été
     raccourci en passant dans le menu, il a été rapproché du geste. Les trois
     familles ne se ressemblent pas — neutre, accent, rouge — pour qu'un clic
     mal visé ne coupe pas une sonde. */
  .mgroup {
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.8px;
    color: var(--ep-text-dim);
    margin: 8px 8px 4px;
  }
  .mgroup:first-child {
    margin-top: 4px;
  }
  .mintro {
    font-size: 11px;
    line-height: 1.55;
    color: var(--ep-text-muted);
    margin: 0 8px 6px;
  }
  .mi {
    display: flex;
    flex-direction: column;
    gap: 3px;
    width: 100%;
    text-align: left;
    padding: 8px 10px;
    border: 1px solid transparent;
    border-left: 2px solid var(--ep-border-strong);
    border-radius: var(--ep-radius-md);
    background: transparent;
    font-family: var(--ep-font-sans);
    cursor: pointer;
  }
  .mi + .mi {
    margin-top: 2px;
  }
  .mi:hover:not(:disabled) {
    background: var(--ep-glass-bg-md);
  }
  .mi:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }
  .mi-label {
    font-size: 12.5px;
    font-weight: 600;
    color: var(--ep-text-primary);
  }
  .mi-hint {
    font-size: 11px;
    line-height: 1.5;
    color: var(--ep-text-muted);
  }
  .mi.accent {
    border-left-color: var(--ep-accent);
  }
  .mi.accent .mi-label {
    color: var(--ep-accent-bright);
  }
  .mi.danger {
    border-left-color: var(--ep-danger);
  }
  .mi.danger .mi-label {
    color: var(--ep-danger);
  }
  /* Les deux gestes qui coupent sont séparés par un trait : le pouce ne glisse
     pas d'une rotation de clé à une révocation. */
  .mdanger {
    margin-top: 8px;
    padding-top: 8px;
    border-top: 1px solid color-mix(in srgb, var(--ep-danger) 35%, var(--ep-border));
  }

  .triage {
    border: 1px solid color-mix(in srgb, var(--ep-danger) 40%, var(--ep-border));
    border-radius: var(--ep-radius-md);
    padding: 11px 13px;
  }
  .triage h3 {
    font-size: 12.5px;
    font-weight: 700;
    margin: 0 0 6px;
    color: var(--ep-danger);
  }
  .triage ul {
    margin: 0;
    padding-left: 18px;
    font-size: 11.5px;
    line-height: 1.6;
    color: var(--ep-text-secondary);
  }
  .triage li + li {
    margin-top: 4px;
  }

  .pending {
    margin-bottom: 14px;
    border: 1px solid color-mix(in srgb, var(--ep-info) 50%, var(--ep-border));
    border-left-width: 3px;
    border-radius: var(--ep-radius-md);
    background: color-mix(in srgb, var(--ep-info) 8%, transparent);
    padding: 11px 13px;
  }
  .pending strong {
    font-size: 12.5px;
    color: var(--ep-text-primary);
  }
  .pending p {
    font-size: 11.5px;
    color: var(--ep-text-secondary);
    margin: 4px 0 0;
    line-height: 1.6;
    max-width: 82ch;
  }
  .done {
    font-size: 12px;
    color: var(--ep-success);
    margin: 0 0 14px;
  }

  .err {
    font-size: 12px;
    color: var(--ep-danger);
    margin: 6px 0 0;
    border-left: 2px solid var(--ep-danger);
    padding-left: 8px;
  }
  .err.top {
    margin: 0 0 14px;
  }
  .keep {
    color: var(--ep-text-primary);
    border-left: 2px solid var(--ep-success);
    padding-left: 10px;
  }
  .muted {
    color: var(--ep-text-muted);
    font-size: 12px;
  }
</style>
