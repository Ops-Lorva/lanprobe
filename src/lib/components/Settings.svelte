<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { _ } from 'svelte-i18n';
  import { settings, type Theme, type Layout, type Lang, type SpeedtestEngine, type Palette } from '../stores/settings';
  import { portscanProfiles, type PortScanProfile } from '../stores/portscanProfiles';
  import { profiles, type Profile } from '../stores/profiles';
  import { get } from 'svelte/store';
  import { loadServerMode, saveServerMode } from '../stores/serverMode';
  import { selectedInterface } from '../stores/selectedInterface';
  import { influxDb, type InfluxDbConfig } from '../stores/influxdb';
  import { scheduler } from '../stores/scheduler';

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

  // Mode serveur : expose l'UI LanProbe sur un port HTTPS pour que d'autres
  // postes du LAN puissent s'y connecter via navigateur. Utile quand on
  // veut piloter LanProbe installé sur un Pi/serveur sans GUI depuis son
  // poste courant, sans changer de VLAN.
  let serverRunning = $state(false);
  let serverAddr = $state<string | null>(null);
  let serverPort = $state(8443);
  let serverHost = $state('0.0.0.0');
  let serverError = $state('');
  let serverBusy = $state(false);

  // InfluxDB config
  let influxCfg = $state<InfluxDbConfig>({ ...$influxDb, v1: { ...$influxDb.v1 }, v2: { ...$influxDb.v2 } });
  let influxTestStatus = $state<'idle' | 'testing' | 'ok' | 'fail'>('idle');
  let influxTestError = $state('');

  $effect(() => {
    influxCfg = { ...$influxDb, v1: { ...$influxDb.v1 }, v2: { ...$influxDb.v2 } };
  });

  async function saveInflux() {
    await influxDb.save(influxCfg);
  }

  async function testInflux() {
    await saveInflux();    // flush current form state to backend config first
    influxTestStatus = 'testing';
    influxTestError = '';
    try {
      const res = await invoke<{ ok: boolean; error?: string }>('cmd_test_influxdb', {});
      if (res.ok) {
        influxTestStatus = 'ok';
      } else {
        influxTestStatus = 'fail';
        influxTestError = res.error ?? '';
      }
    } catch (e) {
      influxTestStatus = 'fail';
      influxTestError = String(e);
    }
  }

  // Compte serveur (username/password)
  let accountUsername = $state('');
  let accountPassword = $state('');
  let accountBusy = $state(false);
  let accountFeedback = $state('');
  let accountFeedbackTimer: ReturnType<typeof setTimeout> | null = null;

  function flashAccountFeedback(msg: string) {
    accountFeedback = msg;
    if (accountFeedbackTimer) clearTimeout(accountFeedbackTimer);
    accountFeedbackTimer = setTimeout(() => { accountFeedback = ''; accountFeedbackTimer = null; }, 4000);
  }

  async function saveAccount() {
    if (!accountUsername.trim()) return;
    if (accountPassword.length < 8) return;
    accountBusy = true;
    try {
      await invoke('cmd_server_mode_set_account', {
        args: { username: accountUsername.trim(), password: accountPassword }
      });
      accountPassword = '';
      flashAccountFeedback($_('settings.server_mode.account_saved'));
    } catch (e) {
      flashAccountFeedback(`${$_('common.error')}: ${e}`);
    }
    accountBusy = false;
  }

  // Remplace 0.0.0.0 dans l'adresse du serveur par l'IP réelle de
  // l'interface active — plus lisible pour l'utilisateur qui veut
  // communiquer l'URL à un autre poste du LAN.
  async function resolveServerAddr(raw: string | null): Promise<string | null> {
    if (!raw || !raw.includes('0.0.0.0')) return raw;
    try {
      const iface = $selectedInterface;
      if (!iface) return raw;
      const details = await invoke<{ ip: string | null }>('cmd_get_interface_details', { name: iface });
      if (details?.ip) return raw.replace('0.0.0.0', details.ip);
    } catch {}
    return raw;
  }

  async function refreshServerStatus() {
    try {
      const s = await invoke<{ running: boolean; addr: string | null }>('cmd_server_mode_status');
      serverRunning = s.running;
      serverAddr = await resolveServerAddr(s.addr);
    } catch (e) { serverError = String(e); }
  }
  async function toggleServer() {
    serverBusy = true; serverError = '';
    try {
      if (serverRunning) {
        await invoke('cmd_server_mode_stop');
        await saveServerMode({ enabled: false, host: serverHost, port: serverPort });
      } else {
        // Le compte est créé via la page web au premier accès (setup page),
        // comme sur la version headless. Pas de formulaire dans les Settings.
        const s = await invoke<{ running: boolean; addr: string | null }>('cmd_server_mode_start', {
          args: { host: serverHost, port: serverPort }
        });
        serverRunning = s.running;
        serverAddr = await resolveServerAddr(s.addr);
        await saveServerMode({ enabled: true, host: serverHost, port: serverPort });
      }
      await refreshServerStatus();
    } catch (e) { serverError = String(e); }
    serverBusy = false;
  }

  onMount(async () => {
    try { appVersion = await invoke<string>('cmd_app_version'); } catch {}
    try { installType = await invoke<'pkg' | 'dmg' | 'unknown' | 'headless-server'>('cmd_install_type'); } catch {}
    if (isHeadless) {
      try { serverPlatform = await invoke<'windows' | 'macos' | 'linux'>('cmd_get_platform'); } catch {}
    }
    await portscanProfiles.init();
    await influxDb.init();
    await scheduler.init();
    // Restaure la dernière config mode serveur pour que l'UI reflète
    // ce qui a été auto-lancé au boot de l'app.
    try {
      const cfg = await loadServerMode();
      serverHost = cfg.host;
      serverPort = cfg.port;
    } catch {}
    await refreshServerStatus();
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

  {#if !isWeb}
  <!-- Serveur web dans sa propre carte -->
  <div class="group">
  <!-- Compte serveur (username/password) — toujours visible -->
  <section class="section">
    <div class="section-head">{$_('settings.server_mode.account_title')}</div>
    <div class="row-control">
      <span class="row-label">{$_('settings.server_mode.username')}</span>
      <input type="text" bind:value={accountUsername} placeholder="admin" autocomplete="username" />
    </div>
    <div class="row-control" style="margin-top: 6px;">
      <span class="row-label">{$_('settings.server_mode.password')}</span>
      <input type="password" bind:value={accountPassword} placeholder="••••••••" autocomplete="new-password" />
    </div>
    <div style="margin-top: 8px; display: flex; align-items: center; gap: 10px;">
      <button
        class="action-btn"
        onclick={saveAccount}
        disabled={accountBusy || !accountUsername.trim() || accountPassword.length < 8}
      >
        {$_('settings.server_mode.save_account')}
      </button>
      {#if accountFeedback}
        <span class="account-feedback">{accountFeedback}</span>
      {/if}
    </div>
    <p class="hint">{$_('settings.server_mode.account_hint')}</p>
  </section>
  <section class="section">
    <div class="section-head">{$_('settings.server_mode.title')}</div>
    <div class="row-control">
      <span class="row-label">{$_('settings.server_mode.label')}</span>
      <button class="action-btn {serverRunning ? 'server-stop' : 'server-start'}" onclick={toggleServer} disabled={serverBusy}>
        {serverRunning ? $_('settings.server_mode.stop') : $_('settings.server_mode.start')}
      </button>
    </div>
    {#if !serverRunning}
      <div class="row-control" style="margin-top: 6px;">
        <span class="row-label">{$_('settings.server_mode.port')}</span>
        <input type="text" bind:value={serverPort} style="min-width: 100px;" />
      </div>
    {/if}
    {#if serverRunning && serverAddr}
      <p class="hint hint-success">
        {$_('settings.server_mode.listening_prefix')}
        <button class="addr-link" onclick={() => invoke('cmd_open_url', { url: serverAddr })}>{serverAddr}</button>
      </p>
    {/if}
    {#if serverError}
      <p class="hint hint-danger">{serverError}</p>
    {/if}
    <p class="hint">{$_('settings.server_mode.hint')}</p>
  </section>
  </div>
  {/if}

  <!-- InfluxDB Export -->
  <div class="group">
    <section class="section">
      <div class="section-head">{$_('settings.influxdb.title')}</div>
      <div class="row-control">
        <span class="row-label">{$_('settings.influxdb.enabled')}</span>
        <input type="checkbox" bind:checked={influxCfg.enabled} onchange={saveInflux} />
      </div>
      <div class="row-control" style="margin-top: 6px;">
        <span class="row-label">{$_('settings.influxdb.version')}</span>
        <select bind:value={influxCfg.version} onchange={saveInflux}>
          <option value="v2">InfluxDB v2</option>
          <option value="v1">InfluxDB v1</option>
        </select>
      </div>
      <div class="row-control" style="margin-top: 6px;">
        <span class="row-label">{$_('settings.influxdb.url')}</span>
        <input type="text" bind:value={influxCfg.url} placeholder={$_('settings.influxdb.url_placeholder')} onblur={saveInflux} />
      </div>
      <div class="row-control" style="margin-top: 6px;">
        <span class="row-label">{$_('settings.influxdb.instance_label')}</span>
        <input type="text" bind:value={influxCfg.instance_label} placeholder={$_('settings.influxdb.instance_label_placeholder')} onblur={saveInflux} />
      </div>
      {#if influxCfg.version === 'v1'}
        <div class="row-control" style="margin-top: 6px;">
          <span class="row-label">{$_('settings.influxdb.v1_database')}</span>
          <input type="text" bind:value={influxCfg.v1.database} onblur={saveInflux} />
        </div>
        <div class="row-control" style="margin-top: 6px;">
          <span class="row-label">{$_('settings.influxdb.v1_username')}</span>
          <input type="text" bind:value={influxCfg.v1.username} onblur={saveInflux} />
        </div>
        <div class="row-control" style="margin-top: 6px;">
          <span class="row-label">{$_('settings.influxdb.v1_password')}</span>
          <input type="password" bind:value={influxCfg.v1.password} onblur={saveInflux} />
        </div>
      {:else}
        <div class="row-control" style="margin-top: 6px;">
          <span class="row-label">{$_('settings.influxdb.v2_org')}</span>
          <input type="text" bind:value={influxCfg.v2.org} onblur={saveInflux} />
        </div>
        <div class="row-control" style="margin-top: 6px;">
          <span class="row-label">{$_('settings.influxdb.v2_bucket')}</span>
          <input type="text" bind:value={influxCfg.v2.bucket} onblur={saveInflux} />
        </div>
        <div class="row-control" style="margin-top: 6px;">
          <span class="row-label">{$_('settings.influxdb.v2_token')}</span>
          <input type="password" bind:value={influxCfg.v2.token} onblur={saveInflux} />
        </div>
      {/if}
      <div style="margin-top: 10px; display: flex; align-items: center; gap: 10px;">
        <button class="action-btn" onclick={testInflux} disabled={influxTestStatus === 'testing' || !influxCfg.enabled}>
          {$_('settings.influxdb.test_btn')}
        </button>
        {#if influxTestStatus === 'ok'}
          <span style="color: var(--ep-success, #22c55e);">{$_('settings.influxdb.test_ok')}</span>
        {:else if influxTestStatus === 'fail'}
          <span style="color: var(--ep-danger, #ef4444);">{$_('settings.influxdb.test_fail')}{#if influxTestError}: {influxTestError}{/if}</span>
        {/if}
      </div>
      <p class="hint">{$_('settings.influxdb.hint')}</p>
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
  .row-control input[type="password"] {
    background: var(--ep-bg-tertiary);
    border: 1px solid var(--ep-glass-border-strong);
    color: var(--ep-text-primary);
    padding: 6px 10px;
    border-radius: var(--ep-radius-md);
    font-size: 12.5px;
    font-family: var(--ep-font-mono);
    min-width: 180px;
    transition: border-color 0.12s;
  }
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
