# Changelog

Toutes les modifications notables de LanProbe sont documentées ici (EN/FR).
All notable changes to LanProbe are documented here (EN/FR).

Le format suit [Keep a Changelog](https://keepachangelog.com/), avec une section
`### English` et `### Français` par version. SemVer.

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
