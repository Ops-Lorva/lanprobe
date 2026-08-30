<script lang="ts">
  import { onMount } from 'svelte';
  import { pushNow, restoreFromHub } from '../hubConfigBackup';
  import { invoke } from '@tauri-apps/api/core';
  import { _ } from 'svelte-i18n';
  import { settings, type Theme, type Layout, type Lang, type SpeedtestEngine, type Palette } from '../stores/settings';
  import { portscanProfiles, type PortScanProfile } from '../stores/portscanProfiles';
  import { profiles, type Profile } from '../stores/profiles';
  import { get } from 'svelte/store';
  import { selectedInterface } from '../stores/selectedInterface';
  import { scheduler } from '../stores/scheduler';

  // ── Rattachement à un hub ───────────────────────────────────────────────
  let hub = $state<{ enrolled: boolean; url: string; name: string; site: string }>({
    enrolled: false, url: '', name: '', site: '',
  });
  let hubUrl = $state('');
  /** Nombre de réglages rendus par le hub au dernier enrôlement. */
  let hubRestored = $state(0);

  // ── Adresse du hub : schéma séparé de l'hôte ─────────────────────────────
  //
  // Taper « https:// » au clavier est pénible et se rate — et une adresse
  // fausse donne un enrôlement qui échoue sans dire pourquoi. Le schéma est
  // donc un bouton à deux positions, l'hôte un champ de texte simple.
  let scheme = $state<'https://' | 'http://'>('https://');
  let hubHost = $state('');

  function toggleScheme() {
    scheme = scheme === 'https://' ? 'http://' : 'https://';
  }

  // Coller une URL complète dans le champ d'hôte est le geste naturel : on
  // en extrait le schéma plutôt que de produire « https://https://… ».
  $effect(() => {
    const match = /^(https?:\/\/)(.*)$/i.exec(hubHost);
    if (match) {
      scheme = match[1].toLowerCase() as 'https://' | 'http://';
      hubHost = match[2];
    }
  });

  $effect(() => {
    hubUrl = hubHost.trim() ? `${scheme}${hubHost.trim()}` : '';
  });
  let hubName = $state('');
  let hubCode = $state('');

  /**
   * Met le code en forme pendant la frappe : majuscules et tiret posé tout
   * seul. Le code est dicté au téléphone — on le tape donc souvent en
   * minuscules, ou sans le tiret. Le refuser pour ça serait absurde quand la
   * correction est mécanique.
   */
  function formatCode(raw: string): string {
    const clean = raw.toUpperCase().replace(/[^A-Z0-9]/g, '').slice(0, 8);
    return clean.length > 4 ? `${clean.slice(0, 4)}-${clean.slice(4)}` : clean;
  }
  /**
   * ⚠️ Toujours faux, et plus proposé nulle part.
   *
   * Le champ reste envoyé parce que le hub et les sondes déjà déployées le
   * connaissent : une sonde qui le porte dans sa configuration doit continuer
   * de fonctionner à la lettre après la mise à jour. Ce qui disparaît, c'est
   * la possibilité de l'activer — remplacée par l'épinglage, qui dit « ce
   * hub-là » au lieu de « je ne vérifie plus rien ».
   */
  const hubSelfSigned = false;
  let hubBusy = $state(false);
  let hubError = $state('');

  /**
   * Épinglage du certificat, à la première connexion.
   *
   * ⚠️ Remplace la case « accepter un certificat auto-signé », qui acceptait
   * **n'importe quel** certificat : n'importe qui sur le chemin pouvait se
   * faire passer pour le hub et repartir avec le jeton de la sonde. Cocher
   * cette case ne disait pas « je fais confiance à MON hub », elle disait
   * « je ne vérifie plus rien ».
   */
  let pendingPin = $state<{ host: string; fingerprint: string } | null>(null);

  async function refreshHub() {
    try {
      hub = await invoke('cmd_hub_status');
    } catch {
      // Le rattachement est optionnel : son indisponibilité ne doit pas
      // empêcher le reste des réglages de s'afficher.
    }
  }

  async function connectHub(pin: string = '') {
    hubError = '';
    hubBusy = true;
    try {
      const result = (await invoke('cmd_hub_enroll', {
        args: {
          url: hubUrl.trim(),
          // Facultatif : vide, c'est le hub qui nomme. Et un ré-enrôlement
          // ignore ce champ de toute façon — il répare une sonde existante.
          name: hubName.trim() || null,
          code: hubCode.trim() || null,
          allow_self_signed: hubSelfSigned,
          pin: pin || null,
        },
      })) as { needs_pin?: boolean; host?: string; fingerprint?: string };

      // Ce n'est pas un échec : c'est le moment où l'utilisateur doit
      // décider. Le rattachement reprendra avec l'empreinte confirmée.
      if (result?.needs_pin) {
        pendingPin = { host: result.host ?? '', fingerprint: result.fingerprint ?? '' };
        return;
      }
      pendingPin = null;
      hubCode = '';
      await refreshHub();

      // Le hub gardait peut-être les profils de cette machine : c'est le seul
      // moment où une machine réinstallée peut les retrouver. Ne remplace que
      // ce qui manque en local — voir `hubConfigBackup`.
      const restored = await restoreFromHub();
      if (restored > 0) {
        hubRestored = restored;
        await profiles.init();
        await portscanProfiles.init();
      } else {
        // Rien à restaurer : on dépose ce que cette machine a, pour la
        // prochaine fois. C'est l'enrôlement initial d'une sonde qui a déjà
        // servi en local.
        void pushNow();
      }
    } catch (e) {
      hubError = String(e);
    } finally {
      hubBusy = false;
    }
  }

  async function forgetHub() {
    try {
      await invoke('cmd_hub_forget');
      await refreshHub();
    } catch (e) {
      hubError = String(e);
    }
  }

  const langs: { value: Lang; labelKey: string }[] = [
    { value: 'en', labelKey: 'settings.language.english' },
    { value: 'fr', labelKey: 'settings.language.french' },
    { value: 'es', labelKey: 'settings.language.spanish' },
  ];

  // isWeb = client d'un desktop en mode serveur → read-only (le desktop pilote).
  // isHeadless = client du binaire headless standalone → full control.
  const isWeb = typeof window !== 'undefined' && (window as any).__LANPROBE_WEB__ === true;
  const isHeadless = typeof window !== 'undefined' && (window as any).__LANPROBE_HEADLESS__ === true;
  // En mode headless, le client web a tous les droits : il n'y a pas de desktop
  // pour piloter la config. En mode web-du-desktop, tout reste read-only.
  const readOnly = isWeb && !isHeadless;

  let appVersion = $state('…');
  let installType = $state<'pkg' | 'dmg' | 'unknown' | 'headless-server'>('unknown');
  let serverPlatform = $state<'windows' | 'macos' | 'linux' | null>(null);

  onMount(refreshHub);

  onMount(async () => {
    try { appVersion = await invoke<string>('cmd_app_version'); } catch {}
    try { installType = await invoke<'pkg' | 'dmg' | 'unknown' | 'headless-server'>('cmd_install_type'); } catch {}
    if (isHeadless) {
      try { serverPlatform = await invoke<'windows' | 'macos' | 'linux'>('cmd_get_platform'); } catch {}
    }
    await portscanProfiles.init();
    await scheduler.init();
  });

  const themes: { value: Theme; labelKey: string; descKey: string }[] = [
    { value: 'system', labelKey: 'settings.theme.system', descKey: 'settings.theme.system_desc' },
    { value: 'dark',   labelKey: 'settings.theme.dark',   descKey: 'settings.theme.dark_desc' },
    { value: 'light',  labelKey: 'settings.theme.light',  descKey: 'settings.theme.light_desc' },
  ];

  const palettes: { value: Palette; label: string; dark: string; light: string }[] = [
    { value: 'indigo',  label: 'Indigo',  dark: '#6366f1', light: '#4f46e5' },
    { value: 'cyan',    label: 'Cyan',    dark: '#06b6d4', light: '#0891b2' },
    { value: 'emerald', label: 'Emerald', dark: '#10b981', light: '#059669' },
    { value: 'rose',    label: 'Rose',    dark: '#f43f5e', light: '#e11d48' },
    { value: 'amber',   label: 'Amber',   dark: '#f59e0b', light: '#d97706' },
    { value: 'slate',   label: 'Slate',   dark: '#64748b', light: '#475569' },
  ];

  const layouts: { value: Layout; labelKey: string; descKey: string }[] = [
    { value: 'sidebar', labelKey: 'settings.layout.sidebar', descKey: 'settings.layout.sidebar_desc' },
    { value: 'single',  labelKey: 'settings.layout.single',  descKey: 'settings.layout.single_desc' },
    { value: 'grouped', labelKey: 'settings.layout.grouped', descKey: 'settings.layout.grouped_desc' },
  ];

  // Export / Import ------------------------------------------------------
  // Tout est géré côté frontend via Blob download + <input type="file">.
  // L'export ne contient que les profils réseau et de ports personnalisés :
  // l'utilisateur ne veut pas transporter le choix d'interface (spécifique
  // à la machine) ni les presets de ports built-in (régénérés à chaque
  // version de LanProbe).
  interface ConfigBundle {
    _meta: { app: 'lanprobe'; version: number; exported_at: string };
    profiles: Profile[];
    portscan_profiles: PortScanProfile[];
  }
  let importInput = $state<HTMLInputElement | null>(null);
  let importStatus = $state('');
  let importTimer: ReturnType<typeof setTimeout> | null = null;
  function flashImport(msg: string) {
    importStatus = msg;
    if (importTimer) clearTimeout(importTimer);
    importTimer = setTimeout(() => { importStatus = ''; importTimer = null; }, 5000);
  }

  function exportConfig() {
    const customPortscan = get(portscanProfiles).filter(p => !p.builtin && !p.id.startsWith('builtin:'));
    const bundle: ConfigBundle = {
      _meta: { app: 'lanprobe', version: 1, exported_at: new Date().toISOString() },
      profiles: get(profiles),
      portscan_profiles: customPortscan,
    };
    const blob = new Blob([JSON.stringify(bundle, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    const stamp = new Date().toISOString().replace(/[:.]/g, '-').slice(0, 19);
    a.href = url;
    a.download = `lanprobe-config-${stamp}.json`;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
    flashImport($_('settings.export_import.exported', { values: { profiles: bundle.profiles.length, portscan: bundle.portscan_profiles.length } }));
  }

  async function onImportFile(ev: Event) {
    const input = ev.target as HTMLInputElement;
    const file = input.files?.[0];
    if (!file) return;
    try {
      const text = await file.text();
      const parsed = JSON.parse(text) as ConfigBundle;
      if (parsed?._meta?.app !== 'lanprobe') {
        throw new Error('Not a LanProbe config');
      }
      // Merge: on ajoute ce qui n'existe pas (par id), on saute le reste.
      const existingProfileIds = new Set(get(profiles).map(p => p.id));
      let addedProfiles = 0;
      for (const p of parsed.profiles ?? []) {
        if (p && p.id && !existingProfileIds.has(p.id)) {
          profiles.add(p);
          addedProfiles++;
        }
      }
      const existingPortscanIds = new Set(get(portscanProfiles).map(p => p.id));
      let addedPortscan = 0;
      for (const p of parsed.portscan_profiles ?? []) {
        if (p && p.id && !p.builtin && !p.id.startsWith('builtin:') && !existingPortscanIds.has(p.id)) {
          portscanProfiles.add(p);
          addedPortscan++;
        }
      }
      flashImport($_('settings.export_import.imported', { values: { profiles: addedProfiles, portscan: addedPortscan } }));
    } catch (e) {
      flashImport(`${$_('common.error')}: ${e}`);
    } finally {
      input.value = '';
    }
  }

  const refreshOptions: { value: number; labelKey: string }[] = [
    { value: 0,  labelKey: 'settings.dashboard_refresh.disabled' },
    { value: 2,  labelKey: 'settings.dashboard_refresh.every_2s' },
    { value: 5,  labelKey: 'settings.dashboard_refresh.every_5s' },
    { value: 10, labelKey: 'settings.dashboard_refresh.every_10s' },
    { value: 30, labelKey: 'settings.dashboard_refresh.every_30s' },
  ];
</script>

<div class="page">
  <h1>{$_('settings.title')}</h1>
  <p class="page-sub">{readOnly ? $_('settings.web_readonly_hint') : 'LanProbe · v' + appVersion}</p>

  <!-- Rattachement à un hub : entièrement optionnel. Sans lui, LanProbe
       fonctionne exactement comme avant, sans compte ni serveur. -->
  <div class="group">
    <section class="section">
      <div class="section-head">{$_('settings.hub.title')}</div>
      {#if hub.enrolled}
        <p class="hub-state">
          {$_('settings.hub.connected')} <strong>{hub.url}</strong>
          — <strong>{hub.name}</strong>{#if hub.site} · {$_('settings.hub.site')} {hub.site}{/if}
        </p>
        <button class="hub-btn ghost" onclick={forgetHub} disabled={readOnly}>
          {$_('settings.hub.forget')}
        </button>
        {#if hubRestored > 0}
          <!-- Le dire : quelqu'un qui vient de réinstaller une machine ne
               s'attend pas à retrouver ses profils, et les découvrirait par
               hasard. -->
          <p class="hub-hint hub-restored">
            {$_('settings.hub.restored', { values: { n: hubRestored } })}
          </p>
        {/if}
        <p class="hub-hint">{$_('settings.hub.forget_hint')}</p>
      {:else}
        <p class="hub-hint">{$_('settings.hub.lead')}</p>
        <label class="hub-field">
          {$_('settings.hub.url')}
          <div class="url-row">
            <!-- Le schéma est un bouton, pas un caractère à taper : c'est la
                 partie de l'adresse qui ne varie qu'entre deux valeurs, et
                 c'est celle qu'on tape le plus mal. -->
            <button
              type="button"
              class="scheme"
              class:secure={scheme === 'https://'}
              onclick={toggleScheme}
              disabled={readOnly}
              title={$_('settings.hub.scheme_hint')}
            >{scheme}</button>
            <input
              type="text"
              bind:value={hubHost}
              placeholder="lanprobe.exemple.fr"
              autocapitalize="off"
              autocorrect="off"
              spellcheck="false"
              disabled={readOnly}
            />
          </div>
        </label>
        <label class="hub-field">
          {$_('settings.hub.name')}
          <input type="text" bind:value={hubName} placeholder={$_('settings.hub.name_ph')} disabled={readOnly} />
          <span class="hub-hint">{$_('settings.hub.name_hint')}</span>
        </label>
        <label class="hub-field">
          {$_('settings.hub.code')}
          <input
            type="text"
            value={hubCode}
            oninput={(e) => (hubCode = formatCode(e.currentTarget.value))}
            placeholder="K7M2-4PQX"
            spellcheck="false"
            autocapitalize="characters"
            autocomplete="off"
            disabled={readOnly}
          />
        </label>
        <p class="hub-hint">{$_('settings.hub.code_hint')}</p>
        {#if pendingPin}
          <!--
            L'empreinte se compare à l'œil, donc elle s'affiche en entier et en
            chasse fixe. La tronquer ferait comparer les premiers caractères,
            ce qui ne prouve rien.
          -->
          <div class="pin-box">
            <p class="pin-title">{$_('settings.hub.pin_title', { values: { host: pendingPin.host } })}</p>
            <p class="pin-fp">{pendingPin.fingerprint}</p>
            <p class="hub-hint">{$_('settings.hub.pin_hint')}</p>
            <div class="pin-acts">
              <button class="hub-btn ghost" onclick={() => (pendingPin = null)} disabled={hubBusy}>
                {$_('common.cancel')}
              </button>
              <button
                class="hub-btn"
                onclick={() => connectHub(pendingPin?.fingerprint ?? '')}
                disabled={hubBusy}
              >
                {$_('settings.hub.pin_confirm')}
              </button>
            </div>
          </div>
        {:else}
          <button class="hub-btn" onclick={() => connectHub()} disabled={readOnly || hubBusy || !hubUrl || !hubCode}>
            {hubBusy ? $_('settings.hub.connecting') : $_('settings.hub.connect')}
          </button>
        {/if}
        {#if hubError}
          <p class="hub-error" role="alert">{$_('settings.hub.error')} — {hubError}</p>
        {/if}
      {/if}
    </section>
  </div>

  <!-- Apparence : Thème + Layout groupés dans une carte -->
  <div class="group">
    <section class="section">
      <div class="section-head">{$_('settings.theme.title')}</div>
      <div class="option-grid">
        {#each themes as t}
          <label class="option-card" class:selected={$settings.theme === t.value} class:readonly={readOnly}>
            <input type="radio" name="theme" value={t.value}
              checked={$settings.theme === t.value}
              disabled={readOnly}
              onchange={() => settings.setTheme(t.value)} />
            <div class="option-body">
              <span class="option-label">{$_(t.labelKey)}</span>
              <span class="option-desc">{$_(t.descKey)}</span>
            </div>
            {#if $settings.theme === t.value}<span class="check">✓</span>{/if}
          </label>
        {/each}
      </div>
    </section>

    <section class="section">
      <div class="section-head">{$_('settings.palette.title')}</div>
      <div class="palette-row">
        {#each palettes as p}
          <button
            class="palette-swatch"
            class:palette-selected={$settings.palette === p.value}
            style="--swatch-color: {$settings.theme === 'light' || ($settings.theme === 'system' && typeof window !== 'undefined' && window.matchMedia('(prefers-color-scheme: light)').matches) ? p.light : p.dark}"
            title={p.label}
            disabled={readOnly}
            onclick={() => settings.setPalette(p.value)}
          >
            <span class="swatch-dot"></span>
            <span class="swatch-label">{p.label}</span>
          </button>
        {/each}
      </div>
    </section>

    <section class="section">
      <div class="section-head">{$_('settings.layout.title')}</div>
      <div class="option-grid">
        {#each layouts as l}
          <label class="option-card" class:selected={$settings.layout === l.value} class:readonly={readOnly}>
            <input type="radio" name="layout" value={l.value}
              checked={$settings.layout === l.value}
              disabled={readOnly}
              onchange={() => settings.setLayout(l.value)} />
            <div class="option-body">
              <span class="option-label">{$_(l.labelKey)}</span>
              <span class="option-desc">{$_(l.descKey)}</span>
            </div>
            {#if $settings.layout === l.value}<span class="check">✓</span>{/if}
          </label>
        {/each}
      </div>
    </section>
  </div>

  <!-- Comportement : Refresh + Auto Port Scan groupés -->
  <div class="group">
    <section class="section">
      <div class="section-head">{$_('settings.dashboard_refresh.title')}</div>
      <label class="row-control">
        <span class="row-label">{$_('settings.dashboard_refresh.label')}</span>
        <select
          value={$settings.dashboardRefreshSec}
          disabled={readOnly}
          onchange={(e) => settings.setDashboardRefresh(Number((e.target as HTMLSelectElement).value))}
        >
          {#each refreshOptions as opt}
            <option value={opt.value}>{$_(opt.labelKey)}</option>
          {/each}
        </select>
      </label>
      <p class="hint">{$_('settings.dashboard_refresh.hint')}</p>
    </section>

    <section class="section">
      <div class="section-head">{$_('settings.auto_port_scan.title')}</div>
      <label class="row-control">
        <span class="row-label">{$_('settings.auto_port_scan.label')}</span>
        <select
          value={$settings.autoPortScanProfileId ?? ''}
          disabled={readOnly}
          onchange={(e) => {
            const v = (e.target as HTMLSelectElement).value;
            settings.setAutoPortScanProfile(v === '' ? null : v);
          }}
        >
          <option value="">{$_('settings.auto_port_scan.disabled')}</option>
          {#each $portscanProfiles as p (p.id)}
            <option value={p.id}>{p.builtin ? p.name : `★ ${p.name}`} ({p.tcp_ports.length}T/{p.udp_ports.length}U)</option>
          {/each}
        </select>
      </label>
      <p class="hint">{$_('settings.auto_port_scan.hint')}</p>
    </section>
  </div>

  <!-- Speedtest : moteur + config iperf dans une carte -->
  <div class="group">
    <section class="section">
      <div class="section-head">{$_('settings.speedtest.title')}</div>
      <div class="option-grid">
        <label class="option-card" class:selected={$settings.speedtestEngine === 'ookla'}>
          <input type="radio" name="speedtest-engine" value="ookla"
            checked={$settings.speedtestEngine === 'ookla'}
            onchange={() => settings.setSpeedtestEngine('ookla')} />
          <div class="option-body">
            <span class="option-label">{$_('settings.speedtest.ookla')}</span>
            <span class="option-desc">{$_('settings.speedtest.ookla_desc')}</span>
          </div>
          {#if $settings.speedtestEngine === 'ookla'}<span class="check">✓</span>{/if}
        </label>
        <label class="option-card" class:selected={$settings.speedtestEngine === 'iperf3'}>
          <input type="radio" name="speedtest-engine" value="iperf3"
            checked={$settings.speedtestEngine === 'iperf3'}
            onchange={() => settings.setSpeedtestEngine('iperf3')} />
          <div class="option-body">
            <span class="option-label">{$_('settings.speedtest.iperf3')}</span>
            <span class="option-desc">{$_('settings.speedtest.iperf3_desc')}</span>
          </div>
          {#if $settings.speedtestEngine === 'iperf3'}<span class="check">✓</span>{/if}
        </label>
      </div>
    </section>
    {#if $settings.speedtestEngine === 'iperf3'}
    <section class="section">
      <div class="section-head">{$_('settings.speedtest.server_label')}</div>
      <label class="row-control">
        <span class="row-label">{$_('settings.speedtest.server_label')}</span>
        <input
          type="text"
          placeholder="iperf.example.com"
          value={$settings.iperfServer}
          oninput={(e) => settings.setIperfServer((e.target as HTMLInputElement).value)}
        />
      </label>
      <p class="hint">{$_('settings.speedtest.iperf_hint')}</p>
    </section>
    {/if}
  </div>

  <!-- Langue + Données dans une carte -->
  <div class="group">
    <section class="section">
      <div class="section-head">{$_('settings.language.title')}</div>
      <div class="option-grid">
        {#each langs as l}
          <label class="option-card" class:selected={$settings.lang === l.value}>
            <input type="radio" name="lang" value={l.value}
              checked={$settings.lang === l.value}
              disabled={readOnly}
              onchange={() => settings.setLanguage(l.value)} />
            <div class="option-body">
              <span class="option-label">{$_(l.labelKey)}</span>
            </div>
            {#if $settings.lang === l.value}<span class="check">✓</span>{/if}
          </label>
        {/each}
      </div>
    </section>

    <section class="section">
      <div class="section-head">{$_('settings.export_import.title')}</div>
      <div class="btn-row">
        <button class="action-btn" onclick={exportConfig} disabled={readOnly}>{$_('settings.export_import.export')}</button>
        <button class="action-btn" onclick={() => importInput?.click()} disabled={readOnly}>{$_('settings.export_import.import')}</button>
        <input
          bind:this={importInput}
          type="file"
          accept=".json,application/json"
          onchange={onImportFile}
          style="display: none;"
        />
      </div>
      <p class="hint">{$_('settings.export_import.hint')}</p>
      {#if importStatus}
        <p class="hint" style="color: var(--ep-accent);">{importStatus}</p>
      {/if}
    </section>
  </div>


  <!-- À propos -->
  <div class="group">
  <section class="section" style="margin-bottom:0;">
    <div class="section-head">{$_('settings.about.title')}</div>
    <div class="about-rows">
      <div class="about-row"><span class="key">{$_('settings.about.application')}</span><span class="mono">LanProbe</span></div>
      <div class="about-row"><span class="key">{$_('settings.about.version')}</span><span class="mono">v{appVersion}</span></div>
      <div class="about-row" class:no-border={installType === 'unknown' || installType === 'headless-server' || (!isHeadless && !navigator.userAgent.includes('Mac'))}>
        <span class="key">{$_('settings.about.platform')}</span><span class="mono">
          {#if isHeadless && serverPlatform}
            {serverPlatform === 'windows' ? 'Windows' : serverPlatform === 'macos' ? 'macOS' : 'Linux'}
            <span style="font-size:11px;color:var(--ep-text-muted);margin-left:4px;">(server)</span>
          {:else if navigator.userAgent.includes('Win')}Windows
          {:else if navigator.userAgent.includes('Mac')}macOS
          {:else}Linux{/if}
        </span>
      </div>
      {#if isHeadless}
      <div class="about-row" style="border-bottom:none;">
        <span class="key">{$_('settings.about.install_type')}</span>
        <span class="mono install-badge" style="background:var(--ep-glass-bg);color:var(--ep-text);">Headless</span>
      </div>
      {:else if navigator.userAgent.includes('Mac') && installType !== 'unknown'}
      <div class="about-row" style="border-bottom:none;">
        <span class="key">{$_('settings.about.install_type')}</span>
        <span class="mono install-badge install-{installType}">
          {installType === 'pkg' ? 'PKG' : 'DMG'}
          <span class="install-hint">{$_('settings.about.install_hint_' + installType)}</span>
        </span>
      </div>
      {/if}
    </div>
  </section>
  </div>
</div>

<style>
  .hub-field { display: flex; flex-direction: column; gap: 4px; margin-bottom: 10px; font-size: 13px; }
  .hub-field input { padding: 7px 9px; border-radius: 6px; border: 1px solid var(--border, #333);
    background: var(--input-bg, #1a1a1a); color: inherit; font-size: 13px; }
  /* Le schéma et l'hôte forment un seul champ à l'œil : bordure commune,
     coins arrondis aux extrémités seulement. Deux boîtes séparées se
     liraient comme deux réglages sans rapport. */
  .url-row { display: flex; }
  .url-row input { flex: 1; min-width: 0; border-radius: 0 6px 6px 0; border-left: none; }
  .scheme {
    padding: 7px 9px; border: 1px solid var(--border, #333); border-radius: 6px 0 0 6px;
    background: var(--input-bg, #1a1a1a); color: inherit;
    font-family: var(--ep-font-mono, monospace); font-size: 13px;
    cursor: pointer; white-space: nowrap;
  }
  .scheme:hover:not(:disabled) { border-color: var(--accent, #6366f1); }
  /* `https` porte une marque : le passage en clair doit se voir, pas se
     deviner à la lecture de huit caractères. */
  .scheme.secure { color: var(--ep-success, #22c55e); }
  .scheme:disabled { opacity: .5; cursor: default; }
  .hub-check { display: flex; align-items: center; gap: 8px; font-size: 13px; margin: 4px 0; }
  .hub-hint { font-size: 12px; opacity: .65; margin: 2px 0 10px; }
  .hub-restored { color: var(--ep-success, #22c55e); opacity: 1; }
  .hub-state { font-size: 13px; margin-bottom: 10px; }
  /* L'empreinte se compare à l'œil : chasse fixe, entière, et coupable de
     prendre de la place. La tronquer ferait comparer les premiers caractères,
     ce qui ne prouve rien. */
  .pin-box {
    margin: 8px 0;
    padding: 10px 12px;
    border: 1px solid var(--lp-warn, #b8860b);
    border-radius: 8px;
    background: color-mix(in srgb, var(--lp-warn, #b8860b) 8%, transparent);
  }
  .pin-title {
    margin: 0 0 6px;
    font-size: 13px;
    font-weight: 600;
  }
  .pin-fp {
    margin: 0 0 6px;
    font-family: ui-monospace, monospace;
    font-size: 11.5px;
    line-height: 1.6;
    word-break: break-all;
    user-select: all;
  }
  .pin-acts {
    display: flex;
    gap: 8px;
    margin-top: 8px;
  }
  .hub-btn.ghost {
    background: none;
    border: 1px solid var(--lp-border, #4444);
  }
  .hub-error { font-size: 12px; color: var(--danger, #f87171); margin-top: 8px; }
  .hub-btn { padding: 7px 14px; border-radius: 6px; border: 1px solid transparent;
    background: var(--accent, #6366f1); color: #fff; font-size: 13px; cursor: pointer; }
  .hub-btn.ghost { background: transparent; border-color: var(--border, #333); color: inherit; }
  .hub-btn:disabled { opacity: .5; cursor: default; }

  .page { padding: 24px 28px; max-width: 680px; }
  h1 { font-size: 18px; font-weight: 800; margin-bottom: 6px; letter-spacing: -.2px; }
  .page-sub { font-size: 12px; color: var(--ep-text-muted); margin-bottom: 24px; }

  /* Group card: plusieurs sections dans une carte vitrée */
  .group {
    background: var(--ep-glass-bg);
    border: 1px solid var(--ep-glass-border);
    border-radius: var(--ep-radius-lg);
    margin-bottom: 16px;
    overflow: hidden;
  }
  .group .section {
    padding: 14px 18px;
    margin-bottom: 0;
    border-bottom: 1px solid var(--ep-glass-border);
  }
  .group .section:last-child { border-bottom: none; }

  .section { margin-bottom: 16px; }
  .section-head {
    font-size: 10px; font-weight: 700;
    color: var(--ep-text-muted);
    text-transform: uppercase; letter-spacing: .8px;
    margin-bottom: 10px;
  }

  /* Option cards (theme, layout, speedtest engine, language) */
  .option-grid { display: flex; flex-direction: row; flex-wrap: wrap; gap: 6px; }
  .option-card {
    display: flex; align-items: center; gap: 12px;
    padding: 10px 14px;
    flex: 1; min-width: 150px;
    background: var(--ep-bg-tertiary);
    border: 1px solid var(--ep-glass-border);
    border-radius: var(--ep-radius-md);
    cursor: pointer;
    transition: border-color 0.12s, background 0.12s;
  }
  .option-card:hover { border-color: var(--ep-glass-border-strong); background: var(--ep-glass-bg-md); }
  .option-card.selected { border-color: var(--ep-accent); background: var(--ep-accent-dim); }
  .option-card.readonly { cursor: not-allowed; opacity: 0.6; }
  .option-card input { display: none; }
  .option-body { flex: 1; display: flex; flex-direction: column; gap: 2px; }
  .option-label { font-size: 13px; font-weight: 600; }
  .option-desc { font-size: 11.5px; color: var(--ep-text-secondary); }
  .check { color: var(--ep-accent-bright); font-size: 14px; }

  /* Row controls (select, input) */
  .row-control {
    display: flex; align-items: center; justify-content: space-between;
    padding: 12px 16px;
    background: var(--ep-glass-bg);
    border: 1px solid var(--ep-glass-border);
    border-radius: var(--ep-radius-lg);
    gap: 16px;
  }
  .row-label { font-size: 13px; font-weight: 600; }
  .row-control select,
  .row-control input[type="text"],
  .row-control select:focus,
  .row-control input:focus { outline: none; border-color: var(--ep-accent); }
  .row-control select:disabled,
  .row-control input:disabled { opacity: 0.55; cursor: not-allowed; }

  .hint { font-size: 11.5px; color: var(--ep-text-muted); margin-top: 7px; line-height: 1.5; }
  .hint-success { color: var(--ep-success); }
  .addr-link { background: none; border: none; padding: 0; color: var(--ep-success); font-size: inherit; font-weight: 700; cursor: pointer; text-decoration: underline; text-underline-offset: 2px; }
  .hint-danger  { color: var(--ep-danger); }

  /* Export/import row */
  .btn-row { display: flex; gap: 8px; flex-wrap: wrap; }
  .action-btn {
    background: var(--ep-glass-bg-md);
    border: 1px solid var(--ep-glass-border-strong);
    color: var(--ep-text-primary);
    padding: 9px 16px;
    border-radius: var(--ep-radius-md);
    font-size: 12.5px; font-weight: 600;
    cursor: pointer;
    transition: border-color 0.12s, background 0.12s;
  }
  .action-btn:hover { border-color: var(--ep-accent); background: var(--ep-accent-dim); color: var(--ep-accent-bright); }
  .action-btn:disabled { opacity: 0.5; cursor: not-allowed; }

  /* Server mode start/stop */
  .server-start { background: var(--ep-accent-dim); border-color: var(--ep-accent); color: var(--ep-accent-bright); }
  .server-stop  { background: rgba(239,68,68,0.12); border-color: var(--ep-danger); color: var(--ep-danger); }

  /* Account feedback */
  .account-feedback { font-size: 12px; color: var(--ep-success); font-weight: 600; }

  /* About rows (dans la group card) */
  .about-rows { display: flex; flex-direction: column; }
  .about-row {
    display: flex; justify-content: space-between; align-items: center;
    padding: 9px 0;
    border-bottom: 1px solid var(--ep-glass-border);
    font-size: 12.5px;
  }
  .about-row .key { color: var(--ep-text-secondary); }
  .about-row.no-border { border-bottom: none; }
  .mono { font-family: var(--ep-font-mono); color: var(--ep-accent-bright); font-size: 12px; }
  .install-badge { display: flex; align-items: center; gap: 8px; }
  .install-pkg  { color: #10b981; }
  .install-dmg  { color: #f59e0b; }
  .install-hint { font-family: var(--ep-font-sans, sans-serif); font-size: 11px;
                  color: var(--ep-text-muted); font-weight: 400; }

  /* Palette picker */
  .palette-row { display: flex; flex-wrap: wrap; gap: 8px; }
  .palette-swatch {
    display: flex; flex-direction: column; align-items: center; gap: 5px;
    padding: 8px 10px;
    background: var(--ep-bg-tertiary);
    border: 2px solid var(--ep-glass-border);
    border-radius: var(--ep-radius-md);
    cursor: pointer;
    transition: border-color 0.12s, transform 0.1s;
    min-width: 62px;
  }
  .palette-swatch:hover { border-color: var(--swatch-color); transform: translateY(-1px); }
  .palette-swatch.palette-selected { border-color: var(--swatch-color); background: var(--ep-accent-dim); }
  .palette-swatch:disabled { opacity: 0.5; cursor: not-allowed; transform: none; }
  .swatch-dot {
    width: 22px; height: 22px; border-radius: 50%;
    background: var(--swatch-color);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--swatch-color) 20%, transparent);
  }
  .swatch-label { font-size: 10px; font-weight: 600; color: var(--ep-text-secondary); }
  .palette-swatch.palette-selected .swatch-label { color: var(--ep-text-primary); }
</style>
