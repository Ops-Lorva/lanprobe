# Changelog — hub

Modifications notables du hub LanProbe (image Docker `ghcr.io/ops-lorva/lanprobe-web`, tags `hub-X.Y.Z`).
Notable changes to the LanProbe hub (Docker image `ghcr.io/ops-lorva/lanprobe-web`, `hub-X.Y.Z` tags).

La sonde (application de bureau et sonde headless) a son propre journal : [CHANGELOG.md](CHANGELOG.md).
The probe (desktop app and headless probe) has its own log: [CHANGELOG.md](CHANGELOG.md).

Le format suit [Keep a Changelog](https://keepachangelog.com/), avec une section
`### English` et `### Français` par version. Les versions antérieures à 1.6.0
sont décrites dans les [releases GitHub](https://github.com/Ops-Lorva/lanprobe/releases).

## [1.6.14] - 2026-11-16

### English

- **Changed: new interface design** for the whole hub: fleet with a 24 h heartbeat strip, probe page, readable audit log, accounts with every account's paired devices, notifications and account pages.
- **Added: "Add a probe" page** (enrolment code per site), replacing the packages screen.
- **Added: SLA report page per site** with a preview (summary, outages, SLA per public IP) before downloading the workbook.
- **Added: archive a probe** from its page, and restore it to the fleet; archived probes carry a badge.
- **Fixed: a probe page left open could overload the hub.** Background refreshes no longer pile up, metrics queries are about 30× faster, and wide windows (7 and 30 days) refresh less often.
- **Added: server-side limits on InfluxDB reads**: 2 at a time (503 with `Retry-After` beyond), 45 s maximum, and at most 400 MB of memory per query.
- **Fixed: no more flickering** of the fleet and probe pages on refresh.
- **Fixed: every audit log action is translated.**
- **Security: a failed webhook test no longer shows the webhook URL.**

### Français

- **Modifié : nouvelle interface** pour tout le hub : parc avec une bande de battement sur 24 h, fiche sonde, journal lisible, comptes avec les appareils de tous les comptes, notifications et mon compte.
- **Ajouté : page « Ajouter une sonde »** (code d'enrôlement par site), qui remplace l'écran des paquets.
- **Ajouté : page Rapport SLA par site**, avec un aperçu (synthèse, coupures, SLA par IP publique) avant de télécharger le classeur.
- **Ajouté : archiver une sonde** depuis sa fiche, et la remettre dans le parc ; une sonde archivée porte un badge.
- **Corrigé : une fiche sonde laissée ouverte pouvait saturer le hub.** Les relectures de fond ne s'empilent plus, les requêtes de mesures sont environ 30 fois plus rapides, et les fenêtres larges (7 et 30 jours) se relisent moins souvent.
- **Ajouté : limites côté serveur sur les lectures InfluxDB** : 2 à la fois (503 avec `Retry-After` au-delà), 45 s au plus, et 400 Mo de mémoire au plus par requête.
- **Corrigé : plus de clignotement** du parc et de la fiche sonde à chaque relecture.
- **Corrigé : toutes les actions du journal sont traduites.**
- **Sécurité : l'échec d'un essai de webhook n'affiche plus l'URL du webhook.**

## [1.6.13] - 2026-11-16

### English

- **Security: an account restricted to some sites no longer sees the others** when it has none assigned; access to every site is now an explicit setting.
- **Security: closed a query injection in probe metrics**, and InfluxDB errors are no longer relayed to non-admins.
- **Security: rate limiting on login, pairing and enrolment**, counted before the password check, with at most 4 password checks at once.
- **Security: a paired phone can no longer change the account's password, passkeys or TOTP, nor administer the hub** (accounts, backups, settings, devices); these require a web session.
- **Security: a restricted account can no longer create a site, enrol a probe outside its sites, or change the hub settings.**
- **Added: report requests are limited per account** (one at a time, 20 per hour).
- **Added: demo accounts**, read-only, whose paired devices are revoked after 7 days.
- **Fixed: behind a reverse proxy, the app receives the hub's public HTTPS address** and no certificate fingerprint to pin.
- **Changed: the sample compose file publishes the hub on 127.0.0.1 only.**

### Français

- **Sécurité : un compte restreint à des sites ne voit plus les autres** quand aucun ne lui est attribué ; l'accès à tous les sites devient un réglage explicite.
- **Sécurité : fermeture d'une injection de requête dans les mesures des sondes**, et les erreurs d'InfluxDB ne sont plus relayées aux non-admins.
- **Sécurité : limitation des tentatives de connexion, d'appairage et d'enrôlement**, décomptées avant la vérification du mot de passe, avec 4 vérifications simultanées au plus.
- **Sécurité : un téléphone appairé ne peut plus changer le mot de passe, les clés d'accès ou le TOTP du compte, ni administrer le hub** (comptes, sauvegardes, réglages, appareils) : il faut une session web.
- **Sécurité : un compte restreint ne peut plus créer de site, enrôler une sonde hors de ses sites, ni modifier les réglages du hub.**
- **Ajouté : les demandes de rapport sont limitées par compte** (une à la fois, 20 par heure).
- **Ajouté : comptes de démonstration**, en lecture seule, dont les appareils appairés sont révoqués au bout de 7 jours.
- **Corrigé : derrière un reverse proxy, l'app reçoit l'adresse publique HTTPS du hub** et aucune empreinte de certificat à épingler.
- **Modifié : le compose d'exemple publie le hub sur 127.0.0.1 seulement.**

## [1.6.12] - 2026-10-02

### English

- **Added: machines scanned with no open port now show up**, on screen and in the SLA workbook.
- **Changed: a port scan is grouped by machine**, not by port.

### Français

- **Ajouté : les machines scannées sans aucun port ouvert apparaissent**, à l'écran et dans le classeur SLA.
- **Modifié : un scan de ports se lit par machine**, et non plus par port.

## [1.6.11] - 2026-10-02

### English

- **Fixed: downloading a backup archive without a password.**
- **Changed: "My account" has its own address.**

### Français

- **Corrigé : le téléchargement d'une archive de sauvegarde sans mot de passe.**
- **Modifié : « Mon compte » a sa propre adresse.**

## [1.6.10] - 2026-10-02

### English

- **Added: the SLA workbook's ports tab shows each machine's scan profile and date**, from the merged port inventory.

### Français

- **Ajouté : l'onglet des ports du classeur SLA montre le profil et la date de scan de chaque machine**, à partir de l'inventaire fusionné.

## [1.6.9] - 2026-10-02

### English

- **Changed: the port inventory is merged machine by machine** instead of being replaced by the latest scan.
- **Added: open ports are counted by family**, not only in total.

### Français

- **Modifié : l'inventaire des ports se fusionne machine par machine** au lieu d'être remplacé par le dernier scan.
- **Ajouté : les ports ouverts sont comptés par famille**, plus seulement au total.

## [1.6.8] - 2026-10-02

### English

- **Added: every backup archive can be downloaded**, optionally sealed with AES-256.
- **Added: a test button on each notification channel.**
- **Changed: "My account" moves to the side bar.**

### Français

- **Ajouté : chaque archive de sauvegarde se télécharge**, scellée en AES-256 au choix.
- **Ajouté : un bouton de test sur chaque canal de notification.**
- **Modifié : « Mon compte » passe dans la barre de gauche.**

## [1.6.7] - 2026-10-02

### English

- **Added: UDP in scan profiles**, and each scan command says which profile it comes from.
- **Added: the probe's built-in scan profiles are seeded on the hub.**
- **Changed: the audit log names the target, and scheduled backups are signed "system".**

### Français

- **Ajouté : l'UDP dans les profils de scan**, et chaque commande de scan dit de quel profil elle vient.
- **Ajouté : les profils de scan de base de la sonde sont semés sur le hub.**
- **Modifié : le journal d'audit nomme la cible, et une sauvegarde planifiée est signée « système ».**

## [1.6.6] - 2026-10-01

### English

- **Added: shared port-scan profiles**, managed in the hub and synchronised with probes at each heartbeat.
- **Added: the hub shows where each command came from** (hub or paired device).

### Français

- **Ajouté : profils de scan de ports partagés**, gérés dans le hub et synchronisés avec les sondes à chaque battement.
- **Ajouté : le hub montre d'où part chaque commande** (le hub ou un appareil appairé).

## [1.6.5] - 2026-09-30

### English

- **Added: a probe running an outdated version is highlighted**, against the latest release published on GitHub.

### Français

- **Ajouté : une sonde en retard de version est signalée**, par rapport à la dernière version publiée sur GitHub.

## [1.6.4] - 2026-09-03

### English

- **Changed: the interface language follows the account, and the report language follows the site.**

### Français

- **Modifié : la langue de l'interface suit le compte, et celle du rapport suit le site.**

## [1.6.3] - 2026-09-03

### English

- **Fixed: the "armed" colour no longer changes with the accent palette.**

### Français

- **Corrigé : la couleur de l'état « armé » ne change plus avec la palette d'accent.**

## [1.6.2] - 2026-09-03

### English

- **Added: the hub serves the probe packages**, for download where they are needed.
- **Changed: a revoked device is hidden, like an archived site.**

### Français

- **Ajouté : le hub distribue les paquets de la sonde**, à télécharger là où on en a besoin.
- **Modifié : un appareil révoqué est masqué, comme un site archivé.**

## [1.6.1] - 2026-09-03

### English

- **Fixed: the Docker image now includes the translation catalogues.**

### Français

- **Corrigé : l'image Docker embarque les catalogues de traduction.**

## [1.6.0] - 2026-09-03

### English

- **Added: pairing a phone** with the LanProbe iOS app, by QR code with a pinned certificate fingerprint; device identity with no expiry, revoked explicitly.
- **Added: SLA workbooks produced by the hub**, with charts that show outages, report history and purge.

### Français

- **Ajouté : appairage d'un téléphone** avec l'app iOS LanProbe, par QR avec empreinte de certificat épinglée ; identité d'appareil sans expiration, révoquée explicitement.
- **Ajouté : classeurs SLA produits par le hub**, avec des graphiques qui montrent les coupures, un historique des rapports et leur purge.
