# Changelog

Toutes les modifications notables de LanProbe sont documentées ici (EN/FR).
All notable changes to LanProbe are documented here (EN/FR).

Le format suit [Keep a Changelog](https://keepachangelog.com/), avec une section
`### English` et `### Français` par version. SemVer.

## [Unreleased]

### English
- Security: InfluxDB credentials (v2 token, v1 password) are now encrypted at rest in `app_config.json` (AES-256-GCM, fresh nonce per write), and the file is `0600`. Non-secret fields stay readable. Existing cleartext configs are read as-is and encrypted on the next write. The key comes from `LANPROBE_SECRET_KEY` (base64, 32 bytes — nothing is then written to the volume) or from a `0600` `secret.key` in the config dir. With no usable key, the server refuses to write a secret instead of silently storing it in cleartext.
- Docs: added a "Security model" section stating exactly what is protected and from what — passwords are hashed (argon2id) and never encrypted, where the encryption key lives and the limits of the default location, and that TLS beyond the LAN is the operator's reverse proxy job. Documented the setup token in the first-run steps.

### Français
- Sécurité : les identifiants InfluxDB (jeton v2, mot de passe v1) sont désormais chiffrés au repos dans `app_config.json` (AES-256-GCM, nonce tiré à chaque écriture), et le fichier est en `0600`. Les champs non secrets restent lisibles. Une config existante en clair est relue telle quelle puis chiffrée à la première écriture. La clé vient de `LANPROBE_SECRET_KEY` (base64, 32 octets — rien n'est alors écrit sur le volume) ou d'un `secret.key` en `0600` dans le config-dir. Sans clé utilisable, le serveur refuse d'écrire un secret au lieu de le stocker silencieusement en clair.
- Docs : ajout d'une section « Modèle de sécurité » qui dit exactement ce qui est protégé et contre quoi — les mots de passe sont hachés (argon2id) et jamais chiffrés, où vit la clé de chiffrement et les limites de son emplacement par défaut, et que le TLS au-delà du LAN relève du reverse proxy de l'opérateur. Le token de setup est documenté dans les étapes de premier démarrage.

## [1.1.5] - 2026-07-02

### English
- Branding: new LanProbe logo — a radar-sweep mark in the app's indigo accent. Applied across the app UI (sidebar, top bars, login/setup screens), the favicon, and all desktop app icons.

### Français
- Identité : nouveau logo LanProbe — un radar (balayage) dans l'accent indigo de l'app. Appliqué dans toute l'UI (sidebar, barres du haut, écrans de connexion/configuration), le favicon et toutes les icônes desktop.

## [1.1.4] - 2026-06-01

### English
- Network discovery: each device now shows its hardware vendor under the MAC address, resolved offline from an embedded IEEE OUI database (no internet lookup).
- UI: wide tables can now be scrolled horizontally on every layout, instead of being clipped.

### Français
- Découverte réseau : chaque appareil affiche désormais son fabricant sous l'adresse MAC, résolu hors-ligne depuis une base OUI IEEE embarquée (aucun appel internet).
- UI : les tableaux larges peuvent maintenant être faits défiler horizontalement sur tous les layouts, au lieu d'être coupés.

## [1.1.3] - 2026-06-01

### English
- Scheduler: auto-run is now set via an interval dropdown (Off / 5 / 10 / 15 / 30 / 60 min, Off by default) on each probe, instead of a free-text field.
- i18n (FR/EN/ES): added the `scheduler.off` label for the dropdown "Off" option.

### Français
- Scheduler : l'exécution automatique se règle via un menu déroulant d'intervalles (Off / 5 / 10 / 15 / 30 / 60 min, Off par défaut) sur chaque sonde, au lieu d'un champ libre.
- i18n (FR/EN/ES) : ajout du libellé `scheduler.off` pour l'option « Off » du menu.

## [1.1.2] - 2026-05-31

### English
- Updater: update check now points to the official release repo (Ops-Lorva/lanprobe).
- Per-probe schedulers: the "auto-run every N min" control now lives on each probe (Discovery, Port Scan, Speed Test) instead of a global Settings panel.
- Speed Test: added a Cancel button during a running test; the ookla/iperf3 processes are killed on cancel.
- Monitoring: fixed a "ghost ping" where a host removed from monitoring reappeared.
- Port Scan: only open ports are shown now (TCP + UDP), with a "no open port" message otherwise.

### Français
- Updater : la vérification de mise à jour pointe désormais vers le repo de release officiel (Ops-Lorva/lanprobe).
- Schedulers par sonde : le contrôle « exécution auto toutes les N min » est porté par chaque sonde (Discovery, Port Scan, Speed Test) au lieu d'un panneau global dans les Settings.
- Speed Test : ajout d'un bouton Annuler pendant un test ; les process ookla/iperf3 sont tués à l'annulation.
- Monitoring : correction d'un « ping fantôme » où un hôte retiré du monitoring réapparaissait.
- Port Scan : n'affiche plus que les ports ouverts (TCP + UDP), avec un message « aucun port ouvert » sinon.

## [1.1.1] - 2026-05-30

### English
- Scheduler (UI): dark background on number inputs, and removed duplicate CIDR/targets from Settings.

### Français
- Scheduler (UI) : fond sombre sur les champs numériques, et suppression des doublons CIDR/cibles dans les Settings.

## [1.1.0] - 2026-05-30

### English
- Added InfluxDB export and a Scheduler (dedicated configuration panels).
- Fixed reqwest 0.13 compatibility and made `testInflux` save config before testing the connection.

### Français
- Ajout de l'export InfluxDB et d'un Scheduler (panneaux de configuration dédiés).
- Compatibilité reqwest 0.13 corrigée et `testInflux` enregistre la config avant de tester la connexion.

## [1.0.0] - 2026

### English
- First stable cross-platform release (Linux .deb, headless server, Windows NSIS, signed/notarized macOS DMG/PKG).

### Français
- Première version stable multiplateforme (Linux .deb, serveur headless, Windows NSIS, macOS DMG/PKG signés et notarisés).
