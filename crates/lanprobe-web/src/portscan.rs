//! Profils de scan de ports partagés — contrat § 25.
//!
//! Avant : **quatre profils en dur** dans l'interface du hub, aucune base,
//! aucun écran d'édition — et de vrais profils modifiables côté sonde, que le
//! hub ne voyait pas. Ici, le hub fait autorité pour tout le parc.
//!
//! ## Les trois règles que personne ne doit « simplifier »
//!
//! - **Une suppression est un FAIT daté** (`deleted_at`), jamais une absence.
//!   Même leçon que `removed_at` au § 20 : sans la ligne, la sonde ne
//!   distingue pas « le hub ne connaît pas ce profil » de « le hub l'a
//!   supprimé », et le recrée au battement suivant en le remontant.
//! - **Le transport se fait par compteur de révision, jamais par dates.** La
//!   sonde et le hub ne partagent pas d'horloge — c'est déjà pourquoi la file
//!   des surveillances voyage en ancienneté et non en horodatage.
//! - **`ports` vide n'est pas une liste vide** envoyée à la sonde : c'est « la
//!   sonde garde la sienne ». Les confondre ferait un scan complet là où on
//!   croyait restreindre.
//!
//! Le module vit hors de `db.rs` — qui fait déjà 5 700 lignes — comme
//! l'appairage et les rapports avant lui : chaque domaine porte son `impl Db`
//! et emprunte la connexion.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
    Extension, Json, Router,
};
use rusqlite::OptionalExtension;

use crate::db::{Db, DbError, DbResult, Role};
use crate::web::{audited, error_response, guarded, ok_json, AppState, Identity};

/// Compteur de révision du hub. Incrémenté à **chaque** création,
/// modification ou suppression ; chaque ligne porte la révision qui l'a
/// produite.
///
/// 🔴 Ce n'est PAS une date. Il vient du hub, la sonde le range et le rend tel
/// quel : aucune horloge n'entre dans l'affaire, et deux machines qui ne seront
/// jamais d'accord sur l'heure n'ont rien à arbitrer.
pub(crate) const PORTSCAN_REV_KEY: &str = "portscan_profiles_rev";

/// Révision en dessous de laquelle les pierres tombales ont été effacées.
///
/// ⚠️ Sans ce seuil, une sonde qui revient après le nettoyage recevrait un
/// delta **amputé** des suppressions effacées : elle garderait des profils que
/// le hub a retirés, et les remonterait. Elle reçoit donc la liste complète.
pub(crate) const PORTSCAN_PURGED_KEY: &str = "portscan_purged_below_rev";

/// Les profils de base ont-ils déjà été posés ?
///
/// 🔴 Les reposer à chaque démarrage ressusciterait celui qu'on vient de
/// supprimer exprès — une suppression qui se défait toute seule au prochain
/// redémarrage du conteneur.
const PORTSCAN_SEEDED_KEY: &str = "portscan_profiles_seeded";

/// Les profils de base ont-ils été mis à niveau sur les listes de
/// l'application sonde ?
///
/// 🔴 Une seule fois, elle aussi. Rejouée à chaque démarrage, elle réécrirait
/// un profil de base que quelqu'un vient de retoucher — et comme la
/// comparaison avec la valeur d'origine ne vaut qu'une fois (après la mise à
/// niveau, la ligne ne ressemble plus au semis d'origine), elle ne protégerait
/// plus rien.
const PORTSCAN_RESEEDED_KEY: &str = "portscan_profiles_seeded_probe_lists";

/// Au-delà, une sonde cesse de retenir le nettoyage des pierres tombales.
///
/// ⚠️ Une sonde simplement **éteinte** compte, et c'est voulu : le jour où elle
/// revient, elle doit apprendre les suppressions. Mais pas pour l'éternité —
/// une machine oubliée dans un placard figerait la table pour toujours.
const TOMBSTONE_GRACE_SECS: i64 = 90 * 86_400;

/// Un profil de base, tel que le hub le pose à l'installation.
struct SeededProfile {
    id: &'static str,
    name: &'static str,
    ports: &'static [i64],
    udp_ports: &'static [i64],
}

/// Les profils de base du hub, **repris de l'application sonde**
/// (`BUILTIN_PROFILES` dans `src/lib/stores/portscanProfiles.ts`).
///
/// 🔴 Demande de Benjamin (02/10) : « de base les profils en local et sur le
/// hub doivent être les mêmes ». Le hub posait jusque-là quatre listes maigres
/// recopiées de son ancienne interface — `Common` sans aucun port, `Web` avec
/// huit, `Databases` avec huit — alors que la sonde en propose cinq bien plus
/// fournies, UDP compris. Deux vérités pour un seul nom, et l'écran de la
/// sonde montrait les deux côte à côte.
///
/// ⚠️ **Ce sont des points de départ, pas des intouchables** : ils se modifient
/// et se suppriment comme les autres. Rien n'est verrouillé.
///
/// ⚠️ Les deux listes ne peuvent pas partager de source — un module TypeScript
/// du bureau, une constante Rust du hub. Le test
/// `les_profils_de_base_sont_exactement_ceux_de_l_application_sonde` est donc
/// le seul endroit qui dit à quoi elles doivent ressembler : s'il tombe après
/// qu'on a touché aux profils de la sonde, c'est ICI qu'il faut recopier.
const SEEDED_PROFILES: &[SeededProfile] = &[
    SeededProfile {
        id: "common",
        name: "Common",
        ports: &[
            21, 22, 23, 25, 53, 80, 110, 143, 443, 445, 3306, 3389, 5432, 5900, 8080, 8443,
        ],
        udp_ports: &[53, 123, 137, 161, 1900, 5353],
    },
    SeededProfile {
        id: "web",
        name: "Web",
        ports: &[
            80, 443, 8000, 8008, 8080, 8081, 8088, 8181, 8443, 8888, 3000, 5000, 9000,
        ],
        udp_ports: &[],
    },
    SeededProfile {
        id: "db",
        name: "Databases",
        ports: &[
            1433, 1521, 3306, 5432, 5984, 6379, 7000, 9042, 9200, 9300, 11211, 27017, 50000,
        ],
        udp_ports: &[1434],
    },
    SeededProfile {
        id: "remote",
        name: "Remote access",
        ports: &[22, 23, 2222, 3389, 5900, 5901, 5902, 5938, 6000],
        udp_ports: &[],
    },
    SeededProfile {
        id: "full",
        name: "Full (extended)",
        ports: &[
            21, 22, 23, 25, 53, 80, 110, 111, 135, 139, 143, 389, 443, 445, 465, 514, 587, 631,
            636, 873, 993, 995, 1080, 1194, 1433, 1521, 2049, 2222, 2375, 2376, 3000, 3128, 3306,
            3389, 5000, 5060, 5222, 5432, 5672, 5900, 5984, 6379, 6443, 7000, 8000, 8008, 8080,
            8086, 8088, 8181, 8443, 8883, 9000, 9092, 9200, 9300, 11211, 27017, 50000,
        ],
        udp_ports: &[53, 67, 68, 69, 123, 137, 161, 500, 514, 1900, 4500, 5353],
    },
];

/// Les quatre profils que la **première** version du semis (01/10) a posés,
/// repris verbatim de l'ancienne interface du hub.
///
/// 🔴 Ils ne sont plus posés : ils servent à **reconnaître** une ligne que
/// personne n'a touchée depuis. Un hub déjà en service les porte, et c'est le
/// seul moyen honnête de distinguer « encore tel qu'il a été semé » — donc
/// remplaçable — de « quelqu'un l'a modifié » — donc intouchable. Pas de
/// colonne « modifié par un humain » à tenir à jour, aucune supposition.
///
/// ⚠️ Le doublon de `161` dans `infra` est celui du code d'origine, laissé ici
/// exprès : c'est [`normalize_ports`] qui le retire, et la comparaison se fait
/// donc sur la liste normalisée, comme en base.
///
/// ⚠️ `infra` n'a **aucun équivalent** dans l'application sonde. Il reste donc
/// en place, ni mis à niveau ni supprimé : rien ne se supprime ici sans qu'on
/// le demande, et une pierre tombale le ferait disparaître de tout le parc.
const LEGACY_SEEDED_PROFILES: &[(&str, &str, &[i64])] = &[
    ("common", "Common", &[]),
    ("web", "Web", &[80, 443, 8080, 8443, 8000, 8888, 3000, 5000]),
    ("infra", "Infra", &[22, 23, 53, 123, 161, 389, 636, 3389, 5900, 161]),
    ("db", "Databases", &[1433, 1521, 3306, 5432, 6379, 9200, 27017, 5984]),
];

/// Un profil tel que le hub le tient — supprimé compris, avec sa date.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct PortscanProfile {
    pub profile_id: String,
    pub name: String,
    /// ⚠️ **Vide veut dire « la sonde garde sa liste »**, pas « ne scanne
    /// rien » : la sonde traite déjà `[]` comme une absence de restriction.
    pub ports: Vec<i64>,
    /// Les ports UDP, que le hub modélise depuis le 02/10.
    ///
    /// ⚠️ Vide veut dire ici « ce profil ne scanne pas d'UDP », et c'est le cas
    /// de tous les profils d'avant la v27 : personne n'avait pu leur en donner.
    /// La sonde en fait donc autorité dès que le champ est là.
    pub udp_ports: Vec<i64>,
    /// D'où sort ce profil. `None` = créé sur le hub.
    ///
    /// ⚠️ C'est une **trace**, pas un droit : elle ne donne aucune autorité à
    /// la sonde qui l'a créé.
    pub origin_probe: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Non nul = supprimé, et c'est un fait daté qu'on conserve.
    pub deleted_at: Option<i64>,
    /// La révision qui a produit cette ligne.
    pub rev: i64,
}

/// Un profil annoncé par une sonde dans sa configuration (§ 16).
///
#[derive(Debug, Clone)]
pub(crate) struct IncomingProfile {
    pub profile_id: String,
    pub name: String,
    pub ports: Vec<i64>,
    /// 🔴 L'UDP monte avec le profil depuis que le hub le modélise. Sans ça, il
    /// ingérerait un profil en laissant tomber ses ports UDP, puis le
    /// réécrirait à la sonde **sans UDP** : un réglage détruit que plus
    /// personne ne pourrait reconstituer.
    pub udp_ports: Vec<i64>,
}

/// Ce que le hub a à apprendre à une sonde en retard.
#[derive(Debug, Clone)]
pub(crate) struct ProfilesUpdate {
    /// Le delta, ou la liste complète si [`Self::replace`]. Pierres tombales
    /// comprises — c'est le cœur du mécanisme.
    pub profiles: Vec<PortscanProfile>,
    /// La révision à ranger **après** avoir écrit la liste.
    pub rev: i64,
    /// « Remplace tout » : ce n'est pas un delta, c'est l'état du hub.
    pub replace: bool,
}

/// Trie, dédoublonne, et jette ce qui n'est pas un port.
///
/// ⚠️ Les trois à la fois, et au MÊME endroit : `infra` répétait `161` dans le
/// code du hub, ce qui faisait annoncer « 10 ports » pour neuf. Et deux listes
/// identiques à l'ordre près se liraient comme deux profils différents.
pub(crate) fn normalize_ports(ports: &[i64]) -> Vec<i64> {
    let mut out: Vec<i64> = ports
        .iter()
        .copied()
        .filter(|p| (1..=65_535).contains(p))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Les profils de scan présents dans la configuration qu'une sonde dépose
/// (§ 16). Une configuration sans profils rend une liste vide.
///
/// ⚠️ **Les profils de BASE de la sonde sont écartés** (`builtin`). Décision 3 :
/// une sonde garde les siens, seuls ceux qu'elle crée en plus montent. Les
/// faire monter poserait cinq lignes « Common », « Web »… dans la liste
/// commune de tout le parc, en doublon de celles du hub.
///
/// ⚠️ **`udp_ports` monte aussi**, depuis que le hub le modélise : l'ingérer à
/// moitié ferait réécrire le profil à la sonde sans son UDP au battement
/// suivant, et détruirait un réglage que rien ne pourrait reconstituer.
pub(crate) fn profiles_from_probe_config(config: &serde_json::Value) -> Vec<IncomingProfile> {
    let Some(entries) = config.get("portscan_profiles").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    profiles_from_entries(entries)
}

/// Les mêmes entrées, quand elles arrivent par le **battement** et non par le
/// dépôt de configuration (§ 25, « montée depuis les sondes »).
///
/// 🔴 Les deux chemins lisent la même forme par la même fonction, et c'est
/// voulu : le défaut du 02/10 est né de ce que la montée n'existait QUE sur le
/// dépôt de configuration, déclenché par l'interface du bureau à l'édition d'un
/// profil. Un profil créé avant l'enrôlement ne remontait jamais. Deux lectures
/// différentes de la même forme auraient fini par diverger.
pub(crate) fn profiles_from_entries(entries: &[serde_json::Value]) -> Vec<IncomingProfile> {
    entries
        .iter()
        .filter(|entry| entry.get("builtin").and_then(|v| v.as_bool()) != Some(true))
        .filter_map(|entry| {
            let profile_id = entry.get("id")?.as_str()?.trim().to_string();
            if profile_id.is_empty() || profile_id.starts_with("builtin:") {
                return None;
            }
            let name = entry.get("name")?.as_str()?.trim().to_string();
            let ports = port_array(entry, "tcp_ports");
            let udp_ports = port_array(entry, "udp_ports");
            Some(IncomingProfile {
                profile_id,
                name,
                ports,
                udp_ports,
            })
        })
        .collect()
}

/// Une liste de ports d'une entrée annoncée par une sonde.
fn port_array(entry: &serde_json::Value, key: &str) -> Vec<i64> {
    entry
        .get(key)
        .and_then(|v| v.as_array())
        .map(|list| list.iter().filter_map(|p| p.as_i64()).collect())
        .unwrap_or_default()
}

/// La ligne est-elle encore **exactement** celle que le semis du 01/10 a
/// posée ?
///
/// ⚠️ Les trois champs, pas un seul. Un nom retouché, un port ajouté ou un port
/// UDP saisi depuis la v27 du schéma suffisent à dire « quelqu'un s'en est
/// occupé », et le hub n'a alors rien à écraser.
fn is_untouched_legacy(existing: &PortscanProfile) -> bool {
    LEGACY_SEEDED_PROFILES.iter().any(|(id, name, ports)| {
        *id == existing.profile_id
            && *name == existing.name
            && normalize_ports(ports) == existing.ports
            && existing.udp_ports.is_empty()
    })
}

fn poisoned() -> DbError {
    DbError::Internal("verrou SQLite empoisonné".into())
}

/// Colonnes de `portscan_profiles`, dans l'ordre où [`profile_from_row`] les
/// lit. Une seule liste : deux `SELECT` qui divergent d'une colonne se paient
/// en panique au premier appel.
const COLUMNS: &str =
    "profile_id, name, ports, origin_probe, created_at, updated_at, deleted_at, rev, udp_ports";

fn profile_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PortscanProfile> {
    let raw: String = row.get(2)?;
    Ok(PortscanProfile {
        profile_id: row.get(0)?,
        name: row.get(1)?,
        // Une liste illisible vaut « vide », donc « la sonde garde la sienne » :
        // c'est le seul repli qui ne fait pas scanner plus que demandé.
        ports: serde_json::from_str(&raw).unwrap_or_default(),
        origin_probe: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        deleted_at: row.get(6)?,
        rev: row.get(7)?,
        udp_ports: {
            let raw: String = row.get(8)?;
            serde_json::from_str(&raw).unwrap_or_default()
        },
    })
}

impl Db {
    /// La révision courante du hub.
    pub(crate) fn portscan_rev(&self) -> DbResult<i64> {
        Ok(self
            .get_setting(PORTSCAN_REV_KEY)?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0))
    }

    /// Relève le compteur sous ses propres lignes. Appelé **au démarrage**.
    ///
    /// 🔴 Le vrai risque n'est pas le débordement — 64 bits signés —, c'est le
    /// RECUL. Une base restaurée revient à une révision plus ancienne alors que
    /// les sondes ont gardé la leur, et une restauration partielle referait
    /// servir des numéros déjà émis : deux changements différents sous le même
    /// numéro, et une sonde qui en ignore un.
    pub(crate) fn lift_portscan_rev(&self) -> DbResult<()> {
        let highest: i64 = {
            let conn = self.conn().lock().map_err(|_| poisoned())?;
            conn.query_row(
                "SELECT COALESCE(MAX(rev), 0) FROM portscan_profiles",
                [],
                |r| r.get(0),
            )?
        };
        if highest > self.portscan_rev()? {
            self.set_setting(PORTSCAN_REV_KEY, &highest.to_string())?;
        }
        Ok(())
    }

    /// Fait avancer le compteur et rend la révision du changement en cours.
    fn bump_portscan_rev(&self) -> DbResult<i64> {
        let next = self.portscan_rev()? + 1;
        self.set_setting(PORTSCAN_REV_KEY, &next.to_string())?;
        Ok(next)
    }

    /// Pose les profils de base, **une seule fois dans la vie de la base**.
    /// Sans effet s'ils l'ont déjà été.
    pub(crate) fn seed_portscan_profiles(&self) -> DbResult<()> {
        if self.get_setting(PORTSCAN_SEEDED_KEY)?.is_some() {
            return Ok(());
        }
        for seeded in SEEDED_PROFILES {
            // Un conflit de nom n'arrête pas le semis : une base neuve n'en
            // aura pas, et une base bricolée à la main ne doit pas empêcher le
            // hub de démarrer.
            let _ = self.insert_portscan_profile(
                seeded.id,
                seeded.name,
                seeded.ports,
                seeded.udp_ports,
                None,
            )?;
        }
        self.set_setting(PORTSCAN_SEEDED_KEY, "1")?;
        // ⚠️ Une base neuve part déjà des bonnes listes : la mise à niveau
        // n'aurait rien à y faire, et la marquer faite évite qu'elle aille
        // comparer des lignes qu'elle vient d'écrire.
        self.set_setting(PORTSCAN_RESEEDED_KEY, "1")?;
        Ok(())
    }

    /// Met les profils de base aux listes de l'application sonde, **une seule
    /// fois**, et seulement ceux que personne n'a touchés.
    ///
    /// 🔴 Le critère est la **comparaison avec la valeur d'origine** : une
    /// ligne encore identique à ce que le semis du 01/10 avait posé — nom,
    /// ports TCP, et pas un seul port UDP — n'a été modifiée par personne. Tout
    /// le reste est le travail de quelqu'un, et le hub n'y touche pas.
    ///
    /// ⚠️ Un profil de base **supprimé** n'est pas mis à niveau, et surtout pas
    /// recréé : sa pierre tombale est un fait daté, et la ressusciter ferait se
    /// défaire une suppression voulue au premier redémarrage.
    pub(crate) fn refresh_seeded_portscan_profiles(&self) -> DbResult<()> {
        if self.get_setting(PORTSCAN_RESEEDED_KEY)?.is_some() {
            return Ok(());
        }
        for seeded in SEEDED_PROFILES {
            match self.get_portscan_profile(seeded.id) {
                Ok(existing) => {
                    if existing.deleted_at.is_some() || !is_untouched_legacy(&existing) {
                        continue;
                    }
                    self.update_portscan_profile(
                        seeded.id,
                        Some(seeded.name),
                        Some(seeded.ports),
                        Some(seeded.udp_ports),
                    )?;
                }
                // Absent : c'est un profil de base que cette version ajoute
                // (`Remote access`, `Full (extended)`). Un nom déjà pris par
                // quelqu'un n'est pas une erreur — on laisse le sien.
                Err(DbError::NotFound(_)) => {
                    match self.insert_portscan_profile(
                        seeded.id,
                        seeded.name,
                        seeded.ports,
                        seeded.udp_ports,
                        None,
                    ) {
                        Ok(_) => {}
                        Err(DbError::Conflict(e)) => {
                            tracing::info!("profil de base {} non posé : {e}", seeded.id)
                        }
                        Err(e) => return Err(e),
                    }
                }
                Err(e) => return Err(e),
            }
        }
        self.set_setting(PORTSCAN_RESEEDED_KEY, "1")?;
        Ok(())
    }

    /// Pose le semis **d'origine** (01/10). Les tests seuls : c'est l'état
    /// d'un hub déjà en service, celui que la mise à niveau doit reconnaître.
    #[cfg(test)]
    pub(crate) fn pose_legacy_seed_for_tests(&self) {
        for profile in self.portscan_profiles_with_tombstones().unwrap() {
            let conn = self.conn().lock().unwrap();
            conn.execute(
                "DELETE FROM portscan_profiles WHERE profile_id = ?1",
                [&profile.profile_id],
            )
            .unwrap();
        }
        for (id, name, ports) in LEGACY_SEEDED_PROFILES {
            self.insert_portscan_profile(id, name, ports, &[], None).unwrap();
        }
        self.set_setting(PORTSCAN_RESEEDED_KEY, "").unwrap();
        let conn = self.conn().lock().unwrap();
        conn.execute(
            "DELETE FROM settings WHERE key = ?1",
            [PORTSCAN_RESEEDED_KEY],
        )
        .unwrap();
    }

    /// La liste de l'interface : **supprimés exclus**, par nom.
    pub(crate) fn list_portscan_profiles(&self) -> DbResult<Vec<PortscanProfile>> {
        self.query_portscan_profiles("deleted_at IS NULL", [])
    }

    /// Tout ce que la table porte, pierres tombales comprises. Sert au
    /// battement et à l'inspection.
    pub(crate) fn portscan_profiles_with_tombstones(&self) -> DbResult<Vec<PortscanProfile>> {
        self.query_portscan_profiles("1 = 1", [])
    }

    fn query_portscan_profiles<P: rusqlite::Params>(
        &self,
        predicate: &str,
        params: P,
    ) -> DbResult<Vec<PortscanProfile>> {
        let conn = self.conn().lock().map_err(|_| poisoned())?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM portscan_profiles
              WHERE {predicate}
              ORDER BY name COLLATE NOCASE"
        ))?;
        let rows = stmt.query_map(params, profile_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    fn get_portscan_profile(&self, profile_id: &str) -> DbResult<PortscanProfile> {
        let conn = self.conn().lock().map_err(|_| poisoned())?;
        conn.query_row(
            &format!("SELECT {COLUMNS} FROM portscan_profiles WHERE profile_id = ?1"),
            [profile_id],
            profile_from_row,
        )
        .optional()?
        .ok_or_else(|| DbError::NotFound("profil inconnu".into()))
    }

    /// Crée un profil. `origin_probe` à `None` = créé sur le hub.
    pub(crate) fn create_portscan_profile(
        &self,
        name: &str,
        ports: &[i64],
        udp_ports: &[i64],
        origin_probe: Option<&str>,
    ) -> DbResult<PortscanProfile> {
        let name = name.trim();
        if name.is_empty() {
            return Err(DbError::Conflict("le nom du profil est requis".into()));
        }
        let profile_id = new_profile_id();
        self.insert_portscan_profile(&profile_id, name, ports, udp_ports, origin_probe)
    }

    fn insert_portscan_profile(
        &self,
        profile_id: &str,
        name: &str,
        ports: &[i64],
        udp_ports: &[i64],
        origin_probe: Option<&str>,
    ) -> DbResult<PortscanProfile> {
        let rev = self.bump_portscan_rev()?;
        let now = crate::db::now();
        {
            let conn = self.conn().lock().map_err(|_| poisoned())?;
            conn.execute(
                "INSERT INTO portscan_profiles
                   (profile_id, name, ports, udp_ports, origin_probe, created_at, updated_at, rev)
                 VALUES (?1, ?2, ?3, ?7, ?4, ?5, ?5, ?6)",
                rusqlite::params![
                    profile_id,
                    name,
                    serde_json::to_string(&normalize_ports(ports)).unwrap_or_else(|_| "[]".into()),
                    origin_probe,
                    now,
                    rev,
                    serde_json::to_string(&normalize_ports(udp_ports))
                        .unwrap_or_else(|_| "[]".into()),
                ],
            )
            .map_err(|e| {
                crate::db::conflict_on_constraint(e, format!("le profil « {name} » existe déjà"))
            })?;
        }
        self.get_portscan_profile(profile_id)
    }

    /// Renomme et/ou change les ports. `None` laisse le champ intact —
    /// renommer ne doit pas effacer les ports au passage.
    pub(crate) fn update_portscan_profile(
        &self,
        profile_id: &str,
        name: Option<&str>,
        ports: Option<&[i64]>,
        udp_ports: Option<&[i64]>,
    ) -> DbResult<PortscanProfile> {
        let existing = self.get_portscan_profile(profile_id)?;
        // ⚠️ Un profil supprimé ne se modifie pas : ce serait une résurrection
        // déguisée, et la sonde recevrait une ligne vivante pour une
        // suppression qu'elle a déjà appliquée.
        if existing.deleted_at.is_some() {
            return Err(DbError::NotFound("profil inconnu".into()));
        }
        let name = match name.map(str::trim) {
            Some("") => return Err(DbError::Conflict("le nom du profil est requis".into())),
            Some(n) => n.to_string(),
            None => existing.name.clone(),
        };
        let ports = match ports {
            Some(p) => normalize_ports(p),
            None => existing.ports.clone(),
        };
        // ⚠️ Même règle : absent = inchangé. Renommer un profil ne doit pas
        // vider ses ports UDP au passage.
        let udp_ports = match udp_ports {
            Some(p) => normalize_ports(p),
            None => existing.udp_ports.clone(),
        };
        let rev = self.bump_portscan_rev()?;
        {
            let conn = self.conn().lock().map_err(|_| poisoned())?;
            conn.execute(
                "UPDATE portscan_profiles
                    SET name = ?2, ports = ?3, udp_ports = ?6, updated_at = ?4, rev = ?5
                  WHERE profile_id = ?1",
                rusqlite::params![
                    profile_id,
                    name,
                    serde_json::to_string(&ports).unwrap_or_else(|_| "[]".into()),
                    crate::db::now(),
                    rev,
                    serde_json::to_string(&udp_ports).unwrap_or_else(|_| "[]".into()),
                ],
            )
            .map_err(|e| {
                crate::db::conflict_on_constraint(e, format!("le profil « {name} » existe déjà"))
            })?;
        }
        self.get_portscan_profile(profile_id)
    }

    /// Supprime — c'est-à-dire **pose une date**. Aucune ligne ne part.
    ///
    /// ⚠️ Rejouer la suppression ne refait pas avancer le compteur : un
    /// deuxième clic ferait sinon voyager un delta qui n'apprend rien à
    /// personne.
    pub(crate) fn delete_portscan_profile(&self, profile_id: &str) -> DbResult<()> {
        let existing = self.get_portscan_profile(profile_id)?;
        if existing.deleted_at.is_some() {
            return Ok(());
        }
        let rev = self.bump_portscan_rev()?;
        let conn = self.conn().lock().map_err(|_| poisoned())?;
        conn.execute(
            "UPDATE portscan_profiles SET deleted_at = ?2, updated_at = ?2, rev = ?3
              WHERE profile_id = ?1",
            rusqlite::params![profile_id, crate::db::now(), rev],
        )?;
        Ok(())
    }

    /// Ingère les profils qu'une sonde annonce. Rend le nombre de nouveaux.
    ///
    /// Le hub **ignore** ceux qu'il connaît — il fait autorité, la sonde ne
    /// modifie rien — et **ignore** ceux qu'il a supprimés : sinon la
    /// suppression se ferait annuler par la première sonde qui n'a pas encore
    /// battu. Les inconnus entrent, avec `origin_probe` = cette sonde.
    pub(crate) fn ingest_probe_profiles(
        &self,
        probe_id: &str,
        profiles: &[IncomingProfile],
    ) -> DbResult<usize> {
        let mut ingested = 0;
        for incoming in profiles {
            let id = incoming.profile_id.trim();
            let name = incoming.name.trim();
            if id.is_empty() || name.is_empty() {
                continue;
            }
            // Connu ou supprimé : dans les deux cas, rien à faire. La
            // distinction n'a pas à être faite ici, et c'est précisément ce
            // qui rend la suppression increvable.
            if self.get_portscan_profile(id).is_ok() {
                continue;
            }
            // ⚠️ Un nom déjà pris n'est PAS une erreur du battement : la sonde
            // a pu créer « Caméras » pendant qu'on en créait un sur le hub. Le
            // hub garde le sien et le battement aboutit — faire échouer le
            // battement ferait passer une sonde saine pour hors ligne.
            match self.insert_portscan_profile(
                id,
                name,
                &incoming.ports,
                &incoming.udp_ports,
                Some(probe_id),
            ) {
                Ok(_) => ingested += 1,
                Err(DbError::Conflict(e)) => {
                    tracing::debug!("profil {id} de {probe_id} non ingéré : {e}")
                }
                Err(e) => return Err(e),
            }
        }
        Ok(ingested)
    }

    /// Ce que la sonde a à apprendre, sachant la révision qu'elle annonce.
    /// `None` = elle est à jour, et le battement ne porte alors **aucun**
    /// champ de profils.
    pub(crate) fn portscan_profiles_since(&self, announced: i64) -> DbResult<Option<ProfilesUpdate>> {
        let current = self.portscan_rev()?;
        if announced == current {
            return Ok(None);
        }
        let purged_below: i64 = self
            .get_setting(PORTSCAN_PURGED_KEY)?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        // 🔴 Deux cas donnent la liste complète, et ils se répareraient mal
        // autrement :
        //   • `announced > current` — le hub a été restauré et la sonde a gardé
        //     sa révision. Lui répondre « tu es à jour » gèlerait sa liste pour
        //     toujours, sans que rien ne le dise.
        //   • `announced < purged_below` — les pierres tombales qu'elle n'a pas
        //     vues ont été effacées : le delta serait honnêtement incomplet.
        let replace = announced > current || announced < purged_below;
        let profiles = if replace {
            self.portscan_profiles_with_tombstones()?
        } else {
            self.query_portscan_profiles("rev > ?1", [announced])?
        };
        Ok(Some(ProfilesUpdate {
            profiles,
            rev: current,
            replace,
        }))
    }

    /// Range la révision qu'une sonde vient d'appliquer. C'est l'accusé de
    /// réception, et c'est lui qui autorise le nettoyage des pierres tombales.
    ///
    /// ⚠️ Plafonnée à la révision du hub : une sonde qui en annonce une plus
    /// grande (hub restauré) ne doit pas faire croire au parc qu'il a appris
    /// des suppressions qui n'existent pas encore.
    pub(crate) fn note_portscan_rev(&self, probe_id: &str, rev: i64, at: i64) -> DbResult<()> {
        let rev = rev.clamp(0, self.portscan_rev()?);
        let conn = self.conn().lock().map_err(|_| poisoned())?;
        conn.execute(
            "UPDATE probes SET last_profiles_rev = ?2, last_profiles_rev_at = ?3
              WHERE probe_id = ?1",
            rusqlite::params![probe_id, rev, at],
        )?;
        Ok(())
    }

    /// Efface pour de bon les suppressions que **tout le parc** a apprises.
    /// Rend le nombre de lignes parties.
    ///
    /// ⚠️ Les sondes **révoquées ou archivées** ne comptent pas : une machine
    /// partie à la benne n'accusera plus jamais rien et bloquerait le nettoyage
    /// pour toujours. Une sonde simplement éteinte, elle, compte — jusqu'à
    /// [`TOMBSTONE_GRACE_SECS`].
    ///
    /// ⚠️ **C'est la seule suppression de ligne du module**, et elle ne porte
    /// que sur des pierres tombales dont plus personne n'a besoin.
    pub(crate) fn prune_portscan_tombstones(&self, now: i64) -> DbResult<usize> {
        let floor: Option<i64> = {
            let conn = self.conn().lock().map_err(|_| poisoned())?;
            conn.query_row(
                "SELECT MIN(COALESCE(last_profiles_rev, 0)) FROM probes
                  WHERE revoked_at IS NULL
                    AND archived_at IS NULL
                    AND COALESCE(last_profiles_rev_at, created_at) > ?1",
                [now - TOMBSTONE_GRACE_SECS],
                |r| r.get(0),
            )?
        };
        // Aucune sonde à ménager : tout ce qui est supprimé peut partir. La
        // sonde qui s'enrôlerait ensuite part de zéro et reçoit la liste
        // complète, pierres tombales ou pas.
        let floor = floor.unwrap_or_else(|| self.portscan_rev().unwrap_or(0));

        let removed = {
            let conn = self.conn().lock().map_err(|_| poisoned())?;
            conn.execute(
                "DELETE FROM portscan_profiles WHERE deleted_at IS NOT NULL AND rev <= ?1",
                [floor],
            )?
        };
        if removed > 0 {
            // Le seuil monte avec le nettoyage : une sonde en dessous ne peut
            // plus recevoir de delta honnête, elle recevra la liste complète.
            let known: i64 = self
                .get_setting(PORTSCAN_PURGED_KEY)?
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            if floor + 1 > known {
                self.set_setting(PORTSCAN_PURGED_KEY, &(floor + 1).to_string())?;
            }
        }
        Ok(removed)
    }
}

// ── Routes (contrat § 25) ─────────────────────────────────────────────────

/// Les quatre routes de l'écran d'administration.
///
/// ⚠️ **Aucune garde de portée**, et c'est voulu : la portée d'un profil de
/// scan est le hub ENTIER (décision 4). Contrairement au profil RÉSEAU, qui
/// décrit un site et reste sur la sonde, un profil de scan est une liste de
/// ports — rien de ce qu'il porte n'appartient à un client.
pub(crate) fn routes(state: &AppState) -> Router<AppState> {
    // La liste ne porte aucun secret : ouverte au rôle le plus bas, comme le
    // reste de la consultation.
    let lecture = guarded(
        state,
        Role::Viewer,
        Router::new().route("/api/portscan-profiles", get(list_profiles)),
    );

    // ⚠️ `operator`, comme tout ce qui touche une sonde : un profil décide de
    // ce qui sera frappé sur le réseau d'un client, ce n'est pas une
    // consultation.
    let ecriture = guarded(
        state,
        Role::Operator,
        Router::new()
            .route("/api/portscan-profiles", post(create_profile))
            .route("/api/portscan-profiles/{id}", patch(update_profile))
            .route("/api/portscan-profiles/{id}", delete(delete_profile)),
    );

    lecture.merge(ecriture)
}

fn fail(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({ "error": message }))).into_response()
}

async fn list_profiles(State(state): State<AppState>) -> Response {
    match (state.db.list_portscan_profiles(), state.db.portscan_rev()) {
        // La révision accompagne la liste : l'app mobile et le hub web peuvent
        // ainsi savoir qu'ils regardent la même version, sans la deviner.
        (Ok(profiles), Ok(rev)) => ok_json(serde_json::json!({
            "profiles": profiles,
            "rev": rev,
        })),
        (Err(e), _) | (_, Err(e)) => error_response(e),
    }
}

#[derive(serde::Deserialize)]
struct ProfileBody {
    #[serde(default)]
    name: Option<String>,
    /// ⚠️ `Option`, pas `Vec`. Absent veut dire « ne touche pas aux ports » —
    /// renommer un profil ne doit pas vider sa liste au passage. Un tableau
    /// explicitement vide, lui, dit « la sonde garde la sienne ».
    #[serde(default)]
    ports: Option<Vec<i64>>,
    /// Les ports UDP, mêmes règles que `ports` : absent = inchangé.
    #[serde(default)]
    udp_ports: Option<Vec<i64>>,
}

async fn create_profile(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Json(body): Json<ProfileBody>,
) -> Response {
    let name = body.name.unwrap_or_default();
    match audited(
        &state,
        Some(&actor.username),
        "portscan_profile.create",
        Some(name.trim()),
        state
            .db
            .create_portscan_profile(
                &name,
                &body.ports.unwrap_or_default(),
                &body.udp_ports.unwrap_or_default(),
                None,
            ),
    ) {
        Ok(profile) => (
            StatusCode::CREATED,
            Json(serde_json::to_value(profile).unwrap_or(serde_json::Value::Null)),
        )
            .into_response(),
        Err(e) => error_response(e),
    }
}

async fn update_profile(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(id): Path<String>,
    Json(body): Json<ProfileBody>,
) -> Response {
    if body.name.is_none() && body.ports.is_none() && body.udp_ports.is_none() {
        // Rien à écrire : accepter ferait avancer la révision pour un
        // changement qui n'existe pas, donc voyager un delta vide dans tout le
        // parc.
        return fail(StatusCode::BAD_REQUEST, "rien à modifier");
    }
    match audited(
        &state,
        Some(&actor.username),
        "portscan_profile.update",
        Some(&id),
        state
            .db
            .update_portscan_profile(
                &id,
                body.name.as_deref(),
                body.ports.as_deref(),
                body.udp_ports.as_deref(),
            ),
    ) {
        Ok(profile) => ok_json(serde_json::to_value(profile).unwrap_or(serde_json::Value::Null)),
        Err(e) => error_response(e),
    }
}

/// Pose `deleted_at`. **Ne supprime aucune ligne** — c'est elle qui dira aux
/// sondes de retirer le profil, et sans elle « supprimé » serait indiscernable
/// de « jamais connu ».
async fn delete_profile(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(id): Path<String>,
) -> Response {
    match audited(
        &state,
        Some(&actor.username),
        "portscan_profile.delete",
        Some(&id),
        state.db.delete_portscan_profile(&id),
    ) {
        Ok(()) => ok_json(serde_json::json!({ "ok": true })),
        Err(e) => error_response(e),
    }
}

/// Identifiant stable, généré par qui crée. Le même alphabet que les autres
/// identifiants du hub n'est pas requis : celui-ci n'est jamais dicté.
fn new_profile_id() -> String {
    let mut bytes = [0u8; 8];
    getrandom::getrandom(&mut bytes).expect("getrandom failed");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::now;

    pub(super) fn open_memory() -> Db {
        Db::open_in_memory().unwrap()
    }

    fn db_with_probe() -> (Db, String) {
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();
        let (probe, _) = db.enroll_probe(&site.site_id, "Paris").unwrap();
        (db, probe.probe_id)
    }

    /// Deux sondes dans le même site : le minimum du parc n'a de sens qu'à
    /// plusieurs.
    fn db_with_two_probes() -> (Db, String, String) {
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();
        let (a, _) = db.enroll_probe(&site.site_id, "Paris").unwrap();
        let (b, _) = db.enroll_probe(&site.site_id, "Lyon").unwrap();
        (db, a.probe_id, b.probe_id)
    }

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lanprobe-portscan-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    pub(super) fn profile<'a>(list: &'a [PortscanProfile], id: &str) -> &'a PortscanProfile {
        list.iter()
            .find(|p| p.profile_id == id)
            .unwrap_or_else(|| panic!("profil {id} absent de {list:?}"))
    }

    #[test]
    fn les_profils_de_base_sont_exactement_ceux_de_l_application_sonde() {
        // 🔴 Demande de Benjamin (02/10) : « de base les profils en local et
        // sur le hub doivent être les mêmes ». Le hub semait quatre listes
        // maigres recopiées de son ancienne interface (`Common` vide, `Web` 8
        // ports, `Infra` 9, `Databases` 8) ; l'application sonde en propose
        // cinq, bien plus fournies. Deux vérités pour un seul nom.
        //
        // ⚠️ Les listes sont copiées de `src/lib/stores/portscanProfiles.ts`,
        // `BUILTIN_PROFILES`. Les deux ne peuvent pas partager de source — un
        // module TypeScript du bureau et une constante Rust du hub — et ce test
        // est donc le seul endroit qui dit à quoi elles doivent ressembler.
        let db = open_memory();
        let list = db.list_portscan_profiles().unwrap();

        let common = profile(&list, "common");
        assert_eq!(
            common.ports,
            vec![21, 22, 23, 25, 53, 80, 110, 143, 443, 445, 3306, 3389, 5432, 5900, 8080, 8443]
        );
        assert_eq!(common.udp_ports, vec![53, 123, 137, 161, 1900, 5353]);

        let web = profile(&list, "web");
        assert_eq!(
            web.ports,
            vec![80, 443, 3000, 5000, 8000, 8008, 8080, 8081, 8088, 8181, 8443, 8888, 9000]
        );
        assert!(web.udp_ports.is_empty());

        let db_profile = profile(&list, "db");
        assert_eq!(
            db_profile.ports,
            vec![1433, 1521, 3306, 5432, 5984, 6379, 7000, 9042, 9200, 9300, 11211, 27017, 50000]
        );
        assert_eq!(db_profile.udp_ports, vec![1434]);

        let remote = profile(&list, "remote");
        assert_eq!(remote.name, "Remote access");
        assert_eq!(remote.ports, vec![22, 23, 2222, 3389, 5900, 5901, 5902, 5938, 6000]);

        let full = profile(&list, "full");
        assert_eq!(full.name, "Full (extended)");
        assert_eq!(full.ports.len(), 59, "{:?}", full.ports);
        assert_eq!(full.udp_ports.len(), 12);

        // Tous posés par le hub, aucun venu d'une sonde.
        assert!(list.iter().all(|p| p.origin_probe.is_none()));
    }

    #[test]
    fn la_mise_a_niveau_ne_touche_que_les_profils_restes_tels_qu_ils_ont_ete_semes() {
        // ⚠️ Le hub de Benjamin est DÉJÀ semé avec les anciennes listes, et il
        // y a ajouté un profil à lui. Mettre à niveau en aveugle écraserait son
        // travail ; ne rien faire laisserait le hub et la sonde en désaccord.
        //
        // Le critère est une **comparaison avec la valeur d'origine** : une
        // ligne encore identique à ce que le semis de la v1 avait posé n'a été
        // touchée par personne. Pas de colonne « modifié par un humain » à
        // tenir à jour, et aucune supposition.
        let db = open_memory();
        db.pose_legacy_seed_for_tests();

        // Quelqu'un a retouché « Web », créé « Test », et laissé le reste.
        db.update_portscan_profile("web", None, Some(&[8080]), None).unwrap();
        let test = db.create_portscan_profile("Test", &[9999], &[], None).unwrap();

        db.refresh_seeded_portscan_profiles().unwrap();
        let list = db.list_portscan_profiles().unwrap();

        // Resté tel quel → mis à niveau.
        assert_eq!(profile(&list, "common").ports.len(), 16, "mis à niveau");
        assert_eq!(profile(&list, "common").udp_ports.len(), 6);
        // Retouché → laissé tranquille. C'est le travail de quelqu'un.
        assert_eq!(profile(&list, "web").ports, vec![8080], "jamais écrasé");
        // Créé par quelqu'un → intouchable, et il ne doit surtout pas
        // disparaître au motif qu'il n'est pas dans la liste de base.
        assert_eq!(profile(&list, &test.profile_id).ports, vec![9999]);
        // ⚠️ `Infra` n'existe PAS dans l'application sonde. On ne le supprime
        // pas pour autant : rien ne se supprime ici sans qu'on le demande, et
        // une pierre tombale le ferait disparaître de tout le parc.
        assert_eq!(profile(&list, "infra").ports.len(), 9, "ni touché ni supprimé");
        // Les deux nouveaux profils de base arrivent.
        assert_eq!(profile(&list, "remote").name, "Remote access");
        assert_eq!(profile(&list, "full").udp_ports.len(), 12);
    }

    #[test]
    fn la_mise_a_niveau_ne_ressuscite_pas_un_profil_de_base_supprime() {
        // 🔴 Même leçon que le semis : reposer un profil ressusciterait celui
        // qu'on vient de supprimer exprès. La pierre tombale doit survivre à la
        // mise à niveau, sinon la suppression se défait au redémarrage suivant.
        let db = open_memory();
        db.pose_legacy_seed_for_tests();
        db.delete_portscan_profile("db").unwrap();

        db.refresh_seeded_portscan_profiles().unwrap();

        let vivants = db.list_portscan_profiles().unwrap();
        assert!(!vivants.iter().any(|p| p.profile_id == "db"), "{vivants:?}");
        let tout = db.portscan_profiles_with_tombstones().unwrap();
        assert!(profile(&tout, "db").deleted_at.is_some(), "la date reste");
    }

    #[test]
    fn la_mise_a_niveau_ne_se_rejoue_pas_au_demarrage_suivant() {
        // Sans quoi un profil de base retouché après la mise à niveau serait
        // réécrit à chaque redémarrage du conteneur.
        let db = open_memory();
        db.pose_legacy_seed_for_tests();
        db.refresh_seeded_portscan_profiles().unwrap();
        db.update_portscan_profile("common", None, Some(&[22]), None).unwrap();

        db.refresh_seeded_portscan_profiles().unwrap();

        assert_eq!(profile(&db.list_portscan_profiles().unwrap(), "common").ports, vec![22]);
    }

    #[test]
    fn le_semis_d_origine_reste_trie_et_dedoublonne() {
        // ⚠️ `infra` répétait `161` dans le code du hub. Posée telle quelle, la
        // ligne ferait annoncer « 10 ports » pour neuf, et deux listes
        // identiques à l'ordre près se liraient comme deux profils différents.
        //
        // On le vérifie sur le semis d'ORIGINE, celui que les hubs déjà en
        // service portent : c'est lui que la mise à niveau doit savoir
        // reconnaître, au port près.
        let db = open_memory();
        db.pose_legacy_seed_for_tests();
        let posed = db.list_portscan_profiles().unwrap();

        let infra = profile(&posed, "infra");
        assert_eq!(infra.ports, vec![22, 23, 53, 123, 161, 389, 636, 3389, 5900]);

        // ⚠️ `common` avait des ports VIDES, et ce n'était pas une liste vide
        // envoyée à la sonde : c'était « la sonde garde la sienne ».
        assert!(profile(&posed, "common").ports.is_empty());
    }

    #[test]
    fn un_profil_porte_ses_ports_udp_tries_et_dedoublonnes() {
        // ⚠️ Le hub ne modélisait QUE le TCP : un profil semé « Common » y
        // valait 16 ports TCP et zéro UDP, alors que le même profil, sur la
        // sonde, en scanne six en UDP. Deux listes pour un seul nom, et
        // l'écart était impossible à rattraper depuis le hub.
        let db = open_memory();
        let cree = db
            .create_portscan_profile("Caméras", &[554, 80, 554], &[5353, 1900, 5353], None)
            .unwrap();
        assert_eq!(cree.ports, vec![80, 554]);
        assert_eq!(cree.udp_ports, vec![1900, 5353], "triés et dédoublonnés aussi");

        let relu = profile(&db.list_portscan_profiles().unwrap(), &cree.profile_id).clone();
        assert_eq!(relu.udp_ports, vec![1900, 5353], "et ils se relisent");
    }

    #[test]
    fn renommer_un_profil_n_efface_pas_ses_ports_udp() {
        // Même règle que pour le TCP : un champ absent de la requête n'est pas
        // modifié. Renommer en effaçant l'UDP détruirait un réglage que rien ne
        // pourrait reconstituer.
        let db = open_memory();
        let cree = db
            .create_portscan_profile("Caméras", &[554], &[5353], None)
            .unwrap();
        let renomme = db
            .update_portscan_profile(&cree.profile_id, Some("Vidéo"), None, None)
            .unwrap();
        assert_eq!(renomme.name, "Vidéo");
        assert_eq!(renomme.ports, vec![554]);
        assert_eq!(renomme.udp_ports, vec![5353]);
    }

    #[test]
    fn la_montee_depuis_une_sonde_emporte_aussi_l_udp() {
        // 🔴 Indispensable depuis que le hub fait autorité sur l'UDP : s'il
        // ingérait un profil en laissant tomber ses ports UDP, il le
        // réécrirait au battement suivant **sans UDP** — et la sonde perdrait
        // un réglage que plus personne n'aurait. Le « Perso » de Benjamin
        // porte deux ports UDP : ils doivent monter avec lui.
        let (db, probe_id) = db_with_probe();
        let montants = profiles_from_probe_config(&serde_json::json!({
            "portscan_profiles": [
                { "id": "perso", "name": "Perso", "tcp_ports": [8006], "udp_ports": [53, 123] }
            ]
        }));
        assert_eq!(montants[0].udp_ports, vec![53, 123]);

        db.ingest_probe_profiles(&probe_id, &montants).unwrap();
        let perso = profile(&db.list_portscan_profiles().unwrap(), "perso").clone();
        assert_eq!(perso.udp_ports, vec![53, 123]);
    }

    #[test]
    fn un_profil_supprime_ne_revient_pas_au_demarrage_suivant() {
        // 🔴 Reposer les quatre profils à chaque démarrage ressusciterait celui
        // qu'on vient de supprimer exprès — une suppression qui se défait toute
        // seule au prochain redémarrage du conteneur.
        let dir = tmp_dir("seed");
        let path = dir.join("hub.db");
        {
            let db = Db::open(&path).unwrap();
            db.delete_portscan_profile("web").unwrap();
        }
        let db = Db::open(&path).unwrap();
        let ids: Vec<String> = db
            .list_portscan_profiles()
            .unwrap()
            .into_iter()
            .map(|p| p.profile_id)
            .collect();
        assert!(!ids.contains(&"web".to_string()), "{ids:?}");
        assert_eq!(ids.len(), SEEDED_PROFILES.len() - 1);
    }

    #[test]
    fn supprimer_un_profil_pose_une_date_et_ne_retire_aucune_ligne() {
        // Même règle que `removed_at` au § 20 : la suppression est un FAIT
        // daté. Sans la ligne, la sonde ne distingue pas « le hub ne connaît
        // pas ce profil » de « le hub l'a supprimé », et le recrée au battement
        // suivant en le remontant.
        let db = open_memory();
        db.delete_portscan_profile("db").unwrap();

        assert!(
            !db.list_portscan_profiles()
                .unwrap()
                .iter()
                .any(|p| p.profile_id == "db"),
            "la liste de l'interface exclut les supprimés"
        );
        let all = db.portscan_profiles_with_tombstones().unwrap();
        assert!(profile(&all, "db").deleted_at.is_some(), "{all:?}");
    }

    #[test]
    fn deux_profils_de_meme_nom_ne_coexistent_pas() {
        let db = open_memory();
        let err = db.create_portscan_profile("Web", &[80], &[], None).unwrap_err();
        assert!(matches!(err, DbError::Conflict(_)), "{err:?}");

        // ⚠️ Un nom libéré par une suppression se réemploie : refuser ici
        // interdirait de recréer « Caméras » après l'avoir retiré, sans jamais
        // dire pourquoi.
        db.delete_portscan_profile("web").unwrap();
        db.create_portscan_profile("Web", &[80], &[], None).unwrap();
    }

    #[test]
    fn chaque_changement_fait_avancer_la_revision() {
        let db = open_memory();
        let start = db.portscan_rev().unwrap();

        let created = db.create_portscan_profile("Caméras", &[554], &[], None).unwrap();
        assert!(created.rev > start, "une création avance le compteur");

        let renamed = db
            .update_portscan_profile(&created.profile_id, Some("Caméras IP"), None, None)
            .unwrap();
        assert!(renamed.rev > created.rev, "une modification aussi");
        assert_eq!(renamed.ports, vec![554], "renommer ne touche pas aux ports");

        db.delete_portscan_profile(&created.profile_id).unwrap();
        assert!(
            db.portscan_rev().unwrap() > renamed.rev,
            "et une suppression — sinon la sonde ne l'apprendrait jamais"
        );
    }

    #[test]
    fn une_sonde_a_jour_n_apprend_rien() {
        // Le cas courant, et il ne doit rien coûter : pas de champ dans la
        // réponse, donc pas 40 pierres tombales à chaque battement.
        let db = open_memory();
        let rev = db.portscan_rev().unwrap();
        assert!(db.portscan_profiles_since(rev).unwrap().is_none());
    }

    #[test]
    fn une_sonde_en_retard_recoit_le_delta_pierres_tombales_comprises() {
        let db = open_memory();
        let rev = db.portscan_rev().unwrap();
        let created = db.create_portscan_profile("Caméras", &[554], &[], None).unwrap();
        db.delete_portscan_profile("db").unwrap();

        let update = db.portscan_profiles_since(rev).unwrap().unwrap();
        assert!(!update.replace, "un delta ne remplace pas la liste");
        assert_eq!(update.rev, db.portscan_rev().unwrap());
        let ids: Vec<&str> = update
            .profiles
            .iter()
            .map(|p| p.profile_id.as_str())
            .collect();
        assert_eq!(ids.len(), 2, "seules les lignes plus récentes : {ids:?}");
        assert!(ids.contains(&created.profile_id.as_str()));
        // 🔴 La pierre tombale EN FAIT PARTIE : c'est le cœur du mécanisme.
        assert!(profile(&update.profiles, "db").deleted_at.is_some());
    }

    #[test]
    fn une_sonde_en_avance_recoit_la_liste_complete() {
        // 🔴 Le cas « hub restauré » : la base revient à 38, la sonde a gardé
        // 42. Sans cette garde le hub lui répondrait « tu es à jour » pour
        // toujours, et sa liste de profils gèlerait sans que rien ne le dise.
        let db = open_memory();
        let update = db
            .portscan_profiles_since(db.portscan_rev().unwrap() + 4)
            .unwrap()
            .unwrap();
        assert!(update.replace, "elle doit tout remplacer");
        assert_eq!(update.profiles.len(), SEEDED_PROFILES.len(), "la liste complète");
    }

    #[test]
    fn le_compteur_est_releve_a_la_plus_grande_revision_au_demarrage() {
        // 🔴 Une restauration partielle — la table rendue, le compteur perdu —
        // referait servir des révisions déjà émises : deux changements
        // différents sous le même numéro, et une sonde qui en ignore un.
        let dir = tmp_dir("recul");
        let path = dir.join("hub.db");
        let high = {
            let db = Db::open(&path).unwrap();
            let p = db.create_portscan_profile("Caméras", &[554], &[], None).unwrap();
            // Ce que fait une sauvegarde restaurée : le compteur recule sous
            // ses propres lignes.
            db.set_setting(PORTSCAN_REV_KEY, "0").unwrap();
            p.rev
        };
        let db = Db::open(&path).unwrap();
        assert_eq!(db.portscan_rev().unwrap(), high, "relevé au démarrage");
    }

    #[test]
    fn une_pierre_tombale_ne_part_qu_apres_la_plus_petite_revision_du_parc() {
        // ⚠️ Une pierre tombale qui vit pour toujours est une fuite lente ;
        // une pierre tombale partie trop tôt est une suppression que la sonde
        // en retard n'apprendra jamais — et qu'elle recréera en la remontant.
        let (db, en_avance, en_retard) = db_with_two_probes();
        db.delete_portscan_profile("db").unwrap();
        let rev = db.portscan_rev().unwrap();

        db.note_portscan_rev(&en_avance, rev, now()).unwrap();
        db.note_portscan_rev(&en_retard, rev - 1, now()).unwrap();
        assert_eq!(
            db.prune_portscan_tombstones(now()).unwrap(),
            0,
            "une sonde du parc ne l'a pas encore apprise"
        );

        db.note_portscan_rev(&en_retard, rev, now()).unwrap();
        assert_eq!(db.prune_portscan_tombstones(now()).unwrap(), 1);
        assert!(
            !db.portscan_profiles_with_tombstones()
                .unwrap()
                .iter()
                .any(|p| p.profile_id == "db"),
            "plus personne n'avait besoin de l'apprendre"
        );
    }

    #[test]
    fn une_sonde_revoquee_ou_archivee_ne_retient_plus_le_nettoyage() {
        // ⚠️ Une machine partie à la benne bloquerait le nettoyage pour
        // toujours : elle n'accusera jamais rien.
        let (db, vivante, benne) = db_with_two_probes();
        db.delete_portscan_profile("db").unwrap();
        db.note_portscan_rev(&vivante, db.portscan_rev().unwrap(), now())
            .unwrap();

        db.set_probe_archived(&benne, true).unwrap();
        assert_eq!(db.prune_portscan_tombstones(now()).unwrap(), 1);

        db.delete_portscan_profile("web").unwrap();
        db.note_portscan_rev(&vivante, db.portscan_rev().unwrap(), now())
            .unwrap();
        db.set_probe_archived(&benne, false).unwrap();
        db.revoke_probe(&benne).unwrap();
        assert_eq!(db.prune_portscan_tombstones(now()).unwrap(), 1);
    }

    #[test]
    fn une_sonde_eteinte_retient_le_nettoyage_jusqu_a_quatre_vingt_dix_jours() {
        // ⚠️ Une sonde simplement éteinte COMPTE : le jour où elle revient, elle
        // doit apprendre les suppressions. Mais pas pour l'éternité — d'où le
        // garde-fou, et la liste complète pour celle qui revient après.
        let (db, vivante, eteinte) = db_with_two_probes();
        db.delete_portscan_profile("db").unwrap();
        let rev = db.portscan_rev().unwrap();
        db.note_portscan_rev(&vivante, rev, now()).unwrap();
        db.note_portscan_rev(&eteinte, rev - 1, now()).unwrap();

        assert_eq!(
            db.prune_portscan_tombstones(now() + 89 * 86_400).unwrap(),
            0,
            "89 jours : elle compte encore"
        );
        assert_eq!(
            db.prune_portscan_tombstones(now() + 91 * 86_400).unwrap(),
            1
        );

        // Et celle qui revient ne reçoit pas un delta amputé de la pierre
        // tombale qu'on vient d'effacer : elle reçoit tout.
        let update = db.portscan_profiles_since(rev - 1).unwrap().unwrap();
        assert!(update.replace, "{update:?}");
    }

    #[test]
    fn les_profils_de_base_de_la_sonde_ne_montent_pas() {
        // Décision 3 : une sonde garde ses profils de base, seuls ceux qu'elle
        // crée en plus montent. Les faire monter poserait « Common », « Web »…
        // dans la liste commune de tout le parc, en doublon de ceux du hub —
        // et chaque sonde en ajouterait sa version.
        let montants = profiles_from_probe_config(&serde_json::json!({
            "portscan_profiles": [
                { "id": "builtin:common", "name": "Common", "tcp_ports": [22] },
                { "id": "local-1", "name": "Caméras", "tcp_ports": [554], "udp_ports": [5353] },
                { "id": "local-2", "name": "Vieux", "tcp_ports": [23], "builtin": true }
            ]
        }));
        let ids: Vec<&str> = montants.iter().map(|p| p.profile_id.as_str()).collect();
        assert_eq!(ids, vec!["local-1"], "{montants:?}");
        // ⚠️ L'UDP ne monte pas : le hub ne le modélise pas. C'est la raison
        // pour laquelle la sonde conserve le sien quand le hub lui réécrit un
        // profil, et pas un oubli.
        assert_eq!(montants[0].ports, vec![554]);
    }

    #[test]
    fn une_configuration_sans_profils_ne_fait_rien_monter() {
        assert!(profiles_from_probe_config(&serde_json::json!({ "profiles": [] })).is_empty());
    }

    #[test]
    fn le_hub_ignore_ce_qu_il_connait_et_ce_qu_il_a_supprime_mais_ingere_l_inconnu() {
        // ⚠️ Le hub fait autorité : un profil qu'il connaît ne se laisse pas
        // réécrire par la sonde, et un profil qu'il a supprimé ne se laisse pas
        // ressusciter par la première sonde qui n'a pas encore battu.
        let (db, probe_id) = db_with_probe();
        db.delete_portscan_profile("db").unwrap();

        let ingested = db
            .ingest_probe_profiles(
                &probe_id,
                &[
                    IncomingProfile {
                        profile_id: "web".into(),
                        name: "Web renommé par la sonde".into(),
                        ports: vec![1234],
                        udp_ports: Vec::new(),
                    },
                    IncomingProfile {
                        profile_id: "db".into(),
                        name: "Bases".into(),
                        ports: vec![3306],
                        udp_ports: Vec::new(),
                    },
                    IncomingProfile {
                        profile_id: "cams".into(),
                        name: "Caméras".into(),
                        ports: vec![554, 80, 554],
                        udp_ports: Vec::new(),
                    },
                ],
            )
            .unwrap();
        assert_eq!(ingested, 1, "seul l'inconnu entre");

        let list = db.list_portscan_profiles().unwrap();
        assert_eq!(profile(&list, "web").name, "Web", "le hub garde le sien");
        assert!(!list.iter().any(|p| p.profile_id == "db"), "{list:?}");

        let cams = profile(&list, "cams");
        assert_eq!(cams.ports, vec![80, 554], "triés et dédoublonnés");
        // `origin_probe` est une TRACE, pas un droit : elle répond « d'où sort
        // celui-là », elle ne donne aucune autorité à la sonde.
        assert_eq!(cams.origin_probe.as_deref(), Some(probe_id.as_str()));
    }
}

#[cfg(test)]
mod routes_tests {
    use super::tests::{open_memory, profile};
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use serde_json::json;
    use tower::ServiceExt;

    const MOT_DE_PASSE: &str = "mot-de-passe-de-test";

    struct Harness {
        state: AppState,
        router: axum::Router,
    }

    impl Harness {
        /// Un hub installé, **sans Influx** : les profils n'en touchent aucun.
        fn new() -> Self {
            use std::sync::Arc;
            let db = Arc::new(open_memory());
            db.create_initial_admin("admin", MOT_DE_PASSE).unwrap();
            let settings = crate::settings::Settings::new(db.clone());
            let secrets = crate::secrets::Secrets::ephemeral(db.clone()).unwrap();
            let state = AppState {
                auth: Arc::new(crate::auth::Auth::new(db.clone())),
                influx: Arc::new(crate::influx::Influx::new(
                    settings.clone(),
                    "jeton-operateur-de-test".into(),
                )),
                notifier: crate::notify::Notifier::new(db.clone(), secrets.clone(), settings.clone()),
                ceremonies: Arc::new(crate::passkeys::Ceremonies::new()),
                db,
                settings,
                secrets,
                tls: false,
                cert_fingerprint: None,
                restart_required: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                config_dir: std::env::temp_dir().join("lanprobe-portscan-tests"),
                backup_dir: std::env::temp_dir().join("lanprobe-portscan-tests"),
                influx_cli: "influx-absent-des-tests".into(),
            };
            let router = crate::web::build_router(state.clone());
            Self { state, router }
        }

        async fn call(&self, req: Request<Body>) -> (StatusCode, serde_json::Value) {
            let response = self.router.clone().oneshot(req).await.unwrap();
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
            let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
            (status, body)
        }

        /// Un compte d'un rôle donné, et son cookie de session.
        fn session(&self, username: &str, role: crate::db::Role) -> String {
            if username != "admin" {
                self.state
                    .db
                    .create_user(username, MOT_DE_PASSE, role)
                    .unwrap();
            }
            let token = self.state.auth.start_session(username.to_string()).unwrap();
            format!("lanprobe_hub_session={token}")
        }

        async fn send(
            &self,
            method: &str,
            path: &str,
            cookie: &str,
            body: serde_json::Value,
        ) -> (StatusCode, serde_json::Value) {
            self.call(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::COOKIE, cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
        }

        /// Une sonde enrôlée, avec son jeton en clair.
        fn probe(&self) -> (String, String) {
            let site = self.state.db.create_site("Durand").unwrap();
            let (probe, token) = self.state.db.enroll_probe(&site.site_id, "Paris").unwrap();
            (probe.probe_id, token)
        }

        async fn heartbeat(&self, id: &str, token: &str, body: serde_json::Value) -> serde_json::Value {
            let mut req = Request::builder()
                .method("POST")
                .uri(format!("/api/probes/{id}/heartbeat"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap();
            req.extensions_mut().insert(axum::extract::ConnectInfo(
                "203.0.113.10:41000".parse::<std::net::SocketAddr>().unwrap(),
            ));
            self.call(req).await.1
        }
    }

    #[tokio::test]
    async fn la_liste_se_lit_en_viewer_et_ne_s_ecrit_qu_en_operateur() {
        // La portée est le hub ENTIER (décision 4) : un profil de scan n'est
        // pas privé, contrairement au profil réseau qui décrit un site.
        let h = Harness::new();
        let lecteur = h.session("lea", crate::db::Role::Viewer);

        let (status, body) = h.send("GET", "/api/portscan-profiles", &lecteur, json!({})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["profiles"].as_array().unwrap().len(), SEEDED_PROFILES.len());

        // Lancer un scan change ce qui frappe le réseau d'un client : ce n'est
        // pas une consultation.
        let (status, _) = h
            .send(
                "POST",
                "/api/portscan-profiles",
                &lecteur,
                json!({ "name": "Caméras", "ports": [554] }),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let operateur = h.session("olivier", crate::db::Role::Operator);
        let (status, created) = h
            .send(
                "POST",
                "/api/portscan-profiles",
                &operateur,
                json!({ "name": "Caméras", "ports": [554, 80, 554] }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        assert_eq!(created["ports"], json!([80, 554]), "triés et dédoublonnés");
    }

    #[tokio::test]
    async fn supprimer_par_l_api_pose_une_date_sans_retirer_la_ligne() {
        let h = Harness::new();
        let operateur = h.session("olivier", crate::db::Role::Operator);

        let (status, _) = h
            .send("DELETE", "/api/portscan-profiles/db", &operateur, json!({}))
            .await;
        assert_eq!(status, StatusCode::OK);

        let (_, body) = h
            .send("GET", "/api/portscan-profiles", &operateur, json!({}))
            .await;
        let ids: Vec<&str> = body["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["profile_id"].as_str().unwrap())
            .collect();
        assert!(!ids.contains(&"db"), "{ids:?}");

        // 🔴 La ligne reste : c'est elle qui dira aux sondes de retirer le
        // profil. La supprimer rendrait « supprimé » indiscernable de « jamais
        // connu », et la première sonde le recréerait en le remontant.
        let all = h.state.db.portscan_profiles_with_tombstones().unwrap();
        assert!(profile(&all, "db").deleted_at.is_some());
    }

    #[tokio::test]
    async fn le_battement_d_une_sonde_a_jour_ne_porte_aucun_champ_de_profils() {
        // ⚠️ Et c'est tout l'intérêt du compteur : le cas courant ne fait
        // voyager NI la liste, NI les pierres tombales.
        let h = Harness::new();
        let (id, token) = h.probe();
        let rev = h.state.db.portscan_rev().unwrap();

        let response = h.heartbeat(&id, &token, json!({ "profiles_rev": rev })).await;
        assert!(
            response.get("portscan_profiles").is_none(),
            "à jour : rien à dire — {response}"
        );
    }

    #[tokio::test]
    async fn un_hub_ne_parle_pas_de_profils_a_une_sonde_qui_les_ignore() {
        // ⚠️ Une sonde antérieure au mécanisme n'annonce aucune révision : elle
        // ne saurait pas quoi faire de la liste, et surtout pas quelle révision
        // ranger. Lui en envoyer une ne ferait que gonfler chaque battement.
        let h = Harness::new();
        let (id, token) = h.probe();

        let response = h.heartbeat(&id, &token, json!({})).await;
        assert!(response.get("portscan_profiles").is_none(), "{response}");
    }

    #[tokio::test]
    async fn une_sonde_en_retard_recoit_le_delta_et_la_revision_a_ranger() {
        let h = Harness::new();
        let (id, token) = h.probe();
        let depart = h.state.db.portscan_rev().unwrap();
        h.state.db.delete_portscan_profile("db").unwrap();

        let response = h.heartbeat(&id, &token, json!({ "profiles_rev": depart })).await;
        let liste = response["portscan_profiles"].as_array().unwrap();
        assert_eq!(liste.len(), 1, "{response}");
        assert_eq!(liste[0]["profile_id"], "db");
        assert!(liste[0]["deleted_at"].is_i64(), "la pierre tombale voyage");
        assert_eq!(
            response["portscan_profiles_rev"],
            json!(h.state.db.portscan_rev().unwrap()),
            "la sonde doit savoir quoi ranger"
        );
        assert_eq!(response["portscan_profiles_replace"], json!(false));

        // L'annonce de la révision EST l'accusé : le hub la range.
        let suivant = h.heartbeat(&id, &token, json!({
            "profiles_rev": h.state.db.portscan_rev().unwrap()
        }))
        .await;
        assert!(suivant.get("portscan_profiles").is_none(), "{suivant}");
    }

    #[tokio::test]
    async fn une_sonde_en_avance_recoit_la_liste_complete_avec_le_drapeau() {
        // 🔴 Le cas « hub restauré », et il se répare tout seul au premier
        // battement. Sans lui, le hub répondrait « tu es à jour » pour toujours
        // à une sonde dont la liste ne bougerait plus jamais.
        let h = Harness::new();
        let (id, token) = h.probe();
        let avance = h.state.db.portscan_rev().unwrap() + 7;

        let response = h.heartbeat(&id, &token, json!({ "profiles_rev": avance })).await;
        assert_eq!(response["portscan_profiles_replace"], json!(true), "{response}");
        assert_eq!(
            response["portscan_profiles"].as_array().unwrap().len(),
            SEEDED_PROFILES.len()
        );
    }

    #[tokio::test]
    async fn la_configuration_remontee_fait_monter_les_profils_inconnus() {
        // Décision 3 : une sonde garde ses profils de base, et ceux qu'elle
        // crée en plus montent et rejoignent la liste commune.
        let h = Harness::new();
        let (id, token) = h.probe();

        let (status, _) = h
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/probes/{id}/config"))
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "portscan_profiles": [
                                { "id": "cams", "name": "Caméras", "tcp_ports": [554, 80], "udp_ports": [5353] }
                            ]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await;
        assert_eq!(status, StatusCode::OK);

        let list = h.state.db.list_portscan_profiles().unwrap();
        let cams = profile(&list, "cams");
        assert_eq!(cams.ports, vec![80, 554]);
        assert_eq!(cams.origin_probe.as_deref(), Some(id.as_str()));
    }

    #[tokio::test]
    async fn le_battement_fait_monter_un_profil_inconnu_avec_son_origine() {
        // 🔴 LE défaut constaté en production le 02/10 : la sonde « Macos »
        // avait un profil local « Perso » que le hub n'a jamais connu. La
        // montée n'était branchée QUE sur le dépôt de configuration
        // (`POST /api/probes/{id}/config`), que seule l'interface du bureau
        // déclenche, et seulement quand on touche à un profil. Rien ne la
        // rejoue : un profil créé avant l'enrôlement — ou avant que le hub
        // sache l'ingérer — ne remontait jamais.
        //
        // Le battement revient toutes les soixante secondes : c'est là que la
        // montée doit vivre, et c'est ce que ce test joue de bout en bout.
        let h = Harness::new();
        let (id, token) = h.probe();

        let reponse = h
            .heartbeat(
                &id,
                &token,
                json!({
                    "profiles_rev": 0,
                    "portscan_profiles": [
                        { "id": "perso", "name": "Perso", "tcp_ports": [8006, 22], "udp_ports": [53, 123] }
                    ]
                }),
            )
            .await;

        let list = h.state.db.list_portscan_profiles().unwrap();
        let perso = profile(&list, "perso");
        assert_eq!(perso.ports, vec![22, 8006]);
        // `origin_probe` répond « d'où sort celui-là » — c'est une trace, pas
        // un droit.
        assert_eq!(perso.origin_probe.as_deref(), Some(id.as_str()));

        // ⚠️ Et il redescend dans le MÊME battement, marqué comme venant du
        // hub : l'ingestion passe AVANT le calcul du delta. Sinon la sonde
        // garderait une minute de plus un profil que le hub connaît déjà, et
        // l'écran en montrerait deux.
        let descendus = reponse["portscan_profiles"].as_array().unwrap();
        assert!(
            descendus.iter().any(|p| p["profile_id"] == "perso"),
            "{reponse}"
        );
    }

    #[tokio::test]
    async fn un_battement_sans_profils_ne_fait_rien_monter() {
        // ⚠️ Pas de champ ≠ liste vide, dans ce sens-là aussi : une sonde
        // antérieure à la montée au battement ne doit rien changer au hub.
        let h = Harness::new();
        let (id, token) = h.probe();
        let avant = h.state.db.portscan_rev().unwrap();

        h.heartbeat(&id, &token, json!({ "profiles_rev": avant })).await;

        assert_eq!(h.state.db.portscan_rev().unwrap(), avant);
    }

    #[tokio::test]
    async fn le_parc_qui_rattrape_sa_revision_laisse_partir_les_pierres_tombales() {
        // ⚠️ Nettoyer au battement et nulle part ailleurs : c'est le seul
        // moment où la plus petite révision du parc peut avoir bougé, et il n'y
        // a aucune tâche de fond à rater.
        let h = Harness::new();
        let (id, token) = h.probe();
        h.state.db.delete_portscan_profile("db").unwrap();
        let rev = h.state.db.portscan_rev().unwrap();

        h.heartbeat(&id, &token, json!({ "profiles_rev": rev })).await;

        assert!(
            !h.state
                .db
                .portscan_profiles_with_tombstones()
                .unwrap()
                .iter()
                .any(|p| p.profile_id == "db"),
            "toute le parc l'a apprise : elle n'a plus rien à dire"
        );
    }
}
