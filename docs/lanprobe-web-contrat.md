# LanProbe Web — contrat d'interface

Document de référence partagé entre le **hub** (`crates/lanprobe-web`, auto-hébergé
en conteneur) et la **sonde** (`crates/lanprobe-server` + client desktop Tauri).

Toute divergence entre les deux implémentations se règle ici, pas dans le code.

---

## 1. Vue d'ensemble

```
┌────────────────┐  1. enrôlement (code court, ou compte)  ┌──────────────────┐
│  sonde         │ ─────────────────────────────────────▶  │  lanprobe-web    │
│  (desktop ou   │ ◀──────────────────  jeton de sonde ──   │  (conteneur)     │
│   headless)    │                                          │                  │
│                │   2. battement de cœur (jeton sonde)     │  SQLite : users, │
│                │ ─────────────────────────────────────▶  │  probes, sites,  │
│                │                                          │  audit, settings │
│                │   3. mesures — RELAYÉES par le hub       │        │         │
│                │ ─────────────────────────────────────▶  │        ▼         │
└────────────────┘   line protocol, une seule adresse       │  InfluxDB 2      │
                                                             │  (embarqué,      │
                                                             │  local au hub)   │
                                                             └──────────────────┘
```

**Le hub relaie, il ne fait plus passer les sondes en écriture directe.** Une
sonde poste ses lots sur `POST /api/probes/{id}/write`, avec le même jeton que
son battement de cœur, et le hub les rejoue vers son Influx embarqué. Une
seule porte, une seule authentification, un seul certificat côté sonde —
Influx n'a plus besoin d'être joignable depuis le réseau de chaque site, ni
d'exposer un port dédié : seul le hub lui parle, en local.

Le hub ne bufferise pas ce qu'il relaie : accumuler ferait gonfler sa mémoire
sur une pointe d'écriture, et un redémarrage perdrait des mesures que la sonde
croit déjà livrées. Une panne d'Influx répond `502`, pas `500` — c'est le
signal pour la sonde de conserver son lot et de réessayer, là où un `4xx` lui
dirait de l'abandonner.

**Pourquoi le battement de cœur reste un flux à part.** Les mesures ne disent
rien d'une sonde éteinte : « rien » ressemble exactement à « tout va bien »
dans une base de séries temporelles. Le battement de cœur est ce qui distingue
une sonde silencieuse d'une sonde absente. Sans lui, l'interface afficherait un
parc vert pendant qu'une sonde est débranchée depuis trois jours.

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
- `stale` — < 2 h
- `offline` — au-delà

⚠️ **Deux heures, pas vingt-quatre.** Le seuil ne décrit pas une panne, il
décide quand quelqu'un doit se déplacer. Un redémarrage ou une mise à jour
dépassent rarement vingt minutes ; une sonde muette depuis deux heures est un
problème qu'on veut régler **dans la journée**, pas retrouver le lendemain
matin. À vingt-quatre heures, l'état « hors ligne » arrivait toujours trop
tard pour servir à quelque chose.

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
| `--host`, `--port` | où écouter. Défauts `0.0.0.0`, `8080`. |
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

## 10. Tunnel inversé — ABANDONNÉ

⚠️ **Décision du propriétaire, 2026-08-28 : on ne le construit pas.** Il ne veut
pas l'interface de la sonde dans le hub, seulement pouvoir la piloter — ce que
la file de commandes (§14) fait sans ouvrir de canal permanent.

C'est aussi le meilleur choix côté sécurité : le tunnel aurait fait du hub une
machine capable d'atteindre l'intérieur de chaque réseau client en permanence.
La file de commandes ne fait rien tant que personne ne demande rien.

Ce qui suit est conservé pour mémoire, au cas où le besoin d'une interface
temps réel réapparaîtrait.

### Ancienne étude

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

## 11. Audit et comptes

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

### Rôle exigé par chaque route

Trois groupes, appliqués par middleware après authentification. Les rôles sont
ordonnés : `viewer < operator < admin`, et une route ouverte à `operator` est
donc aussi ouverte à `admin`.

| Route | Rôle | Note |
|---|---|---|
| `GET /api/status`, `POST /api/setup`, `POST /api/login`, `POST /api/logout` | — | authentification propre |
| `POST /api/probes/enroll`, `.../write`, `.../heartbeat` | — | code d'enrôlement ou jeton de sonde |
| `GET /api/sites` | `viewer` | accepte aussi les identifiants du compte (Basic) |
| `GET /api/probes`, `GET /api/probes/{id}/metrics` | `viewer` | |
| `GET /api/settings`, `GET /api/settings/influx-advertise`, `GET /api/influx` | `viewer` | aucune de ces réponses ne porte de secret |
| `GET /api/notifications/subscriptions` | `viewer` | |
| `POST /api/sites`, `PATCH`/`DELETE /api/sites/{id}` | `operator` | |
| `POST /api/enroll-codes`, `GET /api/enroll-codes/pending` | `operator` | le code y figure **en clair** pendant sa validité |
| `PATCH`/`DELETE /api/probes/{id}` | `operator` | |
| `POST /api/probes/{id}/rotate`, `/revoke-token`, `/reenroll-code` | `operator` | |
| `POST /api/settings/influx-advertise/test` | `operator` | émet une requête sortante |
| `PUT /api/notifications/subscriptions` | `operator` | |
| `PUT /api/settings`, `PUT /api/settings/influx-advertise` | `admin` | la rétention en fait partie |
| `POST /api/influx/read-token`, `GET`/`DELETE /api/influx/read-tokens[/{id}]` | `admin` | un jeton qui quitte le hub |
| `GET`/`POST /api/users`, `PATCH /api/users/{username}`, `POST /api/users/{username}/password` | `admin` | |
| `GET /api/audit` | `admin` | |
| `GET /api/notifications`, `PUT`/`DELETE /api/notifications/{smtp,webhook}`, `POST /api/notifications/test` | `admin` | |

Un refus rend `403` avec `{ "error": "rôle insuffisant pour cette action" }`
— et **écrit une ligne d'audit** `access.denied`. Un `401` renverrait à l'écran
de connexion, où se reconnecter ne changerait rien.

Le rôle est **relu à chaque requête**, jamais figé dans la session : une
rétrogradation ou une désactivation prend effet tout de suite, y compris pour
qui garde son onglet ouvert. Désactiver un compte invalide donc aussi ses
sessions en cours.

L'enrôlement par identifiants de compte (`POST /api/probes/enroll` avec
`username`/`password`/`site`) vérifie lui aussi le rôle : `403` en dessous
d'`operator`. Sans ce contrôle, le chemin scripté contournait tous les autres.

### Comptes — `admin`

**Aucune route ne supprime un compte.** On désactive : la ligne reste, parce
que le journal d'audit la nomme. Trois refus (`409`) protègent le dernier
accès : pas d'auto-rétrogradation, pas d'auto-désactivation, pas de
désactivation ni de rétrogradation du dernier administrateur actif.

```
GET    /api/users
       → [ { "username": "admin", "role": "admin",
             "created_at": 1787788800, "disabled_at": null } ]

POST   /api/users
       { "username": "claire", "password": "…", "role": "viewer" }
       → 201 { "username": "claire", "role": "viewer",
               "created_at": …, "disabled_at": null }
       400 rôle inconnu · 409 nom déjà pris · 409 mot de passe < 8 caractères

PATCH  /api/users/{username}
       { "role": "operator" }   et/ou   { "disabled": true }
       → 200 <le compte, forme ci-dessus>
       400 rôle inconnu · 404 compte inconnu · 409 dernier admin / soi-même

POST   /api/users/{username}/password
       { "password": "…" }  → 200 { "ok": true }
       404 compte inconnu · 409 mot de passe < 8 caractères

DELETE /api/users/{username} → 405, et il en sera toujours ainsi.
```

Le hash du mot de passe n'apparaît dans aucune réponse.

### Journal d'audit — `admin`, lecture seule

```
GET /api/audit?limit=&before_id=&actor=&action=
    → { "entries": [ { "id": 42, "at": 1787788800, "actor": "admin",
                       "action": "probe.revoke", "target": "<probe_id>",
                       "outcome": "success" | "failure",
                       "detail": "…" | null } ],
        "next_before_id": 37 | null }
```

`limit` vaut 100 par défaut, plafonné à 500. La pagination va à rebours :
`before_id` reprend là où la page précédente s'est arrêtée. Un `OFFSET`
rejouerait des lignes déjà vues dès qu'une ligne s'ajoute pendant la lecture.
`next_before_id` n'est rendu que si la page est pleine — sinon l'interface
afficherait un « suivant » qui ne mène à rien.

`actor` vaut `null` pour une tentative anonyme : une connexion ratée sur un
compte inexistant n'a pas d'acteur connu, et **la saisie n'est jamais
recopiée** — ce champ reçoit un jour un mot de passe tapé dans la mauvaise
case.

**Aucune route ne supprime une ligne, et il n'y a pas de purge par
ancienneté.** `DELETE /api/audit` rend `405`. Un journal qu'on peut nettoyer ne
prouve rien, et quelques dizaines d'octets par geste d'opérateur ne justifient
pas d'inventer un chemin d'effacement.

Actions journalisées : `auth.setup`, `auth.login`, `auth.logout`,
`access.denied`, `user.create`, `user.role`, `user.password`, `user.disable`,
`user.enable`, `site.create`, `site.rename`, `site.delete`, `enroll.code`,
`probe.enroll`, `probe.reenroll`, `probe.reenroll_code`, `probe.update`,
`probe.revoke`, `probe.rotate`, `probe.revoke_token`, `settings.update`,
`settings.retention`, `settings.advertise`, `influx.read_token.create`,
`influx.read_token.revoke`, `notify.webhook`, `notify.smtp`,
`notify.unconfigure`, `notify.subscription`, `notify.test`, `probe.down`,
`probe.up`.

**Jamais de secret dans une ligne.** Ni jeton de sonde, ni jeton de lecture, ni
code d'enrôlement, ni mot de passe, ni URL de webhook. Les réglages, eux, y
figurent en clair : la table `settings` n'en contient aucun, par construction.

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

## 13. Notifications

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

## 14. Fiche d'une sonde : cinq onglets ✅ implémenté (six, avec les commandes)

Demandé par le propriétaire : la fiche reprend ce que fait LanProbe, et permet
de le **déclencher à distance**.

```
Tableau de bord   la vue actuelle — état, latence, internet, débits
Surveillance      les cibles surveillées, ajout et retrait
Speedtest         historique des tests, et lancer un test
Scan de ports     inventaire par machine, et lancer un scan
Découverte        machines vues sur le réseau, et relancer une découverte
```

### Déclencher à distance sans tunnel

Le battement de cœur est **déjà** un canal du hub vers la sonde. Sa réponse
transporte donc une **file de commandes** :

```json
{ "ok": true, …, "commands": [
  { "id": 42, "kind": "port_scan", "args": { "ip": "192.168.1.10" } },
  { "id": 43, "kind": "add_monitor", "args": { "ip": "192.168.1.1" } }
]}
```

La sonde exécute, publie le résultat (§12 pour les inventaires, Influx pour les
séries) et accuse chaque `id` au battement suivant. Le hub retire les commandes
accusées ; une commande sans accusé au bout de trois battements est marquée
échouée plutôt que rejouée indéfiniment.

**Latence : jusqu'à un intervalle de battement.** Pour un scan ou un speedtest
qui dure plusieurs minutes, c'est invisible. Le tunnel inversé (§10) ne devient
nécessaire que pour l'interface temps réel de la sonde — voir les résultats
défiler pendant qu'ils arrivent. Construire la file d'abord donne l'essentiel
pour une fraction du travail.

⚠️ **Une commande est une action sur le réseau d'un client.** Un scan de ports
lancé depuis le hub part de l'intérieur du LAN surveillé : réservé à `operator`,
journalisé à l'audit avec son auteur, et jamais rejoué automatiquement.

### Ce que le hub expose

Toutes ces routes sont réservées à `admin`, sauf les abonnements — `viewer` en
lecture, `operator` en écriture, parce que régler l'alerte d'un site est une
conduite de parc et non une administration du hub.

```
GET /api/notifications
    → { "delay_secs": 300,
        "smtp": { "configured": true, "host": "smtp.example.org", "port": 587,
                  "security": "starttls", "username": "hub",
                  "password_set": true, "from": "hub@…", "to": ["ops@…"] },
        "webhook": { "configured": true, "template": "discord" } }
```

**Ni l'URL du webhook ni le mot de passe SMTP n'apparaissent** — jamais, sur
aucune route. Les autres champs SMTP sont rendus parce qu'ils ne sont pas des
secrets et qu'un formulaire d'édition qui les cacherait obligerait à tout
retaper à chaque correction de port.

Un canal non configuré rend `{ "configured": false, "unreadable": <bool> }`.
`unreadable: true` veut dire qu'une valeur scellée existe mais ne s'ouvre plus
— volume restauré sans son `secret.key`. L'interface doit le dire : « configuré
mais illisible » n'est pas « jamais réglé », et la réponse n'est pas la même.

```
PUT    /api/notifications/webhook
       { "url": "https://…", "template": "generic" | "slack" | "discord" }
       → 200 <même corps que GET>   ·   400 URL ou modèle invalide

PUT    /api/notifications/smtp
       { "host": "smtp.example.org", "port": 587,
         "security": "none" | "starttls" | "tls",
         "username": "hub" | null, "password": "…" | null,
         "from": "hub@example.org", "to": ["ops@example.org"] }
       → 200 <même corps que GET>
       400 serveur, expéditeur, port ou destinataire manquant, chiffrement inconnu

DELETE /api/notifications/{webhook,smtp}
       → 200 <même corps que GET>, canal dé-configuré.
       Aucune ligne n'est supprimée : la valeur scellée est vidée.

POST   /api/notifications/test
       → { "channels": [ { "channel": "webhook", "attempted": true,
                           "ok": false, "error": "le webhook a répondu 404 : …" },
                         { "channel": "smtp", "attempted": false,
                           "ok": false, "error": null } ] }
```

Le test envoie un **vrai** message sur chaque canal configuré. `attempted:
false` veut dire « canal non configuré » : on ne prétend pas avoir essayé.

`security` par défaut vaut `starttls`, `template` vaut `generic`.

**Champ absent ≠ champ à `null`.** L'interface n'a jamais vu l'URL du webhook
ni le mot de passe SMTP : les lui faire renvoyer pour corriger un port
reviendrait à les effacer un jour sur deux. Omettre `password` ou `url` garde
la valeur déjà scellée ; envoyer `null` (ou `""`) la retire. À la première
configuration, `url` est obligatoire — il n'y a rien à conserver.

```
GET /api/notifications/subscriptions
    → { "sites":  [ { "site_id": "…", "name": "Durand", "enabled": true } ],
        "probes": [ { "probe_id": "…", "name": "Paris", "site_id": "…",
                      "site": "Durand",
                      "exception": null,   // ce qui est réglé SUR la sonde
                      "enabled": true } ]  // ce qui s'applique
      }

PUT /api/notifications/subscriptions
    { "scope": "site" | "probe", "scope_id": "…",
      "enabled": true | false | null }
    → 200 { "ok": true }   ·   409 portée inconnue
```

**L'héritage est résolu côté serveur.** `enabled` dit ce qui s'applique,
`exception` dit ce qui est réglé sur la sonde — `null` signifie « suit son
site ». Recalculer cette résolution dans l'interface est exactement l'endroit
où les deux se mettraient à diverger.

`enabled: null` sur une sonde la rend à son site. Ce n'est pas une suppression :
la ligne reste, avec `enabled` à NULL.

`delay_secs` se règle par `PUT /api/settings` sous la clé `notify_delay_secs` —
c'est le seul réglage de notification qui n'est pas un secret, il a donc sa
place dans `settings` comme les autres.

### Ce que la boucle fait, et ce qu'elle ne fait pas

Elle passe toutes les 30 secondes ; c'est `notify_delay_secs` qui décide de la
réactivité, pas la cadence de la boucle. Une sonde est déclarée hors ligne
quand son dernier battement remonte à plus que le délai.

Trois cas se taisent volontairement :

- une sonde **jamais vue** — on ne revient pas d'un état où l'on n'a jamais
  été, et une machine enrôlée qu'on n'a pas encore branchée n'est pas une
  panne ;
- une sonde **révoquée** — elle a cessé d'émettre parce qu'on le lui a demandé ;
- la **première évaluation** d'une sonde — activer les alertes ne rejoue pas
  les pannes en cours. L'interface les montre déjà ; ce qu'on attend d'une
  notification, c'est ce qui change après.

Chaque transition émise écrit une ligne d'audit `probe.down` ou `probe.up`,
dont le `detail` dit quels canaux ont abouti — jamais vers quelle adresse.

### Limites assumées du client SMTP

Écrit à la main (`smtp.rs`) : `EHLO`, `STARTTLS`, TLS implicite (465),
`AUTH LOGIN` et `AUTH PLAIN`, plusieurs destinataires, corps UTF-8 en base64.
Pas de pièce jointe, pas d'OAuth, pas de `DSN`, pas de pipelining. Le
certificat du relais est vérifié contre le magasin de la plateforme —
contrairement à Influx, joint en local sur un certificat auto-signé dont le hub
épingle l'empreinte, un relais SMTP est une machine distante quelconque, et
accepter n'importe quel certificat livrerait le mot de passe au premier
intermédiaire venu.

## 15. Ce que la sonde raconte d'elle-même ✅ implémenté

Le battement de cœur transporte, en plus de son état :

```json
{ "public_ip": "88.120.x.x", "interface": "en0",
  "local_ips": ["10.6.8.42/24"], "gateway": "10.6.8.1" }
```

L'IP publique s'affiche sur la ligne du parc — c'est elle qui identifie le site
d'un coup d'œil et qui trahit un changement d'opérateur ou une bascule sur un
lien de secours. L'interface et les adresses locales vont sur le tableau de bord
de la sonde, comme dans l'app.

⚠️ **L'IP publique ne se redemande pas à chaque battement.** La connaître exige
un appel sortant vers un service tiers ; le faire toutes les minutes pour chaque
sonde du parc, c'est marteler un service gratuit et publier son trafic. La sonde
la met en cache et ne la rafraîchit **qu'au changement d'interface ou toutes les
6 heures** — elle change rarement, et un changement compte plus que sa fraîcheur.

Le hub garde la **dernière valeur connue** avec sa date : une sonde hors ligne
doit continuer d'afficher l'adresse qu'elle avait, pas un tiret. C'est justement
quand elle ne répond plus qu'on veut savoir où elle était.

### Champs ajoutés à `GET /api/probes`

```json
"public_ip": "88.120.0.1",      // null si inconnue
"public_ip_at": 1756339200,     // date du CHANGEMENT, pas du dernier battement
"interface": "en0",
"local_ips": ["10.6.8.42/24"],  // toujours un tableau, [] si inconnu
"gateway": "10.6.8.1"
```

Ces valeurs ne repassent **jamais** à vide une fois connues, quel que soit le
statut de la sonde. Un changement d'IP publique produit une ligne d'audit
`probe.public_ip_changed` avec l'ancienne et la nouvelle valeur.

### État du rattachement, côté app — `cmd_hub_status`

```json
"state": "off" | "ok" | "degraded" | "broken",
"last_beat_at": 1787914000,   // dernier battement RÉUSSI, conservé en panne
"last_error": null,
"buffered_points": 0
```

⚠️ **`degraded` et `broken` ne se confondent pas.** `degraded` se répare seul au
retour du lien ; `broken` signifie que le hub a **refusé le jeton** — la sonde est
révoquée et réessayer ne servira jamais à rien. Les mélanger laisserait attendre
indéfiniment une reconnexion impossible.

## 16. Configuration des sondes remontée au hub ✅ implémenté

Demandé le 28/08 : le hub connaît l'identité d'une sonde, pas sa configuration.
Si la machine meurt, on ré-enrôle et **on reconfigure tout de mémoire**.

La sonde remonte donc, avec son battement de cœur : cibles surveillées, profils
de scan de ports, interface sélectionnée, planification. Le hub les stocke, les
inclut dans sa sauvegarde, et les **repose** à la sonde après un ré-enrôlement.

Deux bénéfices distincts : une sonde devient **remplaçable**, et on voit d'un
écran ce que chaque site surveille réellement.

⚠️ **Sans les secrets.** La configuration d'une sonde peut contenir un mot de
passe InfluxDB v1 ou un serveur iperf privé, scellés côté sonde. Ils ne montent
pas : le hub reçoit la configuration expurgée, et la sonde garde les siens au
retour. Remonter les secrets ferait du hub un dépôt de mots de passe de tous les
sites, et de sa sauvegarde une cible autrement plus intéressante — pour un
bénéfice nul, puisqu'une sonde ré-enrôlée peut les redemander à son opérateur.

## 17. Portée par site d'un compte ✅ implémenté

Demandé le 29/08 : un compte doit pouvoir ne voir **que certains sites**, ou
tous. C'est ce qui permet de montrer son parc à un client sans lui montrer ceux
des autres.

```sql
CREATE TABLE user_sites (
  username TEXT NOT NULL REFERENCES users(username),
  site_id  TEXT NOT NULL REFERENCES sites(site_id),
  PRIMARY KEY (username, site_id)
);
```

**Aucune ligne = accès à tous les sites.** C'est le cas courant (une équipe qui
gère tout) et il ne doit rien coûter à configurer. Des lignes = restriction à
ceux-là.

⚠️ **Le filtrage se fait en SQL, dans chaque requête**, jamais en écartant des
résultats après coup côté interface. `GET /api/probes`, `/api/sites`,
`/api/probes/{id}/metrics`, l'audit et les abonnements aux alertes doivent tous
respecter la portée — et `GET /api/probes/{id}` d'une sonde hors portée répond
`404`, pas `403` : dire « ça existe mais pas pour vous » révèle déjà l'existence
du site d'un autre client.

⚠️ Un `admin` n'est jamais restreint : il gère les comptes, donc il pourrait
lever sa propre restriction en trois clics. Une limite qu'on peut retirer
soi-même n'est pas une limite, et prétendre le contraire serait pire que ne rien
promettre.

## 18. Ce qu'un rôle ne peut pas faire ne s'affiche pas

Demandé le 29/08 : « cacher les options qu'un user ne peut pas utiliser ».

Un `viewer` voit aujourd'hui le « + » d'ajout de sonde. Il le tente, il se prend
un refus. **Ne pas l'afficher vaut mieux** : un bouton qui refuse apprend une
règle au prix d'une frustration ; un bouton absent ne pose pas la question.

⚠️ **Ça ne remplace pas le contrôle côté serveur, ça s'y ajoute.** Le masquage
est du confort ; la protection reste la vérification de rôle sur chaque route.
Une interface qui cache sans que le serveur refuse ne protège rien du tout.

`GET /api/me` donne le rôle avant le premier clic — c'est ce qui rend le
masquage possible sans sonder une route interdite et salir le journal d'audit.

## 19. Mon compte : mot de passe ✅, TOTP ✅, passkeys (à faire)

Un onglet « Mon compte » dans les réglages, accessible à **tous les rôles** :
changer son mot de passe, activer une double authentification, gérer ses clés
d'accès. Aujourd'hui seul un `admin` peut réinitialiser un mot de passe, et
personne ne peut changer le sien.

### Par ordre de faisabilité

**1. Changer son propre mot de passe** — petit, et manquant. Exiger l'ancien :
une session volée ne doit pas suffire à verrouiller le compte de son
propriétaire.

**2. TOTP** — secret scellé (`secrets.rs`), vérifié à la connexion.

⚠️ **Des codes de secours, ou rien.** Un hub auto-hébergé n'a **aucun** chemin
de réinitialisation : pas d'e-mail de récupération, pas de support. Un
administrateur qui perd son téléphone perd son hub, et la seule issue est
d'éditer SQLite dans le conteneur. Les codes de secours ne sont pas un confort,
ils sont la condition pour que la fonctionnalité soit acceptable.

**3. Passkeys (WebAuthn)** — le plus gros, et **il ne marchera pas partout** :

⚠️ WebAuthn exige un **contexte sécurisé** — HTTPS, ou `localhost`. Or le hub
sert **en clair par défaut**, derrière le reverse proxy de l'utilisateur. Sur une
instance jointe par `http://10.0.30.12:18080`, le navigateur refusera de créer
une clé, quelle que soit la qualité de notre code.

La fonctionnalité doit donc se désactiver d'elle-même et **dire pourquoi** —
« les clés d'accès demandent une connexion HTTPS » — plutôt que d'échouer avec
une erreur de navigateur incompréhensible. Et elle n'est jamais le seul facteur :
un utilisateur qui perd sa clé garde son mot de passe.

### Le masquage ne dispense pas du contrôle

Confirmé par le propriétaire : on cache les boutons **et** on vérifie côté
serveur. Le masquage évite la tentation ; la vérification évite l'accès. Sans la
seconde, il suffit d'appeler la route à la main.

## 20. Ce qui est testé, et ce qui ne l'est pas

⚠️ **Il n'y a pas de CI de test.** `.github/workflows/release.yml` est déclenché
par un tag et construit des binaires ; rien ne s'exécute sur un push. `cargo
test` comme `npm test` sont donc des gestes **manuels**, à faire avant de
déployer.

### Rust — `cargo test`

Couvre le hub et la sonde : routes, rôles, sauvegarde/restauration, conversion
Flux, construction du line protocol. C'est la partie solide.

### Interface — `npm test`

`svelte-check` puis Vitest (`web-ui/vitest.config.ts`), sur les fonctions pures
de `web-ui/src/lib` : `metrics.ts`, `charts.ts`, `format.ts`.

Ces tests existent pour une raison datée. Le 29/08/2026, **trois défauts
d'affichage** ont atteint la production alors que le backend rendait la bonne
réponse et que tous les tests Rust passaient :

1. Les en-têtes de colonnes de la vue de parc avaient disparu — en déplaçant
   les règles `.cols`, leurs enveloppes `@media` avaient été perdues, et
   l'en-tête affichait cinq colonnes quand les lignes en avaient sept.
2. Les trois graphiques d'une sonde restaient vides malgré des milliers de
   points en base : le hub décore le libellé d'une courbe de ses étiquettes
   (`latency_ms · 10.0.30.12`) et l'interface cherchait `latency_ms`.
3. Le second moteur de speedtest n'était jamais tracé — un `.find` là où il
   fallait un `.filter`.

Aucun n'était visible côté Rust. **Ce qui casse à cette frontière, c'est la
lecture de la réponse, pas sa production** — d'où des tests sur des fonctions
pures plutôt que sur des composants montés : un test de fonction pure ne dérive
pas avec le rendu, et ces trois défauts y étaient tous attrapables.

Le CSS, lui, reste hors de portée d'un test unitaire. Le défaut n°1 se serait
vu à l'œil, pas dans une assertion : une capture avant/après reste nécessaire
pour toute modification de mise en page.
