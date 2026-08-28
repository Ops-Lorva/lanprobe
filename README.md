<div align="center">

[🇫🇷 Français](README.fr.md) · 🇬🇧 English

# LanProbe

**Network monitoring & debugging — desktop app and headless server**

*Interface Profiles · Ping Monitor · SLA · Network Discovery · Port Scan · Speed Test · Web Server Mode*

[![Latest Release](https://img.shields.io/github/v/release/Benjamin-Chianese/lanprobe?label=release&style=flat-square)](https://github.com/Benjamin-Chianese/lanprobe/releases/latest)
[![Tauri](https://img.shields.io/badge/Tauri-2.x-FFC131?logo=tauri&logoColor=white&style=flat-square)](https://tauri.app)
[![Rust](https://img.shields.io/badge/Rust-1.85+-CE422B?logo=rust&logoColor=white&style=flat-square)](https://rustlang.org)
[![Svelte](https://img.shields.io/badge/Svelte-5-FF3E00?logo=svelte&logoColor=white&style=flat-square)](https://svelte.dev)
[![Platform](https://img.shields.io/badge/Platform-Windows%20%7C%20macOS%20%7C%20Linux-6366f1?style=flat-square)](#compatibility)
[![License](https://img.shields.io/badge/License-MIT-22c55e?style=flat-square)](LICENSE)

</div>

---

## 🖧 What is LanProbe?

LanProbe replaces a handful of separate network utilities with one coherent interface. Built for engineers who switch between interfaces frequently, debug connectivity issues, or need to monitor multiple hosts at once.

- 🔀 **Switch network profiles in one click** — no more typing static IPs into system dialogs
- 📡 **Watch multiple hosts in real time** with latency history and SLA statistics
- 🔍 **Scan your network** to discover who's on the subnet
- ⚡ **Test throughput** bound to a specific interface — no OS routing surprises
- 🗄️ **Deploy headless** on a Debian server or Raspberry Pi — access the full UI from any browser on the LAN

---

## 🧩 Features

| Module | What it does |
|--------|-------------|
| 🔀 **Network Profiles** | Save named static-IP or DHCP configurations, apply them in one click |
| 📡 **Ping Monitor** | Continuous ICMP monitoring of multiple hosts, real-time latency graph, configurable alert thresholds |
| 📊 **SLA Export** | Per-host uptime %, avg / min / max / P95 latency — exportable to CSV |
| 🔍 **Network Discovery** | Fast async CIDR scan returning IP, hostname and MAC address of live hosts |
| 🔌 **Port Scan** | TCP scan with built-in profiles (common, web, full) and custom profiles |
| ⚡ **Speed Test** | Ookla CLI speed test bound to the selected interface via `IP_BOUND_IF` / `SO_BINDTODEVICE` |
| 🌐 **Web Server Mode** | Expose the full LanProbe UI over HTTPS on the LAN — desktop app or standalone headless binary |
| 🛡️ **Internet Status** | Dual-probe (ICMP + HTTP) internet health with public-IP info and uptime percentage |
| 🎨 **Color Palettes** | 6 accent palettes (Indigo, Cyan, Emerald, Rose, Amber, Slate) — dark and light mode |

---

## 📦 Installation

### 💻 Desktop app

Pre-built installers are published on **[GitHub Releases](https://github.com/Benjamin-Chianese/lanprobe/releases/latest)**.

| OS | File | Notes |
|----|------|-------|
| Windows 10 / 11 | `lanprobe_vX.Y.Z_x64-setup.exe` | NSIS installer, UAC required for network config |
| macOS (Intel + Apple Silicon) | `lanprobe_vX.Y.Z_universal.pkg` | Signed + notarized, provisions sudoers entry automatically |
| macOS (Intel + Apple Silicon) | `lanprobe_vX.Y.Z_universal.dmg` | Drag-to-Applications, password prompted on first profile apply |
| Linux (Debian / Ubuntu) | `lanprobe_vX.Y.Z_amd64.deb` | Desktop app with WebKit2GTK |

The app ships an **auto-updater** — subsequent updates are one click from the notification banner.

> **macOS** — use the `.pkg` installer for the smoothest experience: it signs the sudoers entry so applying network profiles never prompts for a password.

---

### 🗄️ Headless server on Debian / Ubuntu (no GUI required)

`lanprobe-server` is a standalone binary that serves the full LanProbe web UI over HTTPS. It requires no desktop environment and runs as a systemd service.

#### 1 — Install or update with the one-liner

```bash
curl -fsSL https://raw.githubusercontent.com/Benjamin-Chianese/lanprobe/main/install-server.sh | sudo bash
```

The script automatically fetches the latest release, installs the `.deb`, and restarts the service if it was already running.
To install a specific version:

```bash
curl -fsSL https://raw.githubusercontent.com/Benjamin-Chianese/lanprobe/main/install-server.sh | sudo bash -s -- --version v0.6.10
```

Or download and run locally:

```bash
curl -fsSL -o install-server.sh https://raw.githubusercontent.com/Benjamin-Chianese/lanprobe/main/install-server.sh
sudo bash install-server.sh
```

The package automatically:
- Creates a dedicated `lanprobe` system user
- Sets `CAP_NET_RAW` + `CAP_NET_ADMIN` on the binary (ICMP + interface config without running as root)
- Registers and starts `lanprobe-server.service` via systemd

#### 2 — Check the service

```bash
sudo systemctl status lanprobe-server
# Listening on https://0.0.0.0:8443 by default

# Follow logs
sudo journalctl -u lanprobe-server -f
```

#### 3 — First-time setup

On first access the UI shows a **setup screen** where you create the admin username and password. Open a browser on any machine on the same LAN:

```
https://<server-ip>:8443
```

The setup screen also asks for a **setup token**, generated on first boot and readable on the server:

```bash
sudo cat /var/lib/lanprobe/setup-token
# or: sudo journalctl -u lanprobe-server | grep -i token
```

> **Why a token?** Without it, anyone who reaches the instance between service start and your first login could claim the admin account. Once the account exists, the token is consumed, the setup route is closed **server-side** for good, and `/api/setup` answers `409` on every later attempt — there is no "re-run setup" path.

> The server generates a self-signed TLS certificate on first boot. Your browser will show a security warning — accept the exception. The cert is stored in `/var/lib/lanprobe/`.

#### 4 — Open the firewall (if needed)

```bash
sudo ufw allow 8443/tcp
```

#### ⚙️ Configuration

The service file at `/lib/systemd/system/lanprobe-server.service` passes these defaults:

```
--host 0.0.0.0   # listen on all interfaces
--port 8443      # HTTPS port
--config-dir /var/lib/lanprobe   # users + TLS cert storage
```

To change the port, edit the service and reload:

```bash
sudo systemctl edit lanprobe-server
# Add:
# [Service]
# ExecStart=
# ExecStart=/usr/bin/lanprobe-server --host 0.0.0.0 --port 9443 --config-dir /var/lib/lanprobe

sudo systemctl daemon-reload && sudo systemctl restart lanprobe-server
```

#### Update

```bash
curl -LO https://github.com/Benjamin-Chianese/lanprobe/releases/latest/download/lanprobe-server_vX.Y.Z_amd64.deb
sudo dpkg -i lanprobe-server_vX.Y.Z_amd64.deb
# dpkg stops, replaces, and restarts the service automatically
```

#### Uninstall

```bash
sudo apt remove lanprobe-server
```

#### 🔒 Security model — what is protected, and from what

LanProbe is **self-hosted only**. There is no central service, no account directory and no relay: your account and your data live in *your* instance. That also means the security boundary is yours to set. Here is the exact measure, without overselling it.

| Data | At rest | Notes |
|---|---|---|
| User passwords (`users.json`) | **Hashed**, argon2id + per-user salt | Never encrypted — a reversible password is a flaw, not a feature. File is `0600`. |
| InfluxDB token / password (`app_config.json`) | **Encrypted**, AES-256-GCM, fresh nonce per write | File is `0600`. Non-secret fields (URL, org, bucket, username) stay readable so you can diagnose an instance. |
| Session tokens | In memory only | 32 random bytes, 7-day TTL, `HttpOnly` + `SameSite=Strict` + `Secure` cookie. Lost on restart, by design. |

**Where the encryption key lives — read this before trusting it.** By default the key is `secret.key` (`0600`) in the same `--config-dir` as `app_config.json`. Be clear-eyed about what that buys you: it protects a **copy of the config file alone** — a backup that leaks, an exported volume, a config pasted into a bug report. It does **not** protect against someone who already has the machine or the whole volume: they read both files.

For a real gain, keep the key off the volume by passing it in the environment:

```bash
# 32 random bytes, base64
openssl rand -base64 32

# systemd drop-in, Docker secret, or your secret manager
sudo systemctl edit lanprobe-server
# [Service]
# Environment=LANPROBE_SECRET_KEY=<base64-32-bytes>
```

When `LANPROBE_SECRET_KEY` is set, **no key file is ever written**. Keep a copy of that key: lose it and the stored InfluxDB credentials become unreadable and must be re-entered. If no usable key is available, the server **refuses to write a secret** rather than silently downgrading it to cleartext.

**Transport.** The built-in HTTPS uses a **self-signed** certificate — enough for a LAN, not a substitute for a real one. If you expose the instance beyond your LAN, put it behind your own reverse proxy (Caddy, Traefik, nginx) and let that terminate TLS with a trusted certificate. LanProbe does not obtain or renew certificates for you.

---

### 🐳 Self-hosted Web Hub (Docker) — for a fleet of probes

🚧 **In development**, built against [`docs/lanprobe-web-contrat.md`](docs/lanprobe-web-contrat.md) — the Docker packaging below is built and tested; the hub binary and its web UI land in separate branches and are not merged yet.

Got more than one probe? The hub is a **single Docker container** — the enrollment API and its own embedded InfluxDB — that your probes report to. **We host nothing**: it runs on a machine you control, end to end.

#### Get started

Nothing to edit — clone, start, and open the browser:

```bash
git clone https://github.com/Benjamin-Chianese/lanprobe.git && cd lanprobe
docker compose up -d
docker compose logs lanprobe-web | grep -i "setup token"
```

Open `https://<this-machine>:8443` (self-signed certificate — accept the browser warning, same as the headless server above) and use that setup token to create the admin account. It is consumed on first use and never shown again.

**Everything else lives in the app, not in `docker-compose.yml`.** Once the admin account exists, the Influx organization, bucket, retention, and the URL advertised to probes are all settings you change from the interface — they persist in the hub's own SQLite database, survive a redeploy, and never require touching a `.env` file or restarting the container. The hub already starts pre-configured against the InfluxDB it just initialized; there's a **test button** next to the advertised URL setting if you ever remap the published port and want to confirm probes can actually reach it. Lowering the retention setting deletes the data outside the new window — the UI asks for confirmation and names the amount of history you're about to lose.

#### Enroll a probe

Point a probe (desktop app or `lanprobe-server`) at the hub's URL with your admin username and password. The hub hands back a probe-specific token and Influx write coordinates restricted to that one bucket — a compromised probe can write, but it can't read or erase anyone else's measurements. See the [interface contract](docs/lanprobe-web-contrat.md) for the exact exchange.

#### Back up

Two volumes, two different rhythms — never remove one without the other, they're two halves of the same state:

| Volume | Contains | Backup cadence |
|---|---|---|
| `lanprobe_data` | Admin account, probe tokens (hashed), setup token, hub's TLS cert | Small — back up often, it's cheap |
| `influxdb_data` | Time-series measurements | Can reach several GB — back up on whatever retention schedule suits your data |

```bash
docker compose stop lanprobe-web
docker run --rm -v lanprobe_data:/from -v "$PWD":/to alpine tar czf /to/lanprobe_data.tar.gz -C /from .
docker run --rm -v influxdb_data:/from -v "$PWD":/to alpine tar czf /to/influxdb_data.tar.gz -C /from .
docker compose start lanprobe-web
```

> The container never deletes or overwrites its own data — not on restart, not on a half-finished setup. If InfluxDB and the hub's token ever get out of sync (an interrupted first boot), `docker logs lanprobe-web` explains exactly what happened and what to do; it never guesses on your behalf.

---

### 🌐 Web Server Mode (desktop app)

The desktop app can also act as a server — enable it from **Settings → Server Mode**. This streams live data from your desktop machine to any browser on the LAN without installing a separate package.

---

## 🔧 Build from source

**Prerequisites**

- [Rust](https://rustup.rs/) ≥ 1.85 (edition 2024)
- [Node.js](https://nodejs.org/) ≥ 18
- **Linux desktop:** `libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev`
- **Linux server only:** `libssl-dev pkg-config` (no GUI deps needed)
- **macOS:** `xcode-select --install`
- **Windows:** [WebView2](https://developer.microsoft.com/en-us/microsoft-edge/webview2/) (pre-installed on Windows 11)

```bash
git clone https://github.com/Benjamin-Chianese/lanprobe.git
cd lanprobe
npm install
```

```bash
# Desktop app (Tauri)
npm run tauri build

# Headless server binary only (no GUI deps)
npm run build                          # build frontend (embedded in server binary)
cargo build -p lanprobe-server --release
# Binary → target/release/lanprobe-server
```

---

## 🛠️ Development

```bash
# Hot-reload desktop dev mode
npm run tauri dev

# TypeScript / Svelte type check
npm run check

# Rust unit tests
cargo test -p lanprobe-core

# Run headless server locally
npm run build
cargo run -p lanprobe-server -- --host 0.0.0.0 --port 8443
```

---

## 🏗️ Tech Stack

```
Backend   →  Rust (Tauri 2 · tokio · reqwest · axum)
Frontend  →  Svelte 5 + TypeScript
Icons     →  Inline SVG (Lucide-style, no runtime dep)
Theme     →  CSS custom properties · OS-adaptive + manual override · 6 palettes
Storage   →  JSON via tauri-plugin-store (desktop) / /var/lib/lanprobe (server)
i18n      →  svelte-i18n — English · French · Spanish
Bundles   →  .exe NSIS · .dmg / .pkg · .deb · headless .deb
```

### Crate workspace

```
lanprobe/
├── src-tauri/                  # Tauri shell — command registration, app lifecycle
├── crates/
│   ├── lanprobe-core/          # Shared async logic: ping, discovery, ports, speedtest, SLA
│   └── lanprobe-server/        # Standalone headless HTTPS server (served UI over LAN)
└── src/                        # Svelte 5 frontend (embedded in both desktop and server)
    └── lib/
        ├── components/         # One component per module
        ├── stores/             # Svelte stores (profiles, monitoring, settings)
        └── i18n/               # en / fr / es translation files
```

---

## 🖧 Compatibility

| OS | Version | Architecture |
|----|---------|--------------|
| Windows | 10, 11 | x64 |
| macOS | 12 Monterey+ | Intel · Apple Silicon · universal |
| Linux (desktop) | Debian 12+ · Ubuntu 22.04+ | x64 |
| Linux (server) | Debian 11+ · Ubuntu 20.04+ · any systemd distro | x64 |

---

## 🚀 CI / Release Pipeline

One GitHub Actions workflow builds all platforms in parallel and publishes a single GitHub Release:

| Job | Runner | Artifacts |
|-----|--------|-----------|
| `build-linux` | `ubuntu-22.04` | `lanprobe_vX.Y.Z_amd64.deb` |
| `build-linux-server` | `ubuntu-24.04` | `lanprobe-server_vX.Y.Z_amd64.deb` |
| `build-windows` | `windows-latest` | `lanprobe_vX.Y.Z_x64-setup.exe` |
| `build-macos` | `macos-latest` | `universal.dmg` + `universal.pkg` (signed + notarized) |
| `release` | `ubuntu-22.04` | collects all artifacts · publishes GitHub Release |

Cut a release by pushing a version tag:

```bash
git tag v1.0.0 && git push origin v1.0.0
```

---

## 🗺️ Roadmap

- [x] Network profile management (static IP / DHCP)
- [x] Real-time multi-host ping monitor with latency graphs
- [x] Network discovery (CIDR scan — IP / hostname / MAC)
- [x] TCP port scan with built-in and custom profiles
- [x] SLA monitoring — uptime %, avg / min / max / P95 latency
- [x] SLA export to CSV
- [x] Speed test bound to selected interface (Ookla + iperf3)
- [x] Glass & Depth UI — dark / light / system theme
- [x] Web server mode — share UI over HTTPS on the LAN
- [x] Headless `.deb` with systemd service, capabilities, auto-start
- [x] i18n — English, French, Spanish
- [x] macOS signed + notarized `.pkg` with sudoers provisioning
- [x] 6 color palettes (dark + light)
- [ ] Self-hosted Web Hub — Docker, embedded InfluxDB, multi-probe fleet enrollment

---

## 🤝 Contributing

Pull requests are welcome. For significant changes please open an issue first to discuss the approach.

---

<div align="center">
<sub>Built with Tauri · Rust · Svelte</sub>
</div>
