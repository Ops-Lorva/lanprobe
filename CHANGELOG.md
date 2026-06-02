# Changelog

Toutes les modifications notables de LanProbe sont documentées dans ce fichier.

Le format suit [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/),
et le projet adhère au [Semantic Versioning](https://semver.org/lang/fr/).

## [1.1.3] - 2026-06-01

### Ajouté
- **Scheduler — select d'intervalles** : l'exécution automatique se règle
  désormais via un menu déroulant (`Off` / 5 / 10 / 15 / 30 / 60 min, `Off`
  par défaut) au lieu d'un champ libre, sur chaque sonde.

### Changé
- i18n FR/EN/ES : ajout de `scheduler.off` pour l'option « Off » du select.

## [1.1.2] - 2026-05-31

### Corrigé
- **Updater** : la vérification de mise à jour pointe vers `Ops-Lorva/lanprobe`
  (repo de release officiel).
- **Monitoring — ping fantôme** : un hôte retiré du monitoring réapparaissait.
  Purge de l'historique au stop (`clear_ip()`) + tombstone de 3 s côté store.
- **Port Scan** : n'affiche plus que les ports ouverts (TCP + UDP), message
  « aucun port ouvert » sinon.

### Ajouté
- **Schedulers au niveau des sondes** : contrôle « exécution auto toutes les
  N min » porté par chaque sonde (Discovery, Port Scan, Speed Test) via
  `SchedulerControl.svelte`, retiré du panneau global des Settings.
- **Annulation du Speed Test** : bouton Annuler pendant un test ; les process
  `ookla`/`iperf3` sont tués à l'annulation (`kill_on_drop`, `tokio::select!`,
  commande `cmd_cancel_speedtest`).

## [1.1.1] - 2026-05-30

### Corrigé
- **Scheduler (UI)** : fond sombre sur les champs numériques et suppression des
  doublons CIDR/cibles dans les Settings.

## [1.1.0] - 2026-05-30

### Ajouté
- Export **InfluxDB** et **Scheduler** (panneaux de configuration dédiés).

### Corrigé
- Compatibilité `reqwest` 0.13 (paramètres de requête dans l'URL, `AppState`
  cloné pour `test_connection`).
- `testInflux` sauvegarde la config avant de tester la connexion.

## [1.0.0] - 2026

Première version stable multiplateforme (Linux `.deb`, serveur headless,
Windows NSIS, macOS DMG/PKG signés et notarisés).
