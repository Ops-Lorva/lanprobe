# Changelog

Toutes les modifications notables de LanProbe sont documentées ici (EN/FR).
All notable changes to LanProbe are documented here (EN/FR).

Le format suit [Keep a Changelog](https://keepachangelog.com/), avec une section
`### English` et `### Français` par version. SemVer.

## [2.4.2] - 2026-09-02

### English

- **Fixed: a hub that refuses the probe now says so.** When the hub
  answered "403", the app reported a network failure — you went looking
  for a cable while the hub was replying perfectly well. It now separates
  "hub unreachable", which retries on its own, from "token refused by the
  hub", which means the probe was most likely revoked and has to be
  enrolled again.
- ⚠️ Unchanged since 2.4.1: the link state is read for real on Linux only
  — it stays hardcoded on macOS and Windows — and the desktop monitoring
  screen still computes its own rate, without knowing "undetermined".

### Français

- **Corrigé : un hub qui refuse la sonde le dit enfin.** Quand le hub
  répondait « 403 », l'application annonçait une panne réseau — on partait
  chercher un câble alors que le hub répondait très bien. Elle distingue
  désormais « hub injoignable », qui se réessaie tout seul, de « jeton
  refusé par le hub », qui signifie que la sonde a probablement été
  révoquée et qu'il faut la rattacher à nouveau.
- ⚠️ Inchangé depuis la 2.4.1 : l'état du lien n'est lu pour de vrai que
  sur Linux — il reste codé en dur sur macOS et Windows — et l'écran de
  surveillance de l'application de bureau recalcule encore son propre
  taux, sans connaître l'indéterminé.

## [2.4.1] - 2026-09-02

### English

- **Fixed: no more false outages when the interface loses its IPv4
  address.** The probe now writes nothing at all, instead of reporting
  every one of its targets as down.
- **Fixed: removing your last target really clears the list.** An empty
  watch list is sent as empty, so the hub can say "nothing" instead of
  going on showing the old one.
- **Pick the network interface without a screen.** `--interface` chooses
  it, `lanprobe-server interfaces` lists what the machine has. An unknown
  name fails loudly and names the ones that exist.
- **The gateway and the link state are read for real — on Linux.** The
  machine's default gateway is no longer copied onto every interface, and
  an unplugged cable no longer presents itself as an active link.
  ⚠️ On macOS and Windows these two fields are still hardcoded: only Linux
  reads them.
- **A measurement with no verdict is reported as "undetermined."** It used
  to count as an outage, which turned a silence into a fact.
  ⚠️ This holds for the hub's screens and exports. The desktop app's
  monitoring screen still computes its own rate and does not know
  "undetermined" yet — it will follow in a later version.

### Français

- **Corrigé : plus de fausses pannes quand l'interface perd son adresse
  IPv4.** La sonde n'écrit alors plus rien du tout, au lieu de déclarer
  toutes ses cibles en panne.
- **Corrigé : retirer sa dernière cible efface vraiment la liste.** Une
  liste de surveillance vide est émise comme telle, pour que le hub puisse
  dire « rien » au lieu de continuer à montrer l'ancienne.
- **Désigner l'interface réseau sans écran.** `--interface` la choisit,
  `lanprobe-server interfaces` liste celles de la machine. Un nom inconnu
  échoue franchement et nomme celles qui existent.
- **La passerelle et l'état du lien sont lus pour de vrai — sur Linux.**
  La passerelle par défaut de la machine n'est plus recopiée sur toutes
  les interfaces, et un câble débranché ne se présente plus comme un lien
  actif.
  ⚠️ Sur macOS et Windows, ces deux champs restent codés en dur : seul
  Linux les lit.
- **Une mesure sans verdict est rendue « indéterminée ».** Elle comptait
  jusqu'ici pour une panne, ce qui faisait d'un silence un fait.
  ⚠️ Cela vaut pour les écrans et les exports du hub. L'écran de
  surveillance de l'application de bureau recalcule encore son propre taux
  et ne connaît pas encore l'indéterminé — il suivra dans une prochaine
  version.

## [2.3.0] - 2026-09-02

### English

- **The watch list is shared with the hub.** What you add or remove on one
  side reaches the other, and a removal is explicit and dated rather than
  an absence.
- The interface you pick is remembered by the probe itself, not only by
  the screen that set it.

### Français

- **La liste des surveillances est partagée avec le hub.** Ce que vous
  ajoutez ou retirez d'un côté parvient à l'autre, et un retrait est
  explicite et daté au lieu d'être une absence.
- L'interface choisie est mémorisée par la sonde elle-même, et plus
  seulement par l'écran qui l'a réglée.

## [2.2.1] - 2026-09-01

### English

- **Fixed: a network scan no longer made the monitors lie.** A scan in
  progress was enough to make targets look down.
- **Fixed: a host past its timeout is down, not slow.**
- The window no longer freezes at startup.
- The public IP is read again when the gateway changes, when the internet
  comes back, or when the hub asks for it.

### Français

- **Corrigé : un scan réseau ne fait plus mentir les surveillances.** Un
  scan en cours suffisait à faire passer des cibles pour en panne.
- **Corrigé : au-delà de son délai d'attente, un hôte est mort, pas lent.**
- La fenêtre ne gèle plus au démarrage.
- L'IP publique est relevée à nouveau sur changement de passerelle, au
  retour d'internet, ou sur demande du hub.

## [2.2.0] - 2026-08-30

### English

- **The hub's certificate is pinned instead of unchecked.** The
  "accept a self-signed certificate" tick box accepted **any**
  certificate: it did not say "I trust MY hub", it said "I no longer
  check anything" — and the probe's token travels in those requests.
  Now, when the certificate does not verify, the probe reads its
  fingerprint and **shows it**: you compare it with the one your hub
  displays and confirm. From then on, only that certificate is accepted.
  ⚠️ The handshake signature is still verified, unlike before — without
  it, anyone replaying the hub's (public) certificate would pass the pin
  without holding the private key.
  ⚠️ Only what needs pinning is pinned: a certificate that verifies on
  its own is left alone, otherwise a renewal at a public authority would
  cut the probe off every three months.
  ⚠️ `allow_self_signed` is still honoured for probes already carrying
  it — removing it would cut them off remotely and without a word. It is
  simply no longer offered.
- 🔴 **Fixed: measurements were not sent through the hub's client.**
  The export built its own, so anything applied to the hub's client did
  not reach it. Enrolment worked, the heartbeat worked, and every write
  failed — the buffer grew in silence while the app said "attached".
- 🔴 **Fixed: certificate failures were never recognised.** `reqwest`
  reports "error sending request for url …" and hides the real cause in
  its source chain. The "certificate cannot be verified" message was
  therefore unreachable, and users were sent to check a network that was
  perfectly fine.
- Scripted enrolment with a username and password is refused when that
  account requires a second factor: that path has no screen and cannot
  present a code, so it would have made the password alone a valid key
  for an account that just declared otherwise. Use an enrolment code.

### Français

- **Le certificat du hub s'épingle, au lieu de ne plus être vérifié.**
  La case « accepter un certificat auto-signé » acceptait **n'importe
  quel** certificat : elle ne disait pas « je fais confiance à MON hub »,
  elle disait « je ne vérifie plus rien » — et c'est le jeton de la sonde
  qui voyage dans ces requêtes. Désormais, si le certificat ne se vérifie
  pas, la sonde relève son empreinte et **la montre** : vous la comparez
  à celle qu'affiche votre hub, et vous confirmez. Ensuite, seul ce
  certificat-là est accepté.
  ⚠️ La signature du handshake reste vérifiée, contrairement à avant :
  sans ce contrôle, un intermédiaire qui rejoue le certificat du hub — il
  est public — passerait l'épinglage sans posséder la clé privée.
  ⚠️ On n'épingle que ce qui en a besoin : un certificat qui se vérifie
  tout seul est laissé tranquille, sinon un renouvellement chez une
  autorité publique couperait la sonde tous les trois mois.
  ⚠️ `allow_self_signed` reste honoré pour les sondes qui le portent déjà
  — le retirer les couperait du hub à distance et sans un mot. Il n'est
  simplement plus proposé.
- 🔴 **Correction : les mesures ne partaient pas par le client du hub.**
  L'export construisait le sien, donc ce qu'on appliquait au client du
  hub ne l'atteignait pas. L'enrôlement passait, le battement passait, et
  chaque écriture échouait — le tampon grossissait en silence pendant que
  l'application annonçait « rattachée ».
- 🔴 **Correction : un échec de certificat n'était jamais reconnu.**
  `reqwest` affiche « error sending request for url … » et cache la vraie
  cause dans sa chaîne de sources. Le message « certificat non
  vérifiable » était donc inatteignable, et l'utilisateur était envoyé
  vérifier un réseau qui allait parfaitement bien.
- L'enrôlement scripté par identifiants est refusé quand le compte exige
  un second facteur : ce chemin n'a pas d'écran et ne peut pas présenter
  de code, il aurait fait du mot de passe seul une clé valide pour un
  compte qui vient de déclarer le contraire. Utilisez un code
  d'enrôlement.

## [2.1.2] - 2026-08-29

### English

- 🔴 **Fixed: the updater no longer sees any update.** The version parser
  existed **twice** — the shared copy and the desktop's own — and only
  the shared one had learned the new `app-v` tags. 2.1.0 and 2.1.1 both
  carry the broken copy and will **never** offer this release: install it
  by hand once, and updates resume on their own afterwards.
- The probe **backs its profiles up to the hub** — network profiles and
  port-scan profiles alike. They stay a probe-side tool: the hub keeps
  them as an opaque blob, never displays them, includes them in its
  backups, and hands them back **at enrolment**. Re-enrolling a
  reinstalled machine restores them; a key that already exists locally is
  never overwritten.
- The hub can impose the **port list** of a scan. Profiles are resolved
  on the hub side and sent as a plain list, so adding a profile needs no
  probe update.
- The probe reports the **min / average / max latency** of every watched
  target, computed on successful pings only — counting timeouts as zero
  would show an excellent latency for a host that answers half the time.

### Français

- 🔴 **Correction : l'updater ne voyait plus aucune mise à jour.** La
  lecture de version existait **en double** — la copie partagée et celle
  du bureau — et seule la partagée avait appris les tags `app-v`. Les
  2.1.0 et 2.1.1 embarquent la copie cassée et ne proposeront **jamais**
  cette version : l'installer une fois à la main, les mises à jour
  reprennent seules ensuite.
- La sonde **sauvegarde ses profils sur le hub** — profils réseau comme
  profils de scan de ports. Ils restent un outil de la sonde : le hub les
  garde en bloc opaque, ne les affiche pas, les inclut dans ses
  sauvegardes, et les rend **à l'enrôlement**. Ré-enrôler une machine
  réinstallée les restitue ; une clé déjà présente en local n'est jamais
  écrasée.
- Le hub peut imposer la **liste de ports** d'un scan. Les profils sont
  résolus côté hub et envoyés en liste brute : en ajouter un ne demande
  aucune mise à jour de la sonde.
- La sonde remonte la **latence min / moyenne / max** de chaque cible
  surveillée, calculée sur les seuls relevés qui ont abouti — compter les
  absences de réponse comme des zéros afficherait une latence excellente
  pour un hôte qui répond une fois sur deux.

## [2.1.1] - 2026-08-29

### English

The hub gained five per-probe tabs and a client-facing SLA report in
2.1.x; this release is what the probe has to send for them to show
anything. Without it the hub can trigger a scan, but its results never
come back.

- The probe **publishes its inventory** after every discovery, port scan
  and speed test: hosts seen, open ports, throughput results with their
  server. Only OPEN ports are sent — the thousands of closed ones would
  bloat the inventory without teaching anything.
- The probe **reports its internet verdict** at every heartbeat. The
  fleet can now flag a probe that beats perfectly while the link it
  measures is dead — until now that showed as a plain green light.
- The probe **reports what it is actually watching**. ⚠️ The hub used to
  infer watched targets from the measurements in the displayed window, so
  a removed target stayed on screen until its points aged out. Restarting
  the app left ghost watches in the hub.
- The hub can impose the **engine and iperf3 server** of a speed test.
  These overrides do not touch the probe's configuration: the hub asks
  for a test, it does not reconfigure the probe, and the next scheduled
  test stays the one it had planned.
- The speed test **server name** now reaches InfluxDB. "216 Mbit/s" says
  nothing without knowing what it was measured against.

### Français

Le hub a gagné cinq onglets par sonde et un rapport SLA remettable à un
client en 2.1.x ; cette version est ce que la sonde doit envoyer pour
qu'ils aient quelque chose à montrer. Sans elle, le hub sait déclencher
un scan, mais ses résultats ne reviennent jamais.

- La sonde **publie son inventaire** après chaque découverte, scan de
  ports et test de débit : machines vues, ports ouverts, résultats de
  débit avec leur serveur. Seuls les ports OUVERTS partent — les milliers
  de ports fermés gonfleraient l'inventaire sans rien apprendre.
- La sonde **remonte son verdict internet** à chaque battement. Le parc
  peut enfin signaler une sonde qui bat parfaitement alors que le lien
  qu'elle mesure est mort — jusqu'ici, cela s'affichait comme un simple
  voyant vert.
- La sonde **annonce ce qu'elle surveille réellement**. ⚠️ Le hub
  déduisait les cibles des mesures présentes dans la fenêtre affichée :
  une cible retirée restait à l'écran tant que ses points n'en étaient
  pas sortis. Redémarrer l'application laissait des surveillances
  fantômes dans le hub.
- Le hub peut imposer le **moteur et le serveur iperf3** d'un test de
  débit. Ces surcharges ne touchent pas la configuration de la sonde : le
  hub demande un test, il ne la reconfigure pas, et le prochain test
  planifié reste celui qu'elle avait prévu.
- Le **nom du serveur** de speedtest atteint désormais InfluxDB.
  « 216 Mbit/s » ne veut rien dire sans savoir contre quoi.

## [2.1.0] - 2026-08-29

### English

**The probe no longer runs a web server.** It used to serve its own HTTPS
interface on 8443, with its own accounts, setup token and self-signed
certificate. That role belongs to the hub: a probe exposing its own interface
as well would be a second surface to secure for the very same measurements.

⚠️ **Two things will surprise you when upgrading a headless probe:**

1. **`https://<probe>:8443` no longer answers anything.** Attach the probe to
   your hub instead — `lanprobe-server enroll --hub <url> --code <code>`.
2. **`--host` and `--port` are gone.** The package replaces its systemd unit,
   so a standard install upgrades cleanly — but a `systemctl edit` override
   keeps the old flags and **the service will refuse to start**. Remove the
   override, or point it at `lanprobe-server --config-dir … run`.

A headless probe that is not attached to a hub still measures: it piles its
readings into its local buffer and says so in the logs. It loses nothing and
catches up on attachment.

- The headless probe is driven from the command line: `run` (default),
  `enroll`, `status`, `forget`. It listens on no port — nothing to open
  inbound, it only needs to reach the hub outbound.
- **All of a probe's traffic now leaves through the selected interface** —
  measurements, heartbeat, and delivery of readings. Previously the heartbeat
  used the default route: a probe whose measured link had lost internet stayed
  green in the fleet, because its lifeline travelled over another link. You can
  have internet on `eth1` and none on `eth0`, and `eth0` is precisely the one
  under test. A selected interface with no address now makes the heartbeat
  fail, and the probe turns "no news" — which is the truth of that interface.
- Direct InfluxDB export removed from the app for the same reason: it asked
  every probe to know Influx and carry a write token, which is exactly what the
  hub avoids. Everything goes through the hub.
- The headless probe can finally **watch ICMP targets**: that loop only existed
  in the desktop layer.
- Probes execute commands sent by the hub (speed test, port scan, discovery,
  add/remove a monitored target), carried in the heartbeat response.
- The speed test result URL now reaches the hub, which offers a link to the
  public Ookla result.
- Fleet: the "offline" threshold drops from 24 h to 2 h. The threshold decides
  when someone has to travel; a reboot or an update rarely exceeds twenty
  minutes, and a probe silent for two hours is a problem you want to fix the
  same day.
- Hub interface: Spanish, human-readable audit actions, real timestamps in the
  log instead of "2 h ago", public IP column, per-engine speed-test curves, and
  curves that break across gaps instead of drawing a straight line through a
  period nobody measured.
- App: the speed test card no longer clips its upload figure, the hub status
  pill sits next to the title, and the hub address has an http/https toggle.

### Français

**La sonde n'embarque plus de serveur web.** Elle servait sa propre interface
HTTPS sur 8443, avec ses comptes, son jeton de configuration et son certificat
auto-signé. Ce rôle appartient au hub : une sonde qui exposerait la sienne en
plus serait une seconde surface à sécuriser pour montrer les mêmes mesures.

⚠️ **Deux choses vont surprendre à la mise à jour d'une sonde headless :**

1. **`https://<sonde>:8443` ne répond plus rien.** Rattachez la sonde à votre
   hub — `lanprobe-server enroll --hub <url> --code <code>`.
2. **`--host` et `--port` n'existent plus.** Le paquet remplace son unité
   systemd, donc une installation standard se met à jour sans rien faire — mais
   un override posé par `systemctl edit` garde les anciens flags et **le
   service refusera de démarrer**. Retirez l'override, ou pointez-le sur
   `lanprobe-server --config-dir … run`.

Une sonde headless non rattachée mesure quand même : elle empile ses relevés
dans son tampon local et le dit dans les logs. Elle ne perd rien et rattrape
au rattachement.

- La sonde headless se pilote en ligne de commande : `run` (défaut), `enroll`,
  `status`, `forget`. Elle n'écoute sur aucun port — rien à ouvrir en entrée,
  il lui faut seulement joindre le hub en sortie.
- **Tout le trafic d'une sonde sort désormais par l'interface sélectionnée** —
  mesures, battement de cœur et envoi des relevés. Le battement passait avant
  par la route par défaut : une sonde dont le lien mesuré n'avait plus internet
  restait verte dans le parc, parce que sa ligne de vie empruntait un autre
  lien. On peut avoir internet sur `eth1` et pas sur `eth0`, et `eth0` est
  justement celui qu'on éprouve. Une interface sélectionnée sans adresse fait
  maintenant échouer le battement, et la sonde passe « sans nouvelles » — ce qui
  est la vérité de cette interface.
- Export InfluxDB direct retiré de l'application pour la même raison : il
  demandait à chaque sonde de connaître Influx et de porter un jeton
  d'écriture, exactement ce que le hub évite. Tout passe par le hub.
- La sonde headless peut enfin **surveiller des cibles ICMP** : cette boucle
  n'existait que dans la couche desktop.
- Les sondes exécutent les commandes envoyées par le hub (test de débit, scan
  de ports, découverte, ajout/retrait d'une cible surveillée), transportées
  dans la réponse au battement de cœur.
- L'URL du résultat de speedtest remonte au hub, qui propose un lien vers le
  résultat public Ookla.
- Parc : le seuil « hors ligne » passe de 24 h à 2 h. Le seuil décide quand
  quelqu'un doit se déplacer ; un redémarrage ou une mise à jour dépassent
  rarement vingt minutes, et une sonde muette depuis deux heures est un
  problème qu'on veut régler dans la journée.
- Interface du hub : espagnol, actions d'audit en clair, horodatage réel dans
  le journal au lieu de « il y a 2 h », colonne IP publique, courbes de
  speedtest séparées par moteur, et courbes qui se coupent sur les
  interruptions au lieu de tracer une droite au travers d'une période où
  personne ne mesurait.
- Application : la carte de speedtest ne coupe plus le débit montant, la
  pastille de rattachement suit le titre, et l'adresse du hub a un bouton
  http/https.

## [2.0.1] - 2026-08-28

### English
- Public IP is no longer reported when the selected interface has no address —
  the value stayed on screen after switching to a dead interface, describing a
  link the probe was no longer using.
- Hub: `reset-password` command, run from inside the container, for a lost
  administrator password. Self-hosting has no recovery e-mail; the operation is
  written to the audit log because it bypasses sign-in.

### Français
- L'IP publique n'est plus remontée quand l'interface sélectionnée n'a pas
  d'adresse — la valeur restait affichée après un passage sur une interface
  morte, décrivant un lien que la sonde n'empruntait plus.
- Hub : commande `reset-password`, lancée depuis le conteneur, pour un mot de
  passe administrateur perdu. L'auto-hébergement n'a pas d'e-mail de
  récupération ; l'opération est journalisée parce qu'elle contourne la
  connexion.

## [2.0.0] - 2026-08-28

### English
- **The LanProbe hub**: one self-hosted Docker container gathering a fleet of
  probes behind a single address and a single authentication — sites, probes,
  accounts with roles, audit log, e-mail and webhook alerts, automatic backup
  and restore. Probes never talk to InfluxDB directly; the hub relays.
- Probes attach with a short code, valid 15 minutes, used once. No password
  reaches the monitored machine.
- Security: InfluxDB credentials are encrypted at rest in `app_config.json`
  (AES-256-GCM, fresh nonce per write), file mode `0600`. Existing cleartext
  configs are read as-is and encrypted on the next write. The key comes from
  `LANPROBE_SECRET_KEY` (base64, 32 bytes — nothing is then written to the
  volume) or from a `0600` `secret.key`. With no usable key, the probe refuses
  to write a secret rather than silently storing it in cleartext.
- Docs: a "Security model" section stating exactly what is protected and from
  what — passwords are hashed (argon2id) and never encrypted, where the key
  lives and the limits of its default location, and that TLS beyond the LAN is
  the operator's reverse proxy job.

### Français
- **Le hub LanProbe** : un conteneur Docker auto-hébergé qui rassemble un parc
  de sondes derrière une seule adresse et une seule authentification — sites,
  sondes, comptes avec rôles, journal d'audit, alertes e-mail et webhook,
  sauvegarde et restauration automatiques. Les sondes ne parlent jamais
  directement à InfluxDB ; le hub relaie.
- Les sondes se rattachent avec un code court, valable 15 minutes, à usage
  unique. Aucun mot de passe n'atteint la machine surveillée.
- Sécurité : les identifiants InfluxDB sont chiffrés au repos dans
  `app_config.json` (AES-256-GCM, nonce tiré à chaque écriture), fichier en
  `0600`. Une config existante en clair est relue telle quelle puis chiffrée à
  la première écriture. La clé vient de `LANPROBE_SECRET_KEY` (base64, 32
  octets — rien n'est alors écrit sur le volume) ou d'un `secret.key` en
  `0600`. Sans clé utilisable, la sonde refuse d'écrire un secret au lieu de le
  stocker silencieusement en clair.
- Docs : section « Modèle de sécurité » qui dit exactement ce qui est protégé
  et contre quoi — les mots de passe sont hachés (argon2id) et jamais chiffrés,
  où vit la clé et les limites de son emplacement par défaut, et que le TLS
  au-delà du LAN relève du reverse proxy de l'opérateur.

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
