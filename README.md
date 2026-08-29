<div align="center">

[🇫🇷 Français](README.fr.md) · 🇬🇧 English

# LanProbe

**Network monitoring and diagnostics — probes, and a hub to drive them**

*Interface Profiles · Ping Monitor · SLA · Discovery · Port Scan · Speed Test · Self-hosted Hub*

[![Latest release](https://img.shields.io/github/v/release/Benjamin-Chianese/lanprobe?label=release&style=flat-square)](https://github.com/Benjamin-Chianese/lanprobe/releases/latest)
[![Tauri](https://img.shields.io/badge/Tauri-2.x-FFC131?logo=tauri&logoColor=white&style=flat-square)](https://tauri.app)
[![Rust](https://img.shields.io/badge/Rust-1.85+-CE422B?logo=rust&logoColor=white&style=flat-square)](https://rustlang.org)
[![Svelte](https://img.shields.io/badge/Svelte-5-FF3E00?logo=svelte&logoColor=white&style=flat-square)](https://svelte.dev)
[![Platform](https://img.shields.io/badge/Platform-Windows%20%7C%20macOS%20%7C%20Linux-6366f1?style=flat-square)](#-compatibility)
[![License](https://img.shields.io/badge/License-MIT-22c55e?style=flat-square)](LICENSE)

</div>

---

## 🖧 What is LanProbe?

LanProbe replaces a handful of separate network utilities with one coherent tool, for people who switch interfaces often, debug connectivity, or watch several hosts at once.

It comes in **two pieces**, and the split is worth knowing before installing anything:

| | Role |
|---|---|
| **The probe** | Measures. Desktop app (Windows / macOS / Linux) or headless service. It pings, scans and speed-tests — from inside the network being watched. |
| **The hub** | Watches. A Docker container on your own machine, gathering every probe behind **one address and one authentication**. |

A probe on its own is perfectly usable: open the window, it measures. The hub earns its place as soon as there are several sites, several machines, or a need to look without sitting in front of the screen.

⚠️ **The direction of the connection matters.** The hub never reaches into a probe — it has no route to its network, and that is the point. The probe calls the hub, outbound. So there is **no inbound port to open** on the monitored network, and no VPN to set up.

---

## 🧩 Features

| Module | Description |
|--------|-------------|
| 🔀 **Interface profiles** | Named static-IP or DHCP configurations, applied in one click |
| 📡 **Ping Monitor** | Continuous ICMP watch over several hosts, live latency, configurable thresholds |
| 📊 **SLA export** | Uptime % per host, avg / min / max / P95 latency — CSV export |
| 🔍 **Network discovery** | Async CIDR sweep: IP, hostname, MAC address |
| 🔌 **Port Scan** | TCP scan with built-in profiles (common, web, full) and custom ones |
| ⚡ **Speed Test** | Ookla or iperf3, **bound to the selected interface** |
| 🛡️ **Internet status** | Dual ICMP + HTTP probe, public IP, uptime percentage |
| 🌐 **Self-hosted hub** | Sites, probes, accounts, roles, audit log, alerts, backups |
| 🎨 **Theme** | Dark / light / system, 6 accent palettes |

### The rule that governs everything: the selected interface

⚠️ **All of a probe's traffic leaves through the interface you picked.** The measurements, but also its heartbeat to the hub and the delivery of its readings.

This is not an implementation detail. You can have internet on `eth1` and none on `eth0`, and `eth0` is precisely the one under test. A probe that measured a dead link while reporting “all good” over another link would be asserting something it never checked.

The consequence to know: if the selected interface loses its address, the probe **stops beating** and turns “no news” in the fleet. That is the truth of the interface being watched, not a fault in the probe.

---

## 📦 Installation

### 💻 Desktop app

From the [releases](https://github.com/Benjamin-Chianese/lanprobe/releases/latest):

| Platform | File |
|---|---|
| Windows | `lanprobe_vX.Y.Z_x64-setup.exe` |
| macOS | `lanprobe_vX.Y.Z_universal.pkg` (signed + notarized) |
| Linux | `lanprobe_vX.Y.Z_amd64.deb` |

On macOS the `.pkg` provisions the sudoers rights needed to change an IP. On Linux the package sets `CAP_NET_RAW` and `CAP_NET_ADMIN` on the binary: raw ICMP pings and interface changes work without root.

---

### 🗄️ Headless probe on Debian / Ubuntu

For a Raspberry Pi, a VM, or a server with no desktop.

```bash
curl -fsSL -o install-server.sh https://raw.githubusercontent.com/Benjamin-Chianese/lanprobe/main/install-server.sh
sudo bash install-server.sh
```

⚠️ **The headless probe listens on no port.** It used to serve its own web interface over HTTPS on 8443, with its own accounts and self-signed certificate. That role now belongs to the hub: a probe exposing its own interface as well would be a second surface to secure for the very same measurements. So you drive it from the command line, and you look at it from the hub.

**Attaching.** In the hub, create an enrolment code (Fleet → “+” on a site), then:

```bash
sudo -u lanprobe lanprobe-server --config-dir /var/lib/lanprobe \
     enroll --hub https://hub.example.com --code A1B2-C3D4

sudo systemctl enable --now lanprobe-server
```

The code lasts 15 minutes and works once. The probe shows up in the fleet on its first heartbeat, so under a minute.

**Commands:**

```bash
lanprobe-server run       # measure and send, until Ctrl-C (default action)
lanprobe-server enroll    # attach to a hub
lanprobe-server status    # show the attachment
lanprobe-server forget    # detach — the hub keeps the measurements
```

`--config-dir` applies to every command. Default: `~/.config/lanprobe`, and `/var/lib/lanprobe` for the systemd service.

For scripted deployment, `--username`, `--password` and `--site` replace `--code`. Avoid it on a machine you do not control: a compromised host should not receive the hub's credentials.

**Firewall:** nothing to open inbound. The probe only needs to reach the hub's address outbound.

**Sealing key.** The probe's local secrets — its hub token — are encrypted at rest. The key is created on first start inside the config directory. To supply your own (immutable container, secret manager):

```bash
head -c 32 /dev/urandom | base64        # 32 random bytes
# then, as a systemd drop-in:
# [Service]
# Environment=LANPROBE_SECRET_KEY=<base64-32-bytes>
```

---

### 🐳 Self-hosted hub (Docker)

One container: the hub, its SQLite database, and an embedded InfluxDB. **We host nothing** — it all runs on a machine you control.

```bash
git clone https://github.com/Benjamin-Chianese/lanprobe.git && cd lanprobe
docker compose up -d
docker compose logs lanprobe-web | grep -i "setup token"
```

Open the hub, paste the token, create the administrator account — the token is consumed on first use. Then **Enrol a probe**: the hub gives you a short code, valid 15 minutes. In LanProbe, **Settings → Server connection**, enter the hub address and that code. No password ever reaches the monitored machine.

Accounts carry a role (`admin` / `operator` / `viewer`), every action is written to the audit log, and offline / online transitions can go out by e-mail or webhook (Slack, Discord, ntfy…). The [full API lives in the contract](docs/lanprobe-web-contrat.md).

**Two traps that cost ten minutes:**
- the setup token is readable **only** in `docker logs` — the interface never shows it;
- if that machine was ever reached over HTTPS, the browser keeps a `Secure` cookie that silently blocks the later HTTP session: sign-in appears to succeed, then bounces back to the login screen, **with no error**. Private window, or clear the site's cookies.

The hub serves **in plain HTTP by default**, meant to live behind your own reverse proxy. `--tls` produces a self-signed certificate, fine on a LAN. The InfluxDB port (`8086`) is closed by default: only open it to point Grafana at it — probes never need it.

The hub carries **its own version**, independent of the app.

#### Backup

The hub backs itself up, Sonarr-style: a complete archive in its backup folder, and restoring means handing the file back. Configured under **Settings → Storage** (interval, archives kept).

⚠️ **An archive contains `secret.key`**, which decrypts notification passwords and webhook URLs. Treat that folder as a secret.

From the command line, for a cron on the host:

```bash
docker exec lanprobe-web lanprobe-web backup     # produces an archive, applies retention
docker exec lanprobe-web lanprobe-web backups    # lists them
```

Restoring is done **with the hub stopped** — that is the only moment it takes effect without a restart.

#### Lost administrator password

This is self-hosted: there is no recovery e-mail. The way back in goes through the container.

```bash
docker exec lanprobe-web lanprobe-web reset-password admin <new-password>
```

The account is re-enabled if it was disabled, and the operation is written to the audit log — this command bypasses sign-in, so it must leave a trace.

---

## 🔧 Building from source

**Prerequisites**

- [Rust](https://rustup.rs/) ≥ 1.85 (edition 2024)
- [Node.js](https://nodejs.org/) ≥ 18
- **Linux desktop:** `libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev`
- **Linux server only:** `libssl-dev pkg-config` (no GUI dependencies)
- **macOS:** `xcode-select --install`
- **Windows:** [WebView2](https://developer.microsoft.com/en-us/microsoft-edge/webview2/) (preinstalled on Windows 11)

```bash
git clone https://github.com/Benjamin-Chianese/lanprobe.git
cd lanprobe
npm install
```

```bash
npm run tauri build                      # desktop app
cargo build --release -p lanprobe-server # headless probe, no GUI
npm run build:web && cargo build --release -p lanprobe-web  # hub
```

⚠️ `npm run build:web` **before** the hub: the interface is embedded in the binary, and a hub built without it starts up and then answers an explicit error in place of the page.

---

## 🛠️ Development

```bash
npm run tauri dev        # desktop, hot reload
npm run dev:web          # hub interface, proxied to a local hub
npm run check            # TypeScript / Svelte, app
npm test                 # svelte-check + Vitest, hub interface
cargo test --workspace   # Rust
```

⚠️ **There is no test CI.** The only workflow is triggered by a tag and merely builds. `cargo test` and `npm test` are **manual** steps, to run before deploying. What each suite covers, and why the interface tests exist at all, is written up in § 20 of the [contract](docs/lanprobe-web-contrat.md).

---

## 🏗️ Technical stack

```
Probes    →  Rust (Tauri 2 · tokio · reqwest) — no HTTP server
Hub       →  Rust (axum · rusqlite · InfluxDB 2)
Interface →  Svelte 5 + TypeScript, two separate frontends
Theme     →  CSS custom properties · dark / light / system · 6 palettes
i18n      →  svelte-i18n — English · French · Spanish, on both sides
Bundles   →  NSIS .exe · .dmg / .pkg · .deb · headless .deb · Docker image
```

### Cargo workspace

```
lanprobe/
├── src-tauri/                  # Tauri shell — commands, app lifecycle
├── crates/
│   ├── lanprobe-core/          # Measurement: ping, discovery, ports, speedtest, SLA
│   ├── lanprobe-server/        # Probe core: scheduler, export buffer, hub attachment
│   └── lanprobe-web/           # Hub: API, SQLite, InfluxDB, backups
├── src/                        # App interface (Svelte 5)
└── web-ui/                     # Hub interface (Svelte 5), embedded in its binary
```

⚠️ `lanprobe-server` is **no longer a server** despite its name: it listens on no port. It carries the measurement scheduler, the export buffer and the hub attachment, shared between the desktop app and the headless binary.

---

## 🖧 Compatibility

| OS | Version | Architecture |
|----|---------|--------------|
| Windows | 10, 11 | x64 |
| macOS | 12 Monterey+ | Intel · Apple Silicon · universal |
| Linux (desktop) | Debian 12+ · Ubuntu 22.04+ | x64 |
| Linux (headless probe) | Debian 11+ · Ubuntu 20.04+ · any systemd distro | x64 |
| Hub | Any machine with Docker | x64 · arm64 |

---

## 🚀 Release

One GitHub Actions workflow builds every platform in parallel and publishes a single GitHub Release:

| Job | Runner | Artifacts |
|-----|--------|-----------|
| `build-linux` | `ubuntu-22.04` | `lanprobe_vX.Y.Z_amd64.deb` |
| `build-linux-server` | `ubuntu-24.04` | `lanprobe-server_vX.Y.Z_amd64.deb` |
| `build-windows` | `windows-latest` | `lanprobe_vX.Y.Z_x64-setup.exe` |
| `build-macos` | `macos-latest` | `universal.dmg` + `universal.pkg` (signed + notarized) |
| `release` | `ubuntu-22.04` | collects artifacts, publishes the Release |

```bash
git tag v2.1.0 && git push origin v2.1.0
```

The version comes from the tag; it is not written in `Cargo.toml`, which stays at `0.0.0`.

---

## 🗺️ Roadmap

- [x] Interface profiles (static IP / DHCP)
- [x] Multi-host ping monitor with latency charts
- [x] Network discovery (CIDR — IP / hostname / MAC)
- [x] TCP port scan, built-in and custom profiles
- [x] SLA — uptime %, avg / min / max / P95 latency, CSV export
- [x] Speed test bound to the selected interface (Ookla + iperf3)
- [x] Dark / light / system theme, 6 palettes
- [x] i18n — English, French, Spanish
- [x] Signed + notarized macOS `.pkg` with sudoers provisioning
- [x] Headless `.deb` with systemd service and capabilities
- [x] **Self-hosted hub** — Docker, embedded InfluxDB, sites, accounts, roles, audit
- [x] **Alerts** — e-mail and webhook, subscription per site and per probe
- [x] **Backup and restore** of the hub, automatic
- [x] **All traffic bound to the selected interface**, heartbeat included
- [x] Removal of the probe's web server — one place to secure
- [ ] The five per-probe tabs in the hub, and remote triggering
- [ ] Per-site scoping on accounts
- [ ] 2FA and passkeys on hub accounts

---

## 🤝 Contributing

Pull requests are welcome. For a significant change, open an issue first to discuss the approach.

---

<div align="center">
<sub>Built with Tauri · Rust · Svelte</sub>
</div>
