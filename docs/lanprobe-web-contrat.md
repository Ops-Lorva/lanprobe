# LanProbe Web — contrat d'interface

Document de référence partagé entre le **hub** (`crates/lanprobe-web`, auto-hébergé
en conteneur) et la **sonde** (`crates/lanprobe-server` + client desktop Tauri).

Toute divergence entre les deux implémentations se règle ici, pas dans le code.

---

## 1. Vue d'ensemble

```
┌────────────────┐   1. enrôlement (HTTPS, compte admin)   ┌──────────────────┐
│  sonde         │ ─────────────────────────────────────▶  │  lanprobe-web    │
│  (desktop ou   │ ◀───────  jeton + coordonnées Influx ──  │  (conteneur)     │
│   headless)    │                                          │                  │
│                │   2. battement de cœur (jeton sonde)     │  SQLite : users, │
│                │ ─────────────────────────────────────▶  │  probes          │
│                │                                          │                  │
│                │   3. mesures — écriture DIRECTE          │  InfluxDB 2      │
│                │ ─────────────────────────────────────────────▶ (embarqué)   │
└────────────────┘        line protocol, port dédié         └──────────────────┘
```

**Pourquoi trois flux et pas un seul.** Les mesures ne transitent pas par le hub :
elles vont droit dans Influx, qui est fait pour ça. Mais une sonde éteinte
n'écrit rien — et « rien » ressemble exactement à « tout va bien » dans une base
de séries temporelles. Le battement de cœur est ce qui distingue une sonde
silencieuse d'une sonde absente. Sans lui, l'interface afficherait un parc vert
pendant qu'une sonde est débranchée depuis trois jours.

## 2. Sites et identité d'une sonde

### Sites

Les sondes sont groupées par **site**, sur le modèle Ubiquiti : un seul niveau de
regroupement, pas de hiérarchie client → site.

En pratique le site correspond au client, et les sondes portent les lieux : un
site « Durand » contenant les sondes « Paris », « Lyon », « Entrepôt ». Rien ne
l'impose — le nom du site est libre — mais c'est la lecture qui marche quand on
gère plusieurs clients depuis un seul hub.

**Un seul bucket InfluxDB pour tous les sites**, le site étant un *tag* sur les
points. Un bucket Influx porte avant tout une politique de rétention — ce n'est
pas une frontière de sécurité exploitable ici, puisque les jetons de sonde sont
en **écriture seule** et que le jeton de lecture ne quitte jamais le hub. Le
contrôle d'accès est fait par le hub, en SQL, pas par le découpage Influx.

Découper en un bucket par site imposerait un `union()` sur N buckets dès qu'on
veut une vue « tous les sites », et rendrait les variables Grafana pénibles. La
seule raison légitime d'y revenir serait une **rétention différente par site** —
on ajouterait alors un bucket dédié à ce site sans rien casser. L'inverse
(partir en buckets et vouloir revenir) serait une migration de données.

### Identité d'une sonde

| Champ | Rôle | Modifiable |
|---|---|---|
| `probe_id` | identité technique, UUIDv4, généré par le hub | non |
| `name` | étiquette lisible, saisie par l'utilisateur | oui |
| `site_id` | site d'appartenance | oui (déplacement) |
| `token` | authentifie la sonde auprès du hub | rotation possible |

Le `name` est **une étiquette, pas une identité**. Le renommer ne casse pas
l'historique : les points Influx sont tagués `probe_id`, et le hub résout le nom
à l'affichage. Un parc où l'on ne peut pas renommer une sonde sans perdre ses
courbes est un parc que personne ne renomme.

Le nom d'une sonde est unique **au sein d'un site**, pas globalement : deux sites
ont chacun le droit d'avoir leur « Baie RDC ».

### Tags écrits sur les points

| Tag | Stabilité | À quoi il sert |
|---|---|---|
| `probe_id` | stable | clé de jointure — ne change jamais |
| `site_id` | stable | filtrage par site, résiste au renommage |
| `probe` | change au renommage | lisibilité directe dans Grafana |
| `site` | change au renommage | lisibilité directe dans Grafana |
| `host` | — | conservé pour les tableaux de bord existants |

On écrit **à la fois l'identifiant et le nom**, en connaissance de cause : un
renommage crée une nouvelle série pour les tags lisibles, et les points antérieurs
gardent l'ancien nom. Le coût en cardinalité est négligeable (quelques renommages
sur la vie d'un parc) et le bénéfice est réel : les jointures restent stables sur
les identifiants, et quelqu'un qui branche Grafana directement sur le bucket lit
des noms plutôt que des UUID. Interroger toujours sur `probe_id` / `site_id`
quand on veut l'historique complet d'une sonde.

## 3. Endpoints du hub

Toutes les réponses d'erreur : `{ "error": "<message lisible>" }` avec le code HTTP adéquat.

### `GET /api/status` — public
```json
{ "needs_setup": true, "version": "1.2.0" }
```
Seule route accessible sans authentification quand `needs_setup` est vrai.

### `POST /api/setup` — création de l'admin initial
```json
{ "setup_token": "...", "username": "admin", "password": "..." }
```
Le `setup_token` est généré au démarrage tant qu'aucun compte n'existe, écrit en
`0600` dans le volume et affiché dans les logs du conteneur (`docker logs`).
Non rejouable : consommé au premier succès.

### `POST /api/login` → cookie de session `HttpOnly`
### `POST /api/logout`

### `GET /api/sites` — authentification : session **ou** identifiants du compte
La sonde l'appelle avant l'enrôlement pour proposer une liste déroulante plutôt
qu'un champ libre — c'est ce qui évite qu'un « Durnad » se glisse dans le parc.
```json
[{ "site_id": "…", "name": "Durand", "probe_count": 3 }]
```

### `POST /api/sites` — `{ "name": "Durand" }` → `201`. `409` si le nom existe déjà.
### `PATCH /api/sites/{id}` — renommage. Les mesures passées gardent l'ancien tag `site` mais restent jointes par `site_id`.

### `POST /api/enroll-codes` — authentification : session
Génère un **code d'enrôlement court** (8 caractères, valable 15 minutes, à usage
unique), affiché dans l'interface avec l'URL du hub :

```json
{ "code": "K7M2-4PQX", "site": "Durand", "expires_at": 1756340100 }
```

C'est le chemin **recommandé** pour enrôler une sonde : l'utilisateur colle une
URL et un code, rien d'autre. Taper l'identifiant et le mot de passe du compte
admin sur chaque machine sonde est à la fois pénible et une mauvaise idée — un
poste compromis ne doit pas livrer les identifiants de l'interface. Un code
expiré ou déjà consommé n'ouvre rien.

### `POST /api/probes/enroll` — code d'enrôlement **ou** identifiants du compte
```json
{ "code": "K7M2-4PQX", "name": "Paris" }
```
ou, pour un usage scripté :
```json
{ "username": "admin", "password": "...", "name": "Paris", "site": "Durand" }
```
Avec un code, le site vient du code ; il n'est pas à ressaisir.
`site` est résolu par nom, insensible à la casse. S'il n'existe pas, le hub le
crée et le signale par `site_created: true` — le champ existe pour que l'app
affiche « nouveau site créé », faute de quoi une faute de frappe passerait
inaperçue jusqu'à ce qu'on cherche une sonde dans le mauvais parc.

→ `201`
```json
{
  "probe_id": "0c1e…",
  "name": "Paris",
  "site_id": "…",
  "site": "Durand",
  "site_created": false,
  "token": "…",
  "influx": {
    "url": "https://lanprobe-web.example.org:8086",
    "org": "lanprobe",
    "bucket": "lanprobe",
    "token": "…"
  },
  "heartbeat_interval_secs": 60
}
```

Le jeton Influx renvoyé est **restreint en écriture au seul bucket**. Une sonde
compromise ne peut ni lire les mesures des autres, ni les effacer.

Si `name` est déjà pris **dans ce site**, le hub répond `409` — deux sondes
« Paris » chez Durand rendent l'interface inutilisable, autant le refuser tout de
suite. Le même nom dans un autre site est parfaitement légitime.

### `POST /api/probes/{id}/heartbeat` — authentification : `Authorization: Bearer <token sonde>`
```json
{
  "version": "1.2.0",
  "platform": "linux",
  "buffered_points": 0,
  "last_write_ok": true
}
```
→ `200`
```json
{ "ok": true, "name": "Paris", "site_id": "…", "site": "Durand",
  "heartbeat_interval_secs": 60 }
```
Et, quand une rotation de clé est en attente, la nouvelle clé d'écriture :
`"influx": { …, "token_version": 3 }` (voir section 9).

C'est la réponse au battement de cœur qui propage aux sondes tout changement
décidé côté hub — renommage, changement de cadence, nouvelle clé. Une sonde
n'interroge jamais le hub pour savoir si quelque chose a bougé.

`buffered_points` remonte la profondeur du tampon local : une sonde qui accumule
sans écrire est visible dans l'interface **avant** qu'on découvre le trou dans les
graphes.

### `GET /api/probes[?site=<site_id>]` — authentification : session
```json
[{
  "probe_id": "0c1e…", "name": "Paris",
  "site_id": "…", "site": "Durand", "platform": "linux",
  "version": "1.2.0", "last_seen": 1756339200, "status": "online",
  "buffered_points": 0, "created_at": 1756332000
}]
```

`status` est **dérivé**, jamais stocké :
- `online` — dernier battement < 3 × `heartbeat_interval_secs`
- `stale` — < 24 h
- `offline` — au-delà

Stocker un statut obligerait quelqu'un à le mettre à jour ; une valeur dérivée ne
peut pas mentir sur sa propre fraîcheur.

### `PATCH /api/probes/{id}` — `{ "name": "Nouveau nom", "site_id": "…" }`
Les deux champs sont facultatifs. Changer `site_id` déplace la sonde d'un site à
l'autre ; son historique la suit, puisqu'il est joint sur `probe_id`.
### `DELETE /api/probes/{id}` — révoque le jeton. **Ne supprime aucune mesure.**
### `GET /api/probes/{id}/metrics?measurement=&range=` — requête Flux proxifiée

Le navigateur ne parle jamais directement à Influx : le jeton de lecture reste
côté serveur.

## 4. Schéma SQLite

```sql
CREATE TABLE users (
  id            INTEGER PRIMARY KEY,
  username      TEXT NOT NULL UNIQUE,
  password_hash TEXT NOT NULL,          -- argon2id
  role          TEXT NOT NULL DEFAULT 'admin',
  created_at    INTEGER NOT NULL
);

CREATE TABLE sites (
  site_id    TEXT PRIMARY KEY,          -- UUIDv4
  name       TEXT NOT NULL UNIQUE COLLATE NOCASE,
  created_at INTEGER NOT NULL
);

CREATE TABLE probes (
  probe_id        TEXT PRIMARY KEY,
  site_id         TEXT NOT NULL REFERENCES sites(site_id),
  name            TEXT NOT NULL,
  token_hash      TEXT NOT NULL,        -- argon2id : jamais le jeton en clair
  platform        TEXT,
  version         TEXT,
  last_seen       INTEGER,
  buffered_points INTEGER DEFAULT 0,
  created_at      INTEGER NOT NULL,
  revoked_at      INTEGER
);

-- Le nom est unique DANS un site, pas globalement.
CREATE UNIQUE INDEX probes_site_name ON probes(site_id, name COLLATE NOCASE);

-- Réglages modifiables depuis l'interface, qui priment sur l'environnement.
CREATE TABLE settings (
  key        TEXT PRIMARY KEY,
  value      TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);
```

`COLLATE NOCASE` sur les noms : « Durand » et « durand » sont le même site.
Sans ça, un doublon de casse crée deux parcs distincts et personne ne comprend
pourquoi la moitié des sondes a disparu.

Supprimer un site n'est pas prévu tant qu'il porte des sondes — le hub répond
`409`. Aucune suppression en cascade dans ce projet.

Le jeton de sonde est haché comme un mot de passe. Une base volée ne donne pas
le droit d'écrire au nom des sondes.

## 5. Tampon hors ligne (côté sonde)

**Défaut à corriger d'abord** — `influxdb.rs` vide le tampon *avant* l'écriture :

```rust
let body = buffer.join("\n");
buffer.clear();                  // ← perte définitive si l'écriture échoue
if let Err(e) = client.write(body).await { tracing::warn!(…) }
```

Comportement attendu :

1. **Ne pas vider avant confirmation.** En cas d'échec, les points retournent en
   tête de file et le prochain tick réessaie.
2. **Persistance disque** — `buffer.ndjson` dans le répertoire de config, relu au
   démarrage. Une coupure de courant pendant l'incident ne doit pas effacer la
   trace de l'incident.
3. **Borné explicitement** — `MAX_BUFFER = 100_000` points *et* 24 h d'âge. Au
   débordement, on jette **les plus anciens** et on **journalise le nombre exact**.
   Un tampon qui déborde en silence ment sur la complétude des données.
4. **Horodatage préservé** — les points sont déjà datés à la mesure dans
   `event_to_points`. Ne pas re-dater à l'envoi : le rejeu doit retomber au bon
   endroit dans le graphe.
5. **Repli exponentiel** plafonné à 60 s entre deux tentatives, pour ne pas
   marteler un Influx qui redémarre.

## 6. Ce que la sonde stocke

Dans `app_config.json`, section `hub`, chiffrée par le scellement existant
(`secrets.rs`, marqueur `enc:v1:`) :

```json
{ "url": "https://…", "probe_id": "…", "name": "…",
  "site_id": "…", "site": "…", "token": "enc:v1:…",
  "influx": { "url": "…", "org": "…", "bucket": "…", "token": "enc:v1:…" } }
```

La sonde a besoin de `site_id` et `site` en local parce que **c'est elle qui
écrit les tags** sur ses points. Après un renommage de site côté hub, elle
récupère la nouvelle valeur dans la réponse au battement de cœur :

```json
{ "ok": true, "name": "Paris", "site_id": "…", "site": "Durand" }
```

Sans ce retour, une sonde continuerait d'écrire l'ancien nom indéfiniment après
un renommage — et le tag lisible divergerait de l'interface, ce qui est pire que
de ne pas l'avoir.

Le mot de passe du compte n'est **jamais** écrit sur disque : il sert une fois,
à l'enrôlement.

## 7. Configuration du hub

**Principe : on configure dans l'application, pas dans l'environnement.** Une
valeur qui vit dans un `.env` demande d'éditer un fichier, de redémarrer le
conteneur, et se perd à la première migration de machine. On a SQLite : les
réglages y vivent, modifiables depuis l'interface, sauvegardés avec le reste.

### Ce qui reste hors de la base — et pourquoi

Seulement ce qui est nécessaire **pour atteindre l'interface**, sans quoi on ne
pourrait rien configurer du tout :

| | Rôle |
|---|---|
| `--host`, `--port` | où écouter. Défauts `0.0.0.0`, `8443`. |
| `--config-dir` | où vit la base. Défaut `/data/lanprobe`. |
| `<config-dir>/influx-operator-token` | jeton opérateur Influx, **lu dans un fichier du volume**, pas dans l'environnement — il est généré au premier démarrage par l'entrypoint, ce n'est pas un réglage utilisateur. **Ne sort jamais du hub.** |

Aucune variable d'environnement n'est requise. Celles qui existent (`INFLUX_URL`,
`INFLUX_ORG`, `INFLUX_BUCKET`, `INFLUX_ADVERTISE_URL`) restent acceptées comme
**valeurs initiales** au premier démarrage, pour qui préfère préconfigurer ; dès
qu'un réglage est écrit en base, la base gagne.

### Ce qui est réglable dans l'interface (table `settings`)

| Clé | Défaut | Effet |
|---|---|---|
| `influx_url` | `https://127.0.0.1:8086` | où le hub joint Influx |
| `influx_org`, `influx_bucket` | `lanprobe` | org et bucket |
| `influx_advertise_url` | déduit | URL remise **aux sondes** (voir plus bas) |
| `retention_days` | `0` (illimité) | rétention du bucket, appliquée via l'API Influx |
| `heartbeat_interval_secs` | `60` | cadence demandée aux sondes |

Changer `retention_days` applique la nouvelle rétention au bucket. **Réduire la
rétention supprime des données** : l'interface doit le dire explicitement et
demander confirmation, en nommant la durée qui sera perdue. Un réglage qui efface
sans le dire est un piège.

### L'URL annoncée n'est pas l'URL interne

Le hub joint Influx sur `127.0.0.1` ; une sonde sur le LAN, non. Renvoyer l'URL
interne à l'enrôlement produirait une sonde qui s'enrôle avec succès puis
n'écrit jamais rien — la panne la plus pénible à diagnostiquer, parce que tout
paraît avoir marché.

**Un conteneur ne peut pas connaître son port publié.** Avec `-p 9086:8086`, le
processus ne voit que `8086` ; Docker ne lui communique jamais le `9086`. Aucune
détection automatique n'est possible — la seule réponse honnête est de rendre la
valeur réglable et vérifiable.

**Ordre de résolution de l'URL annoncée :**
1. valeur réglée **dans l'interface web** (`settings['influx_advertise_url']`) —
   c'est elle qui gagne ;
2. sinon `INFLUX_ADVERTISE_URL`, si fournie au premier démarrage ;
3. sinon, l'hôte de l'en-tête `Host` de la requête d'enrôlement, avec le port
   Influx. La sonde vient de joindre le hub à cette adresse — Influx est sur la
   même machine, c'est le meilleur pari disponible ;
4. journaliser laquelle des trois a été retenue.

Le repli sur l'en-tête `Host` évite d'imposer une variable obligatoire à qui
monte le conteneur : ports standards et pas de reverse proxy, ça marche sans
rien régler. Les ports remappés se corrigent dans l'interface, sans redémarrage.

### `POST /api/settings/influx-advertise/test` — authentification : session

Le hub tente de joindre `/health` à l'URL annoncée et rend le verdict :

```json
{ "url": "https://192.168.1.10:9086", "reachable": true, "source": "settings" }
```

Sans ce test, une URL fausse produit exactement la panne qu'on cherche à éviter :
une sonde qui s'enrôle, apparaît dans l'interface, et n'écrit rien. Avec lui,
l'erreur est visible **avant** d'enrôler quoi que ce soit, à l'endroit où on la
corrige. `source` vaut `settings`, `env` ou `host_header` — pour que le
diagnostic dise d'où vient la valeur, pas seulement qu'elle est mauvaise.

`GET`/`PUT /api/settings/influx-advertise` lisent et écrivent la valeur.
L'écran d'enrôlement **affiche l'URL exacte** qui sera remise à la sonde : une
valeur qu'on ne découvre qu'en lisant les logs est une valeur que personne ne
vérifie.

### TLS auto-signé côté Influx

Influx est servi en HTTPS avec un certificat auto-signé généré au premier
démarrage. La sonde doit donc pouvoir **épingler ou accepter explicitement** ce
certificat. La réponse d'enrôlement inclut son empreinte :

```json
"influx": { "url": "…", "org": "…", "bucket": "…", "token": "…",
            "tls_fingerprint_sha256": "AB:CD:…" }
```

**Le hub calcule cette empreinte à partir du certificat présenté pendant le
handshake TLS vers `influx_url`** — jamais en lisant un fichier sur disque. Le
certificat d'Influx vit dans son propre volume, à un chemin qui ne regarde pas le
hub : en dépendre coupleraient deux composants qui n'ont aucune raison de
partager leur arborescence, et casserait le jour où Influx tourne ailleurs.
Recalculée à chaque démarrage, mise en cache — un certificat renouvelé se
propage aux sondes au battement de cœur suivant.

Épingler l'empreinte vaut mieux que désactiver la vérification : on garde
l'authentification du serveur sans exiger une autorité de certification que
personne n'a dans un réseau local.

## 8. Influx vu depuis les réglages

L'utilisateur ne doit **jamais avoir à administrer Influx**. Le bucket et l'org
sont créés au premier démarrage. Mais il doit pouvoir aller voir ses données —
donc les réglages exposent, en lecture : URL, org, bucket, version, état de
santé, taille occupée, et le nombre de séries.

### `POST /api/influx/read-token` — authentification : session

Génère un jeton **en lecture seule** sur le bucket, à coller dans Grafana.
Affiché **une seule fois** — le hub n'en garde que l'identifiant, pour pouvoir le
révoquer. Sans ce bouton, « voir ses données » s'arrête à savoir où elles sont.

`GET /api/influx/read-tokens` liste les jetons émis (identifiant, description,
date), `DELETE /api/influx/read-tokens/{id}` en révoque un.

## 9. Rotation des jetons d'écriture

**Il n'y a pas une clé de bucket : il y a une clé par sonde.** Une sonde
compromise se révoque seule, les autres continuent d'écrire. Une clé partagée
aurait imposé de redéployer tout le parc à la première machine volée.

Deux actions distinctes, parce qu'elles ne répondent pas au même besoin :

### `POST /api/probes/{id}/rotate` — hygiène

1. le hub crée un nouveau jeton d'écriture et le stocke comme **en attente** ;
2. la réponse au **battement de cœur suivant** le transporte, avec un numéro de
   version : `{ "influx": { …, "token_version": 3 } }` ;
3. la sonde renvoie `influx_token_version` dans ses battements suivants ;
4. **c'est seulement à la réception de cet accusé** que le hub détruit l'ancien
   jeton dans Influx.

Détruire avant l'accusé couperait toute sonde éteinte au mauvais moment — elle
se réveillerait avec un jeton mort et sans moyen d'en obtenir un autre.

### `POST /api/probes/{id}/revoke-token` — compromission

L'ancien jeton est détruit **immédiatement**, le nouveau attend le prochain
contact. La sonde ne peut pas écrire dans l'intervalle : c'est le comportement
voulu — quand une clé fuit, on coupe d'abord et on rétablit ensuite.

L'interface doit présenter les deux avec leur conséquence écrite. Confondre les
deux donnerait soit des sondes coupées par une rotation de routine, soit une clé
compromise qui reste valide tant que la machine volée ne rappelle pas —
c'est-à-dire indéfiniment.

Ni l'une ni l'autre ne supprime la moindre mesure.

### Remise en service à distance

Le cas réel : la sonde est à l'autre bout du pays et personne ne peut s'y rendre.

**Clé d'écriture fuitée** — après `revoke-token`, le lien de la sonde avec le hub
est **intact** : au battement de cœur suivant elle reçoit sa nouvelle clé et se
remet à écrire. Aucune intervention, un trou d'un battement.

**Machine compromise** — il ne faut surtout pas qu'elle se répare seule, c'est
l'intérêt de la révoquer. Il faut donc une action humaine sur place, aussi légère
que possible :

### `POST /api/probes/{id}/reenroll-code` — authentification : session

Émet un code d'enrôlement **rattaché à la sonde existante** (mêmes règles que
`/api/enroll-codes` : 8 caractères, 15 minutes, usage unique). La personne sur
place le saisit dans l'app — huit caractères dictés au téléphone, sans compte ni
compréhension du système.

La sonde revient avec **le même `probe_id`, donc tout son historique**. Un
`DELETE` suivi d'un nouvel enrôlement aurait créé une seconde sonde et coupé les
courbes en deux à la date de l'incident — précisément quand on veut comparer
l'avant et l'après.

Le ré-enrôlement remplace le jeton de sonde **et** le jeton d'écriture Influx :
les identifiants de la machine compromise ne valent plus rien.

## 10. Phase 2 — piloter une sonde à distance (non implémenté)

Objectif : cliquer sur une sonde dans le parc et **piloter la vraie LanProbe du
site** — scan, ports, speedtest — depuis le navigateur, sans VPN ni port ouvert
chez le client.

La moitié du travail est déjà faite : `lanprobe-server` sert **l'intégralité** de
l'interface LanProbe en HTTP+WebSocket. L'obstacle est le sens de la connexion —
la sonde est derrière le NAT du client, le hub ne peut pas l'appeler.

**Approche : tunnel inversé sur le canal existant.** La sonde garde une
connexion sortante vers le hub (le battement de cœur en pose déjà la base), et
le hub y multiplexe les requêtes du navigateur.

⚠️ **Ça change la nature du produit côté sécurité, et il faut le traiter comme
tel.** Aujourd'hui une sonde compromise ne peut qu'écrire de fausses mesures.
Avec le tunnel, qui prend la main sur le hub peut **piloter les outils réseau de
tous les sites** — scanner, sonder les ports, depuis l'intérieur de chaque
réseau client. Conditions minimales : désactivé par défaut, activable **par
sonde**, révocable, et chaque session journalisée.

À cadrer pour lui-même, pas à greffer.

## 11. Audit et comptes (non implémenté)

### Deux journaux, à ne pas confondre

**Journal technique** (fait) — requêtes, codes, présence du cookie de session,
`X-Forwarded-Proto`. Va sur la sortie standard, se perd au redémarrage. Sert au
diagnostic. Sans lui, un « je n'arrive pas à me connecter » est indiagnosticable :
le serveur répond correctement à tous les tests et personne ne voit ce que le
client a réellement envoyé.

**Journal d'audit** (à faire) — en base, consultable dans l'interface : qui s'est
connecté, qui a enrôlé ou révoqué une sonde, fait tourner une clé, changé la
rétention. Acteur, action, cible, date, résultat. Jamais de secret.

Deux règles sans lesquelles il ne sert à rien :

1. **Les échecs sont journalisés autant que les réussites.** Un journal qui
   n'enregistre que les succès ne montre jamais une tentative d'intrusion — il
   montre celui qui a fini par entrer.
2. **Ajout seul.** Aucune route ne supprime une ligne d'audit, pas même pour un
   administrateur. Un journal qu'on peut nettoyer ne prouve rien.

### Rôles

La table `users` porte déjà `role` ; il manque la gestion et l'application.

| Rôle | Peut |
|---|---|
| `admin` | tout, y compris les comptes et la rétention |
| `operator` | enrôler, renommer, déplacer, faire tourner les clés |
| `viewer` | consulter |

`viewer` est le rôle qui manque le plus en entreprise : montrer le parc à un
client ou à un collègue sans qu'un clic révoque une sonde.

L'application des rôles se fait **côté serveur**, route par route. Une interface
qui se contente de masquer les boutons ne protège rien.

## 12. Inventaire réseau — le détail va en SQLite, pas dans Influx

Validé avec le propriétaire : il veut **le détail complet** des scans de ports,
des machines découvertes et des speedtests, pas seulement des compteurs.

### Pourquoi pas InfluxDB

Un scan de ports n'est pas une métrique, c'est **un inventaire daté**. Une
latence est une valeur par instant — Influx est fait pour ça. « Le port 22 est
ouvert sur 192.168.1.5 » est un fait, pas une mesure.

Et le ranger dans Influx casse à l'échelle. Influx crée une série par
combinaison d'étiquettes : avec `ip` + `port`, un scan d'un /24 dont chaque
machine expose une vingtaine de ports fabrique **~5 000 séries pour un seul site
et un seul scan**. Multiplié par les sites et par les machines qui changent
d'adresse au fil des mois, la base ralentit puis sature. C'est le mode de panne
classique d'InfluxDB, et il ne se voit pas avant qu'il soit trop tard.

### Répartition

```
Influx  → ce qui varie en continu : latence, débits, disponibilité, état internet
SQLite  → ce qui est un inventaire : ports ouverts, hôtes découverts,
          adresses MAC, constructeurs, noms d'hôte, résultats de speedtest
```

Le compteur reste dans Influx pour la courbe (« 3 ports ouverts hier, 7
aujourd'hui ») ; le détail est à un clic derrière. Et « toutes les machines avec
le 3389 ouvert, tous sites confondus » devient une requête SQL triviale, là où
elle serait une horreur en Flux.

### Schéma

```sql
CREATE TABLE scans (               -- une exécution
  scan_id    TEXT PRIMARY KEY,
  probe_id   TEXT NOT NULL REFERENCES probes(probe_id),
  kind       TEXT NOT NULL,        -- 'ports' | 'discovery' | 'speedtest'
  started_at INTEGER NOT NULL,
  cidr       TEXT
);

CREATE TABLE scan_hosts (          -- une machine vue lors d'un scan
  scan_id  TEXT NOT NULL REFERENCES scans(scan_id),
  ip       TEXT NOT NULL,
  hostname TEXT, mac TEXT, vendor TEXT, latency_ms INTEGER,
  PRIMARY KEY (scan_id, ip)
);

CREATE TABLE scan_ports (          -- un port ouvert
  scan_id TEXT NOT NULL REFERENCES scans(scan_id),
  ip      TEXT NOT NULL,
  port    INTEGER NOT NULL,
  proto   TEXT NOT NULL,           -- 'tcp' | 'udp'
  service TEXT,
  PRIMARY KEY (scan_id, ip, port, proto)
);

CREATE TABLE speedtests (
  scan_id       TEXT PRIMARY KEY REFERENCES scans(scan_id),
  engine        TEXT, server_name TEXT,
  download_mbps REAL, upload_mbps REAL,
  latency_ms    INTEGER, jitter_ms REAL
);

CREATE INDEX scan_ports_lookup ON scan_ports(port, proto);
CREATE INDEX scan_hosts_ip     ON scan_hosts(ip);
```

On enregistre **un scan par exécution**, jamais un état courant écrasé : c'est ce
qui permet de répondre à « quels ports étaient ouverts le 12 mars » et « quelle
machine est apparue cette semaine ». Un inventaire sans historique ne répond
qu'au présent, qu'on pouvait déjà observer.

### `POST /api/probes/{id}/scans` — authentification : jeton de sonde

Troisième flux, en plus de l'écriture Influx et du battement de cœur. La sonde
publie le résultat complet d'un scan. Corps : `kind`, `started_at`, `cidr`, la
liste des hôtes et de leurs ports ouverts, ou le résultat de speedtest.

Le hub **ne dérive pas** le compteur Influx de ce message : la sonde continue de
l'écrire elle-même. Deux chemins pour la même valeur finiraient par diverger.

### Purge

L'inventaire grossit. Une rétention lui est propre (`settings['inventory_days']`,
défaut `0` = illimité) — et comme pour la rétention Influx, **la réduire supprime
des données** : même confirmation explicite exigée, côté serveur comme côté
interface.

## 13. Notifications (non implémenté)

### Deux canaux, pas cinq

```
SMTP              l'e-mail
webhook générique Slack, Discord, Teams, ntfy, Gotify… tous acceptent un POST JSON
```

Un webhook paramétrable avec deux modèles préremplis (Slack, Discord) couvre
l'écosystème entier, et le cas maison ou Mattermost marche sans toucher au code.
Cinq intégrations dédiées auraient cinq fois plus de surface à maintenir pour
le même résultat.

### Le difficile n'est pas le canal, c'est le déclencheur

Notifier « sonde hors ligne » condamne la fonctionnalité : un portable qu'on
referme émet une alerte à chaque fois. Trois alertes inutiles et l'utilisateur
coupe tout — et le jour où un vrai site tombe, personne ne le voit.

1. **Transitions, pas états.** « Paris est passée hors ligne », une fois. Pas un
   rappel par minute tant qu'elle l'est.
2. **Délai avant alerte**, 5 minutes par défaut. Une sonde qui redémarre ne
   réveille personne.
3. **Le retour est notifié aussi.** Une alerte sans son « c'est revenu » oblige
   à aller vérifier à la main, donc on cesse de les lire.
4. **Activable par sonde ou par site.** Un poste de test n'alerte pas, la baie
   d'un client si.

Même logique que les alertes de disponibilité Taskori, pour les mêmes raisons.

### ⚠️ Les secrets ne vont pas dans `settings`

La section 7 pose que la table `settings` ne contient **aucun secret** et que
`GET /api/settings` peut donc être rendue telle quelle. Un mot de passe SMTP et
une URL de webhook (qui est elle-même un secret porteur) violent les deux.

Ils vivent dans un stockage scellé à part, chiffré par le mécanisme existant
(`secrets.rs`, marqueur `enc:v1:`), **jamais renvoyé par l'API**. L'interface
affiche « configuré » ou « non configuré », avec un bouton de test — jamais la
valeur.
