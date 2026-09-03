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
{ "needs_setup": true, "version": "1.2.0", "restart_required": false }
```
Seule route accessible sans authentification quand `needs_setup` est vrai.

⚠️ **`restart_required` dit que le hub sert encore l'état d'AVANT.** C'est un
drapeau du **processus** : rien ne le baisse, c'est le redémarrage qu'il réclame
qui le fait disparaître. Deux causes, vérifiées toutes les deux :

- **après une restauration de sauvegarde** — le processus tient encore l'ancienne
  base **ouverte** et continue de la servir. Sans ce drapeau, le hub refuse
  d'écrire (« readonly database ») et rien à l'écran ne l'explique : on conclut
  que la restauration n'a rien fait, et on la rejoue ;
- **après un changement de `tls_enabled`** — le mode de service (clair ou TLS) est
  lu au **démarrage**. Sans ce drapeau, on enregistre, on recharge en `https://`,
  on tombe sur rien, et on croit avoir cassé son hub.

Il est porté par `/api/status` pour que l'avertissement **survive au rechargement
de la page** : un message affiché une fois après l'action disparaîtrait au premier
rafraîchissement, alors que la condition, elle, dure jusqu'au redémarrage.

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
### `GET /api/probes/{id}/metrics` — requête Flux proxifiée

Le navigateur ne parle jamais directement à Influx : le jeton de lecture reste
côté serveur.

| Paramètre | Défaut | Rôle |
|---|---|---|
| `measurement` | — | filtre sur `_measurement`. Absent ou vide : tout ce que la sonde écrit. |
| `range` | `-24h` | fenêtre glissante (`-30m`, `-7d`, `-4w`…) |
| `start`, `stop` | — | bornes explicites, **en secondes epoch** |
| `max_points` | `800` | points affichables — en pratique la largeur du graphe en pixels |

**`start`/`stop` priment sur `range`**, et seulement s'ils tiennent debout : les
deux présents, et `stop > start`. Des bornes incomplètes ou inversées retombent
sur la fenêtre glissante — une fenêtre vide rendrait un rapport à 0 %, qu'on lit
comme une panne totale plutôt que comme une saisie fautive.

⚠️ Ces deux bornes ont été **déclarées puis ignorées en silence** : demander « du
20 au 22 » rendait la dernière heure, sous une légende qui annonçait trois jours,
et l'interface compensait en sur-tirant tout depuis le début de la plage pour
découper elle-même. Un paramètre accepté et ignoré est pire qu'un paramètre
refusé. La même fenêtre sert désormais à la clause Flux, au tableau par adresse
publique et au calcul du pas d'agrégation : un seul endroit décide de la période,
deux calculs séparés finiraient par légender une courbe avec la période d'une
autre.

⚠️ **Une fenêtre glissante illisible rend `400 { "error": "fenêtre illisible" }`,
jamais un repli muet.** `range=nawak` ou `range=12x` retombaient en silence sur une
heure : l'écran affichait une heure sous une légende qui annonçait autre chose, et
rien ne permettait de s'en apercevoir. Un refus se comprend, un repli muet non.

⚠️ **Le signe est normalisé : `24h` et `-24h` désignent tous les deux le passé.**
En Flux une durée **positive** part de maintenant vers l'**avant** — `range(start:
24h)` visait les 24 h à venir, Influx refusait, et le hub rendait **502**, donc
« InfluxDB est en panne » pour une saisie parfaitement anodine. Même faute
d'honnêteté que le `500` sur une sonde inconnue : elle envoie chercher une panne
qui n'existe pas. Unités acceptées : `s`, `m`, `h`, `d`, `w`, et rien d'autre —
l'alphabet est clos par construction, jamais un fragment de Flux ne sort d'ici.

**Les cinq routes de fenêtre se comportent pareil** — `/metrics`, `/sla`,
`/sla.csv`, `/uptime` et `/public-ips` : mêmes paramètres, même refus, même
période. Il n'y a pas d'exception à retenir.

🔴 **Et ce n'est plus une règle à appliquer route par route : la chaîne n'est plus
lue qu'UNE fois.** Deux lectures cohabitaient — l'une pour écrire la clause Flux,
l'autre pour obtenir des secondes — et elles ne comprenaient pas la même chose. Le
refus et la durée pouvaient donc se contredire, et l'ont fait **trois** fois :

1. `24h` visait le **futur** dans la clause Flux (`502`) et le passé en secondes ;
2. `nawak` était refusé d'un côté et valait 24 h de l'autre — d'où le repli muet
   resté sur `/public-ips` quand les autres routes avaient été corrigées ;
3. une valeur numérique **énorme** (`99999999999999999999h`) partait en `502` par
   un chemin et retombait sur 24 h par l'autre.

Une seule lecture, une seule décision : la clause Flux, le libellé rendu et les
bornes en secondes viennent maintenant du même objet, construit une fois par
requête. C'est ce qui ferme la porte, là où corriger chaque route la laissait
ouverte pour la suivante.

```json
{ "range": "-24h",
  "aggregated_every": "15m",
  "series": [ { "field": "latency_ms · 1.1.1.1", "measure": "latency_ms",
                "tags": { "ip": "1.1.1.1" },
                "points": [ { "t": 1756339200000, "v": 12.4 } ] } ] }
```

`range` reprend la fenêtre demandée, sous la forme `"<start>..<stop>"` quand elle
est explicite. `field` est le **libellé** — le champ décoré de ses étiquettes — et
`measure` le nom **nu** du champ : c'est `measure` qu'on compare, jamais le
libellé. Chercher `latency_ms` dans `latency_ms · 10.0.30.12` est l'un des trois
défauts du 29/08 (voir « Ce qui est testé »). Les étiquettes qui ne distinguent
rien ici (`probe_id`, `site_id`, `probe`, `site`, `host` — la requête porte déjà
sur une seule sonde) sont retirées : gardées, elles décoreraient identiquement
toutes les courbes.

#### Quand le hub agrège — et quand il ne le fait pas

⚠️ **On agrège parce que les points ne tiennent pas dans le graphe, jamais parce
que la plage est longue ou parce qu'elle est ancienne.** Dix minutes prises il y
a trois mois se rendent **brutes**, sans le moindre `aggregateWindow` : les points
existent, ils tiennent dans la carte, il n'y a aucune raison de les moyenner.
Quatre-vingts jours à un ping par seconde font sept millions et demi de points par
cible, que le navigateur n'affichera jamais.

La règle tient en une comparaison : **durée de la fenêtre en secondes ≤
`max_points` → points bruts**. Sinon le pas est le quotient, arrondi à l'échelon
**lisible** supérieur (`1s`, `2s`, `5s`, `10s`, `15s`, `30s`, `1m`… jusqu'à `24h`
et `7d`). Une fenêtre plus large que le dernier échelon retombe sur lui, jamais
sur « brut ».

La durée est un **majorant du nombre de points**, pas un compte : à une mesure
par seconde, une fenêtre de *n* secondes en porte au plus *n*. Le hub ne sait pas
combien de points existent sans les lire, et sur-agréger un peu vaut mieux que de
découvrir le débordement une fois la réponse partie. Arrondir vers le haut
garantit en prime de ne jamais dépasser la largeur demandée — et « moyenné par
30 min » se lit là où « moyenné par 1 847 s » ne se lit pas.

`max_points` est **borné à 100..5000** : la valeur vient du **navigateur**, `0`
diviserait par zéro et `10 000 000` rendrait la protection inopérante.

⚠️ **`aggregated_every` est ABSENT de la réponse quand elle est brute** — jamais
`null`, jamais `"1s"`. C'est ce qui permet à l'interface de n'annoncer une moyenne
que s'il y en a une : légender des mesures prises à la seconde par « moyenné par
1 s » serait une affirmation fausse, du même genre que celle qu'on corrige ici.

#### ⚠️ Ce qu'on agrège dépend de ce qu'on mesure

Quand le hub agrège, il ne traite pas toutes les séries de la même façon — et
l'asymétrie est voulue. C'est exactement le genre de chose qu'un relecteur pressé
« corrige » par cohérence apparente.

| Type de série | Agrégation | Champs rendus |
|---|---|---|
| latence, débits… (valeurs continues) | `mean` **et** `max` | `latency_ms` (la moyenne) et `latency_ms_max` |
| disponibilité — `alive`, `icmp_ok`, `dns_ok`… (booléens) | `mean` **seule** | `alive`, qui **est** le taux |
| texte — `state`, `server`, `result_url` | aucune, rendu brut | le champ, tel quel |

Le nom **nu** reste à la moyenne, pour que l'interface qui cherchait `latency_ms`
continue de le trouver ; seul l'extrême prend un suffixe.

⚠️ **Sur une valeur continue, c'est le maximum qui préserve l'incident.** Une
moyenne sur deux heures noie un pic à 800 ms dans une valeur à 12 ms : la courbe
reste crédible et l'incident disparaît. Une valeur plausible et fausse est pire
qu'une valeur absente.

🔴 **Sur une disponibilité, ni minimum ni maximum : un TAUX.** `max(alive)` vaut 1
dès qu'**un seul** ping passe — cinquante minutes de coupure sur une heure
s'afficheraient « tout va bien ». Mais `min(alive)` vaut 0 dès qu'un seul ping
échoue, et un plancher entre 0 et 1 ne se lit pas davantage. La bonne réponse n'est
aucun des deux : « 95 % sur la période » se lit d'un coup, et c'est exactement ce
que `mean` sur un booléen converti en 0/1 calcule. Le support visuel d'une coupure
reste la bande verte et rouge, pas un extrême.

⚠️ **Il n'existe donc aucun champ `alive_min` ni `alive_max`.** `alive_min` a
existé quelques heures le 02/09 avant d'être retiré (`64612dd`) : ne pas le
réintroduire au nom de la symétrie avec `latency_ms_max`. La règle commune n'est
pas « un extrême partout », c'est « la valeur qui ne ment pas sur la période ».

Le tri se fait sur le **type Influx**, donc avant `toFloat()` : après conversion,
un booléen ne se distingue plus d'un compteur qui vaut 0 ou 1.

⚠️ Les champs **texte** (`state` d'`internet_status`, `server` d'un speedtest) ne
se moyennent pas et sont rendus **bruts**, dans leur propre `yield` : `mean` sur
une chaîne fait échouer la requête **entière**, donc les trois graphes de la fiche
d'un coup.

⚠️ **`createEmpty: false`, sans exception.** Un intervalle où rien n'a été mesuré
ne produit **aucun point** — ni zéro, ni valeur interpolée. Un trou reste un trou.
Même règle que le `confirmed_until` du §15 : on ne déduit jamais un fait d'un
silence. Un zéro inventé se lirait comme une latence nulle sur une courbe et comme
une coupure sur une autre ; les deux sont faux, et aucun des deux ne se voit.

### `GET /api/probes/{id}/sla` — les relevés bruts d'un rapport

Même construction de fenêtre que les métriques : `range`, ou `start`/`stop` qui
priment (voir ci-dessus). ⚠️ `max_points` n'a **aucun effet** ici : rien n'est
agrégé, jamais.

```json
{ "probe": "Paris", "site": "Durand", "range": "-24h",
  "start": null, "stop": null, "generated_at": 1756418401,
  "targets": [ { "ip": "10.0.8.1",
                 "samples": [ { "timestamp": 1756332001, "alive": true,
                                "latency_ms": 3.1 },
                              { "timestamp": 1756332002, "alive": null,
                                "latency_ms": 4.0 } ],
                 "coverage": { "window_secs": 86401, "covered_secs": 75600,
                               "gap_secs": 10801 } } ],
  "internet": [ … ], "speedtests": [ … ],
  "discovery": { … } | null, "ports": { … } | null,
  "public_ip_history": [ … ] }
```

`start` et `stop` sont **renvoyés tels qu'ils ont été demandés** — `null` sur une
fenêtre glissante. `timestamp` est en **secondes**, pas en millisecondes comme les
points des métriques : c'est l'unité des relevés de l'application, et le rapport
doit pouvoir mélanger les deux origines.

⚠️ **Rendus bruts, jamais agrégés**, et c'est délibéré : le rapport ne se contente
pas de moyennes, il liste les coupures avec leur durée. Un résumé calculé par le
hub l'obligerait à refaire ce que l'application sait déjà faire, et les deux
finiraient par diverger sur la définition d'une coupure.

⚠️ **Ce que ce rapport ne prétend pas savoir.** Il ne rend que ce qui a été
**mesuré** : une période sans relevé n'y produit aucun échantillon, et n'est donc
ni « en panne » ni « disponible » — elle est **indéterminée**. Les pourcentages
calculés par-dessus ne totalisent volontairement pas 100 % ; le reste est la part
assumée d'inconnu (§15). Une coupure imputée à la mauvaise période, ou une période
muette comptée comme disponible, a l'air normale et personne ne la remet en cause :
c'est le mode de panne que ce projet refuse partout ailleurs.

L'historique des adresses voyage **avec** le rapport plutôt qu'à côté : le tableau
de l'écran et l'onglet du classeur découpent alors les mêmes relevés avec les mêmes
intervalles. Deux sources séparées finiraient par diverger d'une minute et
donneraient deux disponibilités différentes pour la même période.

Le volet `internet` est le seul qui **dégrade** : si sa requête échoue, le rapport
part sans lui plutôt que d'échouer en entier — l'absence se voit à l'écran.

#### 🔴 « Disponible » : une seule définition, et elle exclut le silence

C'est **le** mot qu'il ne faut pas laisser diverger : trois calculs — les
échantillons de `/sla`, les taux de `/sla.csv`, la route `/uptime` — et un seul
sens. Le contrat est l'endroit qui doit l'empêcher, parce qu'une seconde
définition ne se voit jamais : les deux rendent un pourcentage crédible.

**Un relevé est disponible quand son champ `alive` vaut vrai, indisponible quand
il vaut faux, et INDÉTERMINÉ quand la sonde ne l'a pas écrit.** `alive` est donc
un booléen **ou `null`**, jamais un booléen seul.

⚠️ **Ce qui n'a pas été mesuré sort du dénominateur** : il ne le gonfle pas et ne
le déflate pas. Un indéterminé n'est ni un succès ni un échec — le compter comme
un échec donnerait 50 % là où la seule mesure existante dit 100 %.

⚠️ **Il n'y a plus de repli sur « une latence existe ».** Ce repli faisait d'une
latence écrite un verdict de disponibilité : toutes les mesures antérieures au
champ `alive` ressortaient à **100 %**, un chiffre que personne n'avait, dans un
document remis au client. C'est le silence transformé en fait, la faute que ce
dépôt refuse partout ailleurs.

⚠️ **Les latences, elles, restent comptées même sur un relevé indéterminé.** Une
latence écrite **est** une mesure vraie : l'écarter perdrait de l'information
réelle à cause d'un verdict manquant. Moyenne, minimum, maximum et p95 portent
donc sur tous les relevés qui ont une latence, pas seulement sur les déterminés.

🔴 **Sans un seul relevé déterminé, il n'y a pas de pourcentage** : le taux vaut
`null`, jamais `0`. Rendre `0` affirmerait une panne **totale** — pire encore que
le faux 100 % qu'on corrigeait, et sur le même document. Le type l'interdit
(`Option<f64>` côté Rust, `number | null` côté navigateur) plutôt qu'un drapeau à
côté : un drapeau finit toujours par ne pas être lu.

Le nombre de relevés **indéterminés** est compté et rendu, jamais tu : « 100 % sur
trois relevés » et « 100 % sur trois relevés et neuf mille indéterminés » ne se
lisent pas pareil.

⚠️ Côté navigateur, la même règle impose `alive === false` là où on écrirait
spontanément `!alive` : la négation est vraie pour `null`, et le détecteur de
coupures **ouvrait une coupure inventée à chaque indéterminé** — le faux rouge que
la règle cherche précisément à éviter, réintroduit par une commodité d'écriture.

Une cible qui n'a **aucun** relevé sur la fenêtre n'apparaît pas dans le rapport :
elle n'y figure pas à 0 %.

#### La couverture : ce que la fenêtre a réellement mesuré

Un taux ne dit rien de sa propre assise. « 100 % » sur trois relevés d'une fenêtre
de trois mois est vrai et trompeur à la fois. Chaque cible porte donc sa
**couverture**, calculée par le hub et transportée avec le rapport :

```json
"coverage": { "window_secs": 86401, "covered_secs": 75600, "gap_secs": 10801 }
```

🔴 **Le seuil au-delà duquel un espacement devient un trou est l'intervalle
MÉDIAN observé, multiplié par trois — jamais une cadence supposée.** À 60 s de
cadence, 70 s d'écart est normal ; à 1 s, c'est un trou. Écrire « au-delà de 3 s »
supposerait la seconde et redeviendrait faux, en silence, le jour où la cadence
sera réglable. Seules les données tranchent. Le facteur 3 est celui du parc : deux
relevés manqués arrivent, trois d'affilée décrivent autre chose.

⚠️ **Les bords de la fenêtre comptent.** Entre le début de la fenêtre et la
première mesure, puis entre la dernière et la fin, ce sont aussi des trous — et
comptés **en entier**, puisqu'aucun relevé ne les ouvre. Les oublier ferait passer
une sonde enrôlée hier pour couvrant « 100 % » d'une fenêtre de 87 jours : le faux
le plus gros possible, et le plus facile à laisser passer. Un trou intérieur, lui,
ne compte que son **excédent** : le pas normal est déjà couvert par le relevé qui
l'ouvre.

⚠️ **Moins de deux relevés ne donne aucun intervalle, donc aucune médiane** : la
fenêtre est alors intégralement un trou. On n'attribue aucune durée à un point
isolé — ce serait inventer la cadence que ce calcul refuse justement de supposer.

⚠️ **Rien n'est affiché quand la couverture est complète.** C'est une information
de **complétude, pas une alerte** : les trous d'aujourd'hui viennent surtout d'une
période antérieure à la fonctionnalité et doivent tendre vers zéro. Une mention
permanente serait du bruit qui finit par masquer le cas où elle compte. Et une
fenêtre **vide** n'est pas complète : sans cette garde, `gap_secs == 0` la
déclarait « tout va bien » sur une période qui n'existe pas.

La couverture est calculée **une seule fois, dans le hub**, et transportée — jamais
recalculée dans le navigateur. Le classeur remis au client et l'export CSV doivent
annoncer la même complétude ; deux calculs finiraient par diverger d'un point, et
c'est celui du document qu'on ne saurait plus expliquer.

### `GET /api/probes/{id}/public-ips` — l'historique des adresses, même fenêtre

```json
{ "intervals": [ { "public_ip": "88.120.0.1", "interface": "en0",
                   "gateway": "10.6.8.1", "local_subnet": "10.6.8.0/24",
                   "confirmed_from": 1756332000, "confirmed_until": 1756418400,
                   "label": "Bureau" } ] }
```

Les intervalles qui **chevauchent** la fenêtre, du plus ancien au plus récent, et
**non rognés** : `confirmed_from` / `confirmed_until` restent les dates réelles de
confirmation (§15). Les couper aux bornes demandées confondrait « l'adresse a été
confirmée jusqu'ici » avec « ma fenêtre s'arrête ici » — c'est à l'affichage de
rogner, pas à la source.

⚠️ **Ce que cette route ne prétend pas savoir.** Le **trou** entre le
`confirmed_until` d'un intervalle et le `confirmed_from` du suivant **est** la zone
indéterminée, et rien ne le comble : ni prolongation jusqu'au suivant, ni
interpolation. Une fenêtre antérieure à la mise en place de l'historique ne
contient donc **aucun** intervalle et ressort entièrement en « indéterminé » —
c'est la vérité, pas une panne, et c'est à l'interface de le dire ainsi. Une sonde
sans historique rend un tableau vide : l'absence d'intervalle n'est pas une erreur.

⚠️ **Une sonde inexistante rend `404 { "error": "sonde inconnue" }`**, comme sur
les cinq autres routes de la fiche. Un tableau vide **affirmerait** que la sonde
existe et n'a jamais changé d'adresse : sur un identifiant mal saisi, c'est une
réponse plausible et fausse, et elle se lit exactement à l'envers de la vérité.
« Aucun intervalle » et « cette sonde n'existe pas » sont donc deux réponses
distinctes, et le cas légitime est intact : une sonde qui **existe** sans
historique rend toujours `200` avec un tableau vide.

🔴 **Une sonde hors portée rend le même code et le même message qu'une sonde
inexistante — et c'est une propriété, pas une négligence.** Distinguer les deux
apprendrait à un compte restreint que la sonde d'un autre client existe (§17),
c'est-à-dire exactement ce que la portée protège. Elle est tenue sous test
(`the_address_history_hides_an_out_of_scope_probe_behind_the_same_404`) parce
qu'un correctif bien intentionné qui « améliore les messages d'erreur » la casserait
sans y penser : le message le plus utile est ici celui qui en dit le moins.

Le libellé de réseau (« Maison », « Bureau ») est joint ici, résolu par le **site**
de la sonde : sous CGNAT deux clients partagent l'adresse publique, et souvent la
même `192.168.1.1` de box — la jointure par site évite qu'un libellé remonte dans
le rapport d'un autre.

### `GET /api/probes/{id}/sla.csv` — le même rapport, aplati

`text/csv; charset=utf-8`, en pièce jointe `sla-<sonde>-<fenêtre>.csv`. Treize
colonnes :

```
sonde,site,fenetre,cible,disponibilite_pct,latence_moy_ms,latence_min_ms,
latence_max_ms,latence_p95_ms,releves,echecs,indetermines,couverture
```

`disponibilite_pct` suit **la même** définition de « disponible » que `/sla` — voir
ci-dessus. `releves` est le nombre de relevés **déterminés**, c'est-à-dire le
dénominateur du pourcentage ; `indetermines` compte ceux qui n'ont pas de verdict,
et il est là pour qu'un « 100 % sur trois relevés » ne se lise pas comme un « 100 %
sur neuf mille ». Les colonnes de latence comptent tous les relevés qui portent une
latence, indéterminés compris.

⚠️ **Deux cellules ne sont JAMAIS numériques, et c'est structurel :**

| Cellule | Valeurs | |
|---|---|---|
| `disponibilite_pct` | `99.87` … ou le mot `indetermine` | ni vide, ni `0.00` |
| `couverture` | `complete`, `87.4 % mesures`, ou `indetermine` | jamais `87.4` seul |

Une cellule **vide** se lit comme un oubli de l'outil ; `0.00` se lit comme une
panne totale. Et surtout : **un tableur agrège ce qui est numérique**. Un `0` ou un
`87.4` posé là se retrouve dans une moyenne de colonne et fait renaître, dans le
document remis au client, le chiffre exact que le hub a refusé de calculer. Le mot
ferme cette porte de derrière — c'est sa fonction, pas sa présentation.

⚠️ **La couverture ne s'affiche pas quand elle est complète** : la cellule dit
`complete` plutôt qu'un « 100 % » qu'on finirait par moyenner lui aussi.

La route honore `start`/`stop` comme les autres (`2c59ca1`) : le libellé de la
fenêtre, le nom du fichier et la requête viennent tous du même calcul, de sorte que
la période lue et la période annoncée ne peuvent plus diverger. ⚠️ Elle lisait
`range` seul jusque-là : demander « du 1er au 31 » rendait les dernières 24 h dans
un fichier qui partait chez un client. **Le bon en-tête sur les mauvaises données
est le pire des deux mondes** — rien, dans le fichier, ne permet de s'en apercevoir.

⚠️ **Aucun écran ne l'appelle.** L'export de l'interface est un classeur construit
dans le navigateur à partir de `/sla`. Cette route est une URL à ouvrir à la main —
écrite, testée, sans appelant, exactement comme la route de retrait du §20 l'a été.
Ne pas supposer qu'elle est branchée quelque part.

### `GET /api/probes/{id}/uptime` — le taux par cible, agrégé côté Influx

Mêmes paramètres de fenêtre que `/metrics` et `/sla` ; `404` sur une sonde
inconnue, `400` sur une fenêtre illisible.

```json
{ "range": "-24h",
  "targets": [ { "ip": "10.0.8.1", "uptime_pct": 99.87, "samples": 8640  },
               { "ip": "10.0.8.20", "uptime_pct": null,  "samples": 0 } ] }
```

⚠️ **Ce n'est pas un doublon de `/sla`, c'est l'autre bout du même chiffre.**
`/sla` rend des relevés **bruts** et la fiche se rafraîchit toutes les trois
secondes : le rejouer pour afficher un pourcentage ferait relire des millions de
points par tour d'horloge. Ici Influx ne renvoie qu'**une ligne par cible**.

🔴 **Le même chiffre que `compute_sla`, et c'est tenu sous test.** `mean()` d'un
booléen converti en 0/1 **est** le taux, arithmétiquement identique au calcul brut.
Un test compare les deux chemins sur les mêmes données et casse si l'un dérive :
deux pourcentages différents sur le même écran seraient pires que pas de
pourcentage du tout.

Un relevé indéterminé sort du dénominateur **par construction** et non par
précaution : le filtre `_field == "alive"` ne sélectionne que les points qui
portent un verdict, donc `count()` ne les voit pas. `samples` est ce dénominateur.

`uptime_pct` vaut **`null`** — jamais `0` — dès qu'aucun relevé déterminé n'existe
sur la fenêtre. Même règle que partout : `0 %` se lirait comme une panne totale.

⚠️ **La couverture ne figure PAS dans cette réponse, délibérément.** Un chemin
agrégé n'a pas les horodatages : la déduire des intervalles vides donnerait une
résolution plus grossière que celle du rapport, donc deux chiffres contradictoires
sur la même donnée. Quand la complétude compte, c'est `/sla` qui la porte.

### `GET /api/probes/{id}/inventory?kind=ports|discovery|speedtest`

Le détail des scans vient de **SQLite**, pas d'Influx (§12).

- `kind=ports` ou `discovery` → `{ "scan": … | null }`, le **dernier** scan connu.
  ⚠️ `null`, et non un scan vide : « jamais lancé » et « lancé, rien trouvé »
  appellent deux phrases différentes à l'écran.
- `kind=speedtest` → `{ "speedtests": [ … ] }`, les 50 derniers tests.
- `kind` absent ou inconnu → `400`. Une sonde inconnue → `404`.

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

## 5. Tampon hors ligne (côté sonde) ✅ implémenté

Le tampon (`export_buffer.rs`) est la mémoire courte de la sonde entre deux
écritures réussies. **Il ne se vide qu'après confirmation de la destination** :
perdre les points parce qu'elle ne répond pas, c'est perdre la trace de
l'incident qu'on voudrait analyser — et la 1.1.5 le faisait, `buffer.clear()`
étant appelé *avant* l'écriture.

Cinq propriétés le tiennent, et elles sont **acquises** :

1. **Rien ne quitte le tampon avant confirmation.** L'écriture emporte tout le
   lot ; seuls les `n` points confirmés sont retirés, **par la tête**. Les points
   nés pendant l'écriture restent en attente — les retirer par un décompte global
   les emporterait sans qu'ils aient jamais été envoyés. Un échec ne retire rien
   du tout et allonge le repli.
2. **Persistance disque** — `buffer.ndjson` dans le répertoire de config, relu au
   démarrage : une coupure de courant pendant l'incident n'efface pas la trace de
   l'incident. L'écriture est **atomique** (fichier temporaire puis renommage),
   sans quoi un arrêt brutal laisserait un tampon tronqué. Elle a lieu toutes les
   dix secondes, et **forcée à l'arrêt propre** — réécrire cent mille lignes
   chaque seconde coûterait plus cher que le service lui-même. Le prix de ce délai
   est un éventuel rejeu de quelques secondes déjà écrites : sans conséquence,
   Influx écrasant un point de mêmes mesure, étiquettes et horodatage.
   ⚠️ Une ligne illisible du fichier est **ignorée**, pas fatale : mieux vaut
   rejouer ce qui reste lisible que refuser de démarrer.
3. **Borné explicitement** — `MAX_POINTS = 100 000` points **et** `MAX_AGE = 24 h`.
   Au débordement on jette **les plus anciens**, et le nombre exact est à la fois
   rendu à l'appelant et journalisé, en distinguant les deux causes (trop vieux /
   trop nombreux). Un tampon qui déborde en silence ment sur la complétude des
   données.
4. **Horodatage préservé** — l'horodatage est lu dans la ligne Line Protocol au
   moment où elle entre dans le tampon, et conservé tel quel. L'instant courant ne
   sert qu'à mesurer l'âge : un point rejoué retombe au bon endroit dans le
   graphe, jamais à l'heure de l'envoi.
5. **Repli exponentiel** — 1 s, doublé à chaque échec, **plafonné à 60 s**, remis à
   zéro par la première écriture réussie. On ne martèle pas un Influx qui
   redémarre.

⚠️ **Une seule chose vide le tampon sans avoir écrit** : l'utilisateur qui coupe
lui-même l'export. C'est explicite, journalisé avec le nombre de points
abandonnés, et réservé à ce cas — **une panne, elle, ne fait jamais oublier**.

La profondeur du tampon remonte au hub à chaque battement (`buffered_points`,
§3) : une sonde qui accumule sans livrer se voit **avant** qu'on découvre le trou
dans les graphes.

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

Le rôle minimum est appliqué par middleware après authentification. Les rôles
sont ordonnés : `viewer < operator < admin`, et une route ouverte à `operator`
est donc aussi ouverte à `admin`.

⚠️ **La portée de site (§17) s'AJOUTE au rôle, elle ne le remplace pas** : un
lecteur restreint à un site reste un lecteur sur ce site. Les routes marquées
« portée » ci-dessous sont celles dont le `{id}` désigne **une sonde** ; elles
vivent derrière un garde séparé, et une sonde hors portée y rend `404`.

| Route | Rôle | Note |
|---|---|---|
| `GET /api/status`, `POST /api/setup`, `POST /api/login`, `POST /api/login/passkey/{start,finish}`, `POST /api/logout` | — | authentification propre |
| `POST /api/probes/enroll`, `.../write`, `.../heartbeat`, `.../scans`, `.../config` | — | code d'enrôlement, ou jeton de sonde en `Authorization: Bearer` (§12, §16) |
| `GET /api/sites` | `viewer` | accepte aussi les identifiants du compte (Basic) |
| `GET /api/me`, et les routes `/api/me/*` — mot de passe, TOTP, clés d'accès | `viewer` | chacun agit sur **son** compte, quel que soit son rôle (§19) |
| `GET /api/probes` | `viewer` | |
| `GET /api/settings`, `GET /api/settings/influx-advertise`, `GET /api/influx` | `viewer` | aucune de ces réponses ne porte de secret |
| `GET /api/notifications/subscriptions` | `viewer` | |
| `GET /api/probes/{id}/metrics`, `/uptime`, `/inventory`, `/sla`, `/sla.csv`, `/public-ips` | `viewer` | **portée** |
| `POST /api/sites`, `PATCH`/`DELETE /api/sites/{id}`, `POST /api/sites/{id}/archive` | `operator` | |
| `PUT /api/networks/label` | `operator` | nommer un réseau est une conduite de parc : le libellé se retrouve dans tous les rapports. Le `site_id` du corps est vérifié contre la portée — `404` sinon, et `404` aussi si le site n'existe pas |
| `POST /api/enroll-codes`, `GET /api/enroll-codes/pending` | `operator` | le code y figure **en clair** pendant sa validité |
| `PATCH`/`DELETE /api/probes/{id}`, `POST .../archive` | `operator` | **portée** |
| `POST /api/probes/{id}/rotate`, `/revoke-token`, `/reenroll-code` | `operator` | **portée** |
| `POST /api/probes/{id}/realtime` | `operator` | **portée** — arme le mode temps réel, minuterie comprise |
| `POST /api/probes/{id}/monitors/remove`, `/monitors/revive` | `operator` | **portée** — §20 |
| `GET`/`POST /api/probes/{id}/commands` | `operator` | **portée** — la lecture aussi : la file nomme ce qu'on a déclenché sur le réseau d'un client (§14) |
| `POST /api/settings/influx-advertise/test` | `operator` | émet une requête sortante |
| `PUT /api/notifications/subscriptions` | `operator` | |
| `PUT /api/settings`, `PUT /api/settings/influx-advertise` | `admin` | la rétention en fait partie |
| `POST /api/influx/read-token`, `GET`/`DELETE /api/influx/read-tokens[/{id}]` | `admin` | un jeton qui quitte le hub |
| `GET`/`POST /api/users`, `PATCH /api/users/{username}`, `POST /api/users/{username}/password` | `admin` | |
| `GET /api/audit` | `admin` | |
| `GET /api/notifications`, `PUT`/`DELETE /api/notifications/{smtp,webhook}`, `POST /api/notifications/test` | `admin` | |
| `POST /api/backup`, `GET /api/backups`, `POST /api/backup/restore`, `POST /api/backup/restore/{file}` | `admin` | une archive porte **toute** la base, portée comprise : la remettre revient à donner le parc entier |

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
`probe.up`, `probe.command`, `probe.monitor_remove`, `probe.monitor_revive`,
`probe.public_ip_changed`, `probe.archive`, `probe.unarchive`, `site.archive`,
`site.unarchive`, `user.scope`, `user.password_reset_cli`, `backup.create`,
`backup.restore`, `backup.retention`.

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
la met en cache et la relève :

- quand la **passerelle ou une IP locale** change — ⚠️ changer de réseau Wi-Fi garde
  souvent le **même nom d'interface** (`en0` reste `en0`), l'interface seule ne suffit
  donc pas : c'était le défaut d'origine, l'adresse pouvait rester fausse six heures ;
- à chaque **transition d'état internet** (`offline` → `online`) — on vient de
  raccrocher, souvent sur un autre lien ;
- quand le **hub le demande** (`recheck_public_ip`, ci-dessous) ;
- sinon toutes les **6 heures**, en dernier recours.

⚠️ **Plancher : au plus un relevé forcé toutes les 5 minutes par sonde.** Une sortie
multi-WAN en répartition de charge alterne l'adresse source à chaque battement ; sans
plancher on martèlerait le service tiers, ce que le TTL long cherchait justement à
éviter. Le plancher est un compteur en mémoire, pas un réglage.

### `recheck_public_ip` — le hub détecte, la sonde mesure

Le hub compare l'adresse source du battement à celle du battement précédent. Si elle a
changé, il ajoute `"recheck_public_ip": true` à sa réponse.

⚠️ **L'adresse vue par le hub n'est JAMAIS enregistrée.** Le battement peut sortir par
la route par défaut alors que les mesures sont liées à une autre interface : on
retomberait sur une adresse plausible et fausse au milieu de mesures honnêtes, c'est-à-dire
sur le défaut que le relevé par interface source corrige déjà. Le hub déclenche, la
sonde mesure.

⚠️ **Ce déclencheur est muet quand le hub est dans le LAN des sondes** — l'adresse source
est alors privée et ne dit rien de l'adresse publique. C'est le cas courant en
auto-hébergement ; les déclencheurs locaux ci-dessus le couvrent.

Une sonde antérieure à ce champ l'ignore et garde son comportement : dégradation, pas
panne.

### `trusted_proxies` — réglage, table `settings`

Liste de CIDR séparés par des virgules. Vide (défaut) : on lit l'adresse de la socket et
on **ignore** `X-Forwarded-For`. Renseignée : on remonte l'en-tête de droite à gauche et
on s'arrête au premier saut hors liste.

⚠️ Sans ce réglage, deux échecs symétriques : derrière un reverse proxy toutes les sondes
portent l'adresse du proxy ; en acceptant l'en-tête naïvement, n'importe qui forge la
sienne — l'en-tête est écrit par le client.

⚠️ **`0.0.0.0/0` et `::/0` sont REFUSÉS.** C'est la saisie naturelle de qui se dit « je suis
derrière un proxy, je ne connais pas sa plage, j'autorise tout » — et elle rend **tous** les
sauts de confiance : le hub retiendrait le premier de la chaîne, celui que le client écrit
lui-même. Le réglage aurait l'air posé sans plus rien protéger. Aucun montage n'en a besoin :
un proxy local s'écrit `127.0.0.1/32`, et sur socket unix il n'y a pas d'adresse de pair.
Une plage réelle, même large, reste acceptée — on refuse le fourre-tout, pas la permissivité
assumée.

⚠️ Une liste dont **aucune** entrée n'est un CIDR lisible est refusée elle aussi : les entrées
fautives sont écartées pour qu'un réglage à moitié juste protège le reste, mais une faute de
frappe sur la seule entrée donnerait une liste vide, c'est-à-dire un réglage silencieusement
inerte.

### Historique des adresses

Le hub tient un intervalle par adresse : `public_ip`, `interface`, `gateway`,
`local_subnet`, `confirmed_from`, `confirmed_until`. L'identité réseau y est un
**instantané figé** : après un changement, elle décrit le réseau tel qu'il était.

⚠️ **`confirmed_until` est le dernier battement ayant CONFIRMÉ l'adresse, pas l'instant
où le changement a été détecté.** Un battement sans adresse — une sonde sans accès
internet — ne prolonge rien. Conséquence, et c'est tout l'intérêt : **le trou entre le
`confirmed_until` d'un intervalle et le `confirmed_from` du suivant EST la zone
indéterminée**. Une coupure suivie d'un retour sur la **même** adresse lui est imputée ;
une coupure suivie d'une adresse **différente** n'est imputée à personne.

Le SLA par adresse en découle sans cas particulier, et ses pourcentages ne totalisent
volontairement pas 100 % : le reste est la part assumée d'inconnu. Un « 99,2 % » collé à
la mauvaise adresse a l'air normal et personne ne le remet en cause — c'est le mode de
panne que ce projet refuse partout ailleurs.

Les libellés de réseau (« Maison », « Bureau ») vivent dans une table séparée, clé
`(site_id, public_ip, gateway)` : ils survivent à la purge des intervalles et se réappliquent quand
le réseau revient. La passerelle fait partie de la clé parce que deux réseaux derrière un
même opérateur partagent l'adresse publique.

⚠️ **Rien de tout cela ne va dans InfluxDB** (§12) : une étiquette par adresse ferait une
série par adresse, et un poste itinérant en voit des dizaines.

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

### Quelle interface la sonde mesure — et comment on la désigne sans écran

Les champs `interface`, `gateway` et `local_ips` ci-dessus n'existent que si une
interface a été **désignée**. Sans désignation, la sonde suit la route par défaut
du système et ne rapporte **ni passerelle ni adresses locales** : la colonne
« Passerelle » du rapport SLA et les libellés de réseau (§15) restent vides à
jamais. En mode graphique le choix se fait à l'écran ; **sans écran, il n'existait
aucun moyen de choisir** — ni option, ni commande.

```
lanprobe-server interfaces          # liste : nom, adresse, passerelle, état
lanprobe-server run    --interface en0
lanprobe-server enroll --interface en0 --hub … --code …
```

⚠️ **La liste n'est pas un confort.** Sans écran, choisir à l'aveugle n'est pas un
choix : c'est elle qui rend l'option utilisable. Elle imprime l'adresse et la
passerelle à côté de chaque nom, faute de quoi on ne reconnaît pas le bon lien sur
une machine qui en expose dix-huit — le cas d'un hôte Docker ordinaire.

L'option n'est déclarée que sur les **deux commandes qui agissent**. Globale, elle
était acceptée puis ignorée en silence par `status` et `forget` : une option qui a
l'air prise en compte et ne fait rien est exactement le défaut qu'on corrige. Le
nom est retenu (`selected_interface` dans `app_config.json`) et relu **avant** tout
démarrage de tâche — une mesure sortie par la mauvaise interface est pire qu'une
mesure absente, elle a l'air normale.

⚠️ **Deux règles asymétriques, et l'asymétrie est le sujet :**

| Cas | Comportement |
|---|---|
| nom **demandé** en ligne de commande, inconnu | **échec immédiat**, en listant les interfaces qui existent |
| nom **enregistré** devenu introuvable | **gardé**, et signalé par un avertissement |

Sans écran, une faute de frappe ne se voit pas : retomber en silence sur « aucune
interface » donnerait une sonde qui **a l'air configurée et ne l'est pas**. À
l'inverse, un adaptateur USB débranché ou un VPN arrêté revient — l'oublier
rendrait la sonde muette sur son réseau sans rien dire, et refuser de démarrer
l'empêcherait de mesurer pour un câble.

### Ce que la liste d'interfaces dit d'un lien

| Champ | Sens |
|---|---|
| `is_up` | état **opérationnel** (RFC 2863) : la porteuse est là, le câble est branché |
| `admin_up` | drapeau **administratif** `UP` : l'interface est activée |
| `gateway` | passerelle **portée par cette interface**, ou rien |

⚠️ **Les deux états ne se confondent pas, et ne se fondent pas en un seul
booléen.** Le drapeau `UP` reste vrai sur un lien sans porteuse : un `docker0`
débranché s'affiche `<NO-CARRIER,…,UP>` avec `state DOWN`. « Activé, sans
porteuse » envoie vérifier le câble ; « inactif » envoie l'activer. Un booléen
unique ne distinguait pas les deux gestes — et `is_up` valait `true` **en dur**,
donc une interface débranchée se présentait comme active, exactement là où on la
choisit à l'aveugle.

C'est ce que la liste rend lisible en un mot, et l'état est la première chose à
lire : `actif`, `activé, sans porteuse` (vérifier le câble) ou `inactif`
(l'activer). Choisir une interface débranchée donne une sonde muette, et le
diagnostic part alors dans toutes les directions sauf la bonne.

⚠️ `state UNKNOWN` compte comme opérationnel, et c'est une décision : le noyau
l'emploie pour les liens sans détection de porteuse — tun, tap, WireGuard — le plus
souvent fonctionnels. Les compter éteints fabriquerait une panne, l'erreur
symétrique de celle qu'on corrige.

⚠️ **La passerelle n'est rendue qu'à l'interface qui la porte.** `ip route show
default` décrit la route par défaut de la **machine**, pas celle de l'interface
interrogée : la recopier sur toutes attribuait la même passerelle aux cinq `veth*`
et aux douze `br-*` d'un hôte Docker, dont ceux qui n'ont pas d'adresse. Une
passerelle en face d'un `veth` invite à le choisir, et une sonde posée sur le
mauvais lien mesure faux sans jamais le dire. Ailleurs : **rien**, pas de valeur
« neutre » — une absence doit se lire comme une absence.

🔴 **Limite connue, écrite parce qu'elle est connue : `is_up` et `admin_up` sont
encore codés en dur à `true` sur macOS et sur Windows.** Seul Linux lit l'état réel
du lien. `networksetup -getinfo` n'expose pas l'état de la porteuse, et la sonde de
production tourne sur macOS : le rattrapage est un chantier à part. En attendant,
sur ces deux plateformes, ces deux champs n'apprennent **rien** — les lire comme un
diagnostic serait se fier à une constante. La passerelle, elle, y est déjà correcte :
les deux systèmes l'interrogent par interface.

### Ce que la sonde n'écrit pas

⚠️ **Un échec local ne s'écrit jamais comme une panne de la cible.** Trois
situations font sauter le relevé plutôt que d'inventer une réponse :

- une **bascule de profil réseau** en cours — l'adresse est brièvement absente ;
- l'**interface choisie n'a pas d'IPv4** — câble débranché, lien tombé, bail DHCP
  perdu ;
- les **sockets ICMP sont épuisés**, typiquement par un scan réseau en cours.

Le deuxième cas écrivait `alive: false` pour **chaque cible surveillée, une fois
par seconde**, jusque dans Influx et donc dans le rapport SLA remis au client.
C'est un fait sur **notre** machine, pas sur la cible. Un faux rouge invente une
panne et fait déplacer quelqu'un pour rien : il use la confiance dans l'outil plus
vite qu'un faux vert. « Ne rien inscrire vaut mieux qu'inscrire une panne qui n'a
pas eu lieu » — et sauter le relevé évite le mauvais lien tout autant que
d'inventer un verdict, sans fabriquer d'incident.

Ces trous se lisent ensuite comme ce qu'ils sont : de l'**indéterminé** dans la
couverture du §3, jamais de l'indisponibilité.

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

### Ce que la garde garantit — « inconnu » veut dire inconnu

C'est une **propriété du hub**, pas une consigne à répéter route par route :

> Toute route dont le `{id}` désigne une **sonde** ou un **site** rend
> `404 { "error": "sonde inconnue" }` — ou `« site inconnu »` — aussi bien sur un
> objet **inexistant** que sur un objet **hors portée**, **quel que soit le rôle
> de l'appelant, administrateur compris**.

Les deux cas sont **indiscernables**, et c'est le but : distinguer « ça n'existe
pas » de « ça existe mais pas pour vous » apprend déjà l'existence du parc d'un
autre client. Une seule vérification répond donc aux deux questions — l'objet
existe, et l'appelant a le droit de le voir — et rend la même réponse quand l'une
des deux manque.

Deux mécanismes symétriques la portent : un **intergiciel** pour les quinze routes
dont le `{id}` du chemin est un `probe_id`, et une **vérification de site** appelée
par les handlers qui reçoivent un site — `PATCH`/`DELETE`/`archive` d'un site, la
création d'un code d'enrôlement, le libellé de réseau, l'abonnement d'alerte, et la
**destination** d'un déplacement de sonde.

⚠️ **Un handler gardé n'a donc plus à vérifier l'existence pour son propre
compte** : il peut supposer que la sonde du chemin existe. Ce qu'il doit encore
vérifier, c'est ce qui n'est **pas** dans l'URL — l'intergiciel ne lit que le `{id}`
du chemin, et un site nommé dans un corps de requête lui est invisible. C'est
exactement ce que faisait déjà `PATCH /api/probes/{id}` pour sa destination : sans
ce contrôle, un opérateur restreint déplaçait une sonde chez un autre client, et la
perdait de vue au passage.

⚠️ Sur `PUT /api/notifications/subscriptions`, la cible n'est pas dans l'URL et
peut être un site **ou** une sonde : le refus y prend la forme
`404 { "error": "cible inconnue" }`. Même code, même indiscernabilité, message
adapté à ce qu'on ne dit pas.

⚠️ **Effet de bord à connaître : sur un objet inconnu, un corps invalide rend
`404` et non `400`.** La garde est une couche, elle parle **avant** que le corps
de la requête soit lu. C'est le bon ordre — « cette sonde n'existe pas » est la
plus vraie des deux réponses, et corriger son JSON n'aurait rien changé.

### ⚠️ L'exception assumée : les routes à jeton de sonde rendent `401`

`POST /api/probes/{id}/write`, `/heartbeat`, `/scans` et `/config` ne sont **pas**
derrière cette garde et ne doivent pas y passer. Une sonde inconnue, une sonde
révoquée et un mauvais jeton y rendent tous les trois le **même**
`401 { "error": "jeton de sonde invalide" }`.

**Ce n'est pas une incohérence, c'est la même règle appliquée à un autre
interlocuteur.** Ici l'appelant n'a pas de session : il présente un jeton. Lui
répondre `404` sur une sonde inexistante lui apprendrait, par différence, **quelles
sondes existent** — avec un jeton quelconque, on énumère le parc. Le `401`
indiscernable est ce qui l'en empêche. Ne pas « harmoniser » ces quatre routes sur
le `404` des autres.

### 🔴 La règle de test : ces gardes se testent avec un compte `Scope::All`

C'est le cœur du sujet, et ça n'a rien d'un détail d'implémentation.

La garde court-circuitait pour les comptes qui voient tout : l'existence n'était
alors vérifiée que par **effet de bord**, dans le `get_probe` que la vérification
de portée fait pour les comptes **restreints**. Une route qui ne contrôlait rien
elle-même **paraissait** gardée, l'était pour un opérateur, et ne l'était pas pour
l'administrateur — l'utilisateur le plus susceptible de taper un identifiant à la
main dans une URL.

⚠️ **Un test écrit avec un compte restreint reste vert quoi qu'on casse sur ce
chemin.** C'est un angle mort de la suite de tests, pas trois oublis de codage : il
a laissé le **même** défaut revenir trois fois. Tout test de cette garantie doit
donc être écrit avec un compte `Scope::All` — sinon il ne teste rien de ce qui a
cassé, et il redevient rouge le jour où le court-circuit revient.

Même leçon côté site : la vérification de portée d'un site tenait, elle aussi, par
effet de bord — un `UPDATE` qui ne touche aucune ligne finit par se voir. ⚠️ Elle a
donc lâché **exactement là où l'effet de bord n'avait pas lieu** :
`notify_subscriptions.scope_id` ne porte **aucune clé étrangère**, et un
administrateur écrivait en `200` un abonnement d'alerte sur un site inexistant, qui
s'affichait ensuite comme un réglage actif sur un parc imaginaire. Une garantie qui
tient par effet de bord n'est pas une garantie : c'est une coïncidence qui dure.

#### Historique — trois occurrences du même défaut

| Route | Réponse plausible et fausse | Corrigée par |
|---|---|---|
| `POST /api/probes/{id}/monitors/{remove,revive}` | `500 FOREIGN KEY constraint failed` | `f8629b6` |
| `GET /api/probes/{id}/public-ips` | `200 { "intervals": [] }` | `c89ddf2` |
| `GET /api/probes/{id}/commands` | `200 { "commands": [] }` | `9f95a12` |
| `PUT /api/notifications/subscriptions` | `200`, abonnement écrit sur un site inexistant | `9f95a12` |

Les trois premières ont d'abord été corrigées **route par route**, et le défaut est
revenu à chaque fois ailleurs. `9f95a12` l'a repris **au niveau de la garde** : la
quatrième occurrence serait arrivée la semaine suivante. Le coût est d'un
`get_probe` par requête, que quatorze des quinze routes gardées faisaient déjà pour
leur propre compte.

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

## 19. Mon compte : mot de passe ✅, TOTP ✅, clés d'accès ✅

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

## 20. Surveillances partagées entre l'app et le hub ✅ implémenté

Avant : **la sonde était seule maîtresse de sa liste**. Elle l'annonçait à chaque
battement, le hub en déduisait tout, et une cible **absente de l'annonce était traitée
comme retirée**.

⚠️ **C'est ce qui effaçait en silence ce qu'on ajoutait depuis le hub** : on ajoutait
`8.8.8.8` côté hub, l'app ajoutait `1.1.1.1` de son côté, annonçait sa liste locale — et
`8.8.8.8` disparaissait. Rien n'échouait, rien n'alertait. Constaté en production le
02/09/2026.

### Le principe

⚠️ **Une suppression est un fait EXPLICITE, jamais une absence.**

Même règle que le `confirmed_until` du §15 : ne jamais déduire un fait d'un silence. Là,
« je n'ai pas relevé » était confondu avec « c'était en panne » ; ici, « je ne l'ai pas
annoncée » avec « je l'ai retirée ».

- la liste effective est l'**union** de ce que connaissent l'app et le hub ;
- retirer **écrit une suppression datée**, conservée — c'est elle qui alimente le
  sous-onglet « Retirées » ;
- **réactiver, c'est effacer la suppression**. Le « revive des deux côtés » en découle,
  sans mécanisme dédié.

### Arbitrage — le plus récent gagne

Chaque changement porte une date ; le plus récent l'emporte. Le cas « la sonde était
éteinte » ne demande aucune règle particulière : sa liste est simplement antérieure.

⚠️ **La sonde n'envoie JAMAIS de date, mais une ANCIENNETÉ** (`age_secs`), mesurée sur
l'horloge **monotone** — celle qui ne recule pas, ignore NTP, les changements d'heure et
les réveils de veille. Le hub la convertit dans sa propre référence.

**Aucune horloge n'est jamais comparée à une autre.** Synchroniser les deux a été envisagé
et écarté : ça dépend de NTP, ça dérive, et un portable qui sort de veille saute de
plusieurs minutes.

⚠️ **`age_secs` est bornée à 30 jours, et une valeur négative est refusée.** Hors bornes,
le changement est daté à la réception. Une valeur aberrante — bug, champ corrompu, sonde
malveillante — placerait sinon un changement en 1970 ou dans le futur et gagnerait **tous**
les arbitrages à l'envers.

⚠️ **Limite connue — et elle ne frappe QU'UN des deux chemins.**

La file de la sonde est bien **persistée** (`hub_monitor_changes` dans
`app_config.json`) : un geste fait pendant que le hub est injoignable survit à un
redémarrage de l'app. Ce qui ne survit pas, c'est l'horloge **monotone** sur laquelle
son ancienneté est mesurée — aucune horloge monotone ne survit à l'arrêt d'un
processus. Au redémarrage, le geste repart donc avec l'ancienneté qu'il avait quand elle
a été écrite : **la durée pendant laquelle l'app était arrêtée n'est pas comptée.**

Conséquence : un retrait fait **depuis l'app** alors qu'elle est hors ligne revient
« rajeuni » de la durée de l'arrêt, et peut gagner un arbitrage contre un ajout qui lui
est postérieur dans les faits.

⚠️ **Le retrait fait depuis le hub n'est pas concerné** : il est daté de l'horloge du
hub à la seconde du clic, et écrit en base tout de suite. Les deux chemins ne portent
donc pas le même risque — une raison de plus de passer par la route du hub quand on l'a
sous la main.

C'est une sous-estimation **assumée**. L'alternative — dater sur l'horloge murale — saute
au réveil de veille et se fait corriger par NTP : elle ferait gagner des arbitrages au
hasard, sans que rien ne le signale. Un décalage borné, toujours dans le même sens et
connu vaut mieux qu'un décalage imprévisible.

### Au battement

```json
"monitor_changes": [
  { "target": "10.0.8.20", "action": "add",    "age_secs": 45 },
  { "target": "8.8.4.4",   "action": "remove", "age_secs": 11520 }
]
```

Le hub répond `monitors` (liste effective, retraits compris) et **`monitor_acks`** (les
cibles prises en compte). ⚠️ La sonde ne retire un changement de sa file **qu'une fois
accusé** — même mécanisme que les commandes du §14. Un changement arbitré **perdant** est
quand même accusé : il a été tranché, le réémettre ne changerait rien.

⚠️ **La sonde ne réactive jamais d'elle-même une cible dont la suppression est plus récente
que son état.** Sans cette règle, une app qui redémarre ressuscite ce qu'on vient de
retirer depuis le hub — le défaut d'origine, inversé.

### 🔴 Un état s'émet même vide ; un acte ne s'émet que s'il a eu lieu

Le battement porte deux choses de nature différente, et la règle d'omission n'est
pas la même :

| Champ | À vide | Pourquoi |
|---|---|---|
| `monitors` — ce qui tourne **maintenant** | **émis**, `[]` | un **état**. `[]` veut dire « je ne surveille plus rien », et le hub l'applique |
| `monitor_changes` — ce qui a été **décidé** | omis | des **actes**. Zéro acte n'apprend rien |
| `command_acks` — ce qui a été **exécuté** | omis | idem, et c'est le cas de loin le plus fréquent |

Le hub distingue explicitement le champ **absent** — « je n'en sais rien », une
sonde antérieure au mécanisme, on garde la liste connue — du tableau **vide**, qui
efface. ⚠️ Or la sérialisation omettait le tableau vide, rendant ce « rien » tout
simplement **impossible à dire** : retirer sa dernière cible n'effaçait jamais rien,
et le parc affichait des surveillances mortes indéfiniment. Une absence redevenait
une suppression — le défaut même que ce paragraphe existe pour supprimer, revenu par
le seul cas de la liste vide.

⚠️ **La règle n'est donc pas « tout émettre ».** Un état doit pouvoir dire « rien » ;
un acte n'a rien à dire quand il n'y en a pas. Émettre `[]` pour les actes à chaque
battement et pour tout le parc n'apprendrait rien à personne — et le hub n'en déduit
aucun état, il ne les fusionne nulle part.

### Routes — c'est par elles que l'interface agit

⚠️ **Ces deux routes sont le chemin qui fait autorité, et elles sont nommées ici
pour cette raison.** Elles ont existé deux jours, écrites et testées, sans **aucun
appelant** côté interface : le bouton « Retirer » empilait une commande
`remove_monitor` (§14), qui n'est distribuée qu'au **battement suivant**. Il
marchait donc sonde allumée et **échouait en silence sonde éteinte** —
c'est-à-dire précisément dans le cas qui motive toute la fonctionnalité. La cause
n'était pas un oubli de code : cette section décrivait le principe et l'arbitrage
**sans nommer une seule des deux routes HTTP** qui les déclenchent. Ce qui n'est
pas dans le contrat ne se branche pas.

```
POST /api/probes/{id}/monitors/remove    { "target": "8.8.8.8" }
POST /api/probes/{id}/monitors/revive    { "target": "8.8.8.8" }
     → 200 { "ok": true, "applied": true | false }
     400 cible vide  ·  403 rôle insuffisant  ·  404 sonde hors portée
```

- **Rôle `operator`**, comme tout ce qui touche une sonde : retirer ou réactiver
  une surveillance change ce qui pingue sur le réseau d'un client, ce n'est pas
  une consultation.
- **Portée de site** : une sonde hors portée rend **404** — pas `403` — et rien
  n'est écrit. « Ça existe mais pas pour vous » révèle déjà l'existence du site
  d'un autre client (§17).
- `target` est rogné de ses espaces ; vide, la route rend `400` sans rien écrire.
- Chaque appel écrit une ligne d'audit : `probe.monitor_remove`,
  `probe.monitor_revive`.

⚠️ **`applied: false` n'est pas une erreur, et ne doit pas être tu.** Il dit qu'un
changement **au moins aussi récent** avait déjà tranché pour cette cible : rien
n'a été écrit, et un écran qui afficherait quand même « retirée » affirmerait ce
que le hub vient de refuser. Rejouer le même geste à la même seconde ne réécrit
rien non plus — sans quoi un battement répété ferait avancer la date d'un état qui
n'a pas bougé.

⚠️ **Le retrait est écrit immédiatement, daté de l'horloge du hub**, sans attendre
la sonde. C'est toute la différence avec la commande : la sonde s'y aligne à son
prochain battement, qu'elle soit là ou non, et l'arbitrage « le plus récent
gagne » le fait tenir face à la liste que rapportera une machine éteinte —
antérieure par construction. Une commande, elle, ne fait rien tant que la sonde ne
revient pas, et se perd si elle ne revient jamais.

La réponse dit **« c'est écrit côté hub »**, pas « la sonde a cessé de pinguer » :
le hub ne joint jamais la sonde. L'interface doit le formuler ainsi.

⚠️ **Rien n'a été retiré au passage** : le type de commande `remove_monitor`
existe toujours et la route de commandes le sert encore. C'est le bouton qui a
changé de chemin, pour le seul qui marche que la sonde soit là ou non.

⚠️ Une sonde **inconnue** rend `404 { "error": "sonde inconnue" }` — **la même
réponse, au mot près, qu'une sonde hors portée**. C'est voulu : une sonde d'un
autre client doit rester indiscernable d'une sonde qui n'existe pas. Avant
`f8629b6`, l'absence de garde laissait la clé étrangère refuser l'écriture et la
route rendait `500 { "error": "FOREIGN KEY constraint failed" }` — un code qui
**affirme que le hub est en panne** quand la vérité est « cette sonde n'existe
pas », et qui livrait au passage le nom d'une contrainte SQL au navigateur.

### ⚠️ Deux champs, deux faits — ce n'est pas une redondance

`GET /api/probes` expose les deux :

- **`monitors`** (colonne `probes.monitors`, annoncée par la sonde) dit **COMMENT** chaque
  cible se porte : `alive`, latence, disponibilité, nombre de relevés. La sonde seule mesure.
- **`probe_monitors`** (table `probe_monitors`, tenue par le hub) dit **QUI** est surveillé
  et depuis quand, avec les retraits datés que la sonde ignore encore.

Fusionner les deux ferait perdre l'un ou l'autre.

⚠️ **Une cible annoncée par la sonde mais absente de `probe_monitors` reste ACTIVE.** Une
sonde antérieure à ce mécanisme n'envoie aucun `monitor_changes` : sa table est vide alors
qu'elle pingue depuis des mois. Lire cette absence comme un retrait viderait l'onglet d'un
parc entier — le défaut du chantier, transposé dans l'interface.

**Compatibilité** : sans `monitor_changes`, le hub garde le comportement d'avant. Une sonde
non mise à jour continue de fonctionner.

## 21. Ce qui est testé, et ce qui ne l'est pas

⚠️ **Il n'y a pas de CI de test.** `.github/workflows/release.yml` est déclenché
par un tag et construit des binaires ; rien ne s'exécute sur un push. `cargo
test` comme `npm test` sont donc des gestes **manuels**, à faire avant de
déployer.

### Rust — `cargo test`

Couvre le hub et la sonde : routes, rôles, sauvegarde/restauration, conversion
Flux, construction du line protocol. C'est la partie solide.

### La doublure InfluxDB refuse un Flux invalide

Les tests du hub ne parlent pas à un vrai Influx : ils parlent à une doublure HTTP.
Elle **répondait `200` à n'importe quoi** — donc **aucun test du dépôt ne pouvait
attraper une requête malformée**. C'est ainsi qu'un `import "experimental/types"`,
paquet qui n'existe pas, a failli partir au vert : la suite entière était verte, et
c'est un influxd jetable, lancé à la main, qui l'a arrêté.

Elle lit désormais le Flux avant de répondre, et refuse en `400` :

- un `import` **absent de la liste des paquets constatés valides** sur influxd
  2.9.1 ;
- un `aggregateWindow` sans `every`, ou avec un `fn:` inconnu ;
- un `range(...)` **vide par construction** — bornes inversées, ou durée positive
  sans `stop`, c'est-à-dire une fenêtre qui vise le futur.

⚠️ **Ce n'est pas un moteur Flux, et ça ne doit jamais le devenir.** Une doublure
qui simule tout devient un second logiciel à maintenir, qui finit par diverger
d'Influx et par produire de **faux échecs** — pires que les faux succès qu'on
corrige, parce qu'ils font perdre confiance dans la suite entière. Elle ne refuse
donc que ce qu'un vrai influxd refuse de façon **certaine**, constaté en le lui
soumettant, et laisse passer tout le reste.

⚠️ Conséquence directe : un paquet absent de la liste est refusé **non parce
qu'Influx le refuserait**, mais parce que **personne ne l'a vérifié**. Pour en
ajouter un : le soumettre à un influxd réel, constater le `200`, puis l'inscrire.

Deux niveaux de test, et il faut les deux : le **verdict** (la fonction de refus,
testée seule) et le **câblage** (un appel HTTP réel à la doublure). Sans le second,
la fonction pourrait être parfaite et n'être appelée nulle part.

Au passage, le hub **remonte la cause** d'un refus d'Influx au lieu de la jeter :
Influx explique toujours ses refus (« invalid import path… », « cannot query an
empty range »), et on n'en gardait qu'un « 400 Bad Request » muet. L'opérateur —
et le test en échec — n'avaient plus rien pour comprendre.

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

### Le périmètre de compilation — `cargo check --workspace`

🔴 **`cargo-lowmem check --workspace --all-targets` fait partie des gestes
manuels d'avant-livraison, au même titre que `cargo test` et `npm test`.**

Il existe pour une raison datée. Le 02/09/2026, la publication a échoué **sur
les trois plateformes** sur une erreur de compilation triviale : `ec83f01`
avait changé `PingSample.alive` en `Option<bool>` dans `lanprobe-core`, et le
site d'appel de `src-tauri` n'avait pas suivi. Personne ne l'a vue parce que
les suites étaient lancées en `-p lanprobe-core -p lanprobe-server -p
lanprobe-web` : **le crate de l'application de bureau n'était dans aucun `-p`**,
et rien ne le compilait donc jamais. Il contenait pourtant déjà des tests
(`src-tauri/src/updater.rs`) que personne n'exécutait.

⚠️ **Le trou n'était pas dans la finesse des tests, il était dans le périmètre
de compilation.** Aucun test supplémentaire n'aurait attrapé cette erreur : un
test ne s'exécute que sur du code compilé, et ce code ne l'était pas. Une suite
ciblée laisse des crates entiers hors du champ, et c'est ce qui a laissé une
erreur de compilation aller jusqu'à la publication.

⚠️ `--all-targets` n'est pas facultatif : sans lui, les `#[cfg(test)]` restent
hors du champ, et un test qui ne compile plus passe inaperçu jusqu'au jour où
on le lance.

⚠️ Ce dépôt **n'a aucune CI de test** (cf. le début de cette section) : le tag
déclenche une construction, qui est le premier moment où le crate de bureau est
compilé. Sans ce geste manuel, la première machine à découvrir qu'un crate ne
compile plus est celle qui produit les binaires livrés.
