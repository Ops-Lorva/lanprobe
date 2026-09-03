//! Le classeur SLA, produit par le hub (contrat § 23).
//!
//! ⚠️ **Souche.** Signatures réelles, corps à écrire.
//!
//! 🔴 **Exactement le même classeur que celui du hub web.** Pas un équivalent,
//! pas une version mobile allégée : six familles de feuilles — Résumé · une
//! feuille par cible (coupures datées + série) · Débit · Machines · Ports ·
//! Adresses publiques. Le modèle est `web-ui/src/lib/sla-report.ts`, et le
//! **hub web devra basculer sur cette route** : sans cela on ne passe pas de
//! deux générateurs à un, on passe à trois. C'est la moitié la plus importante
//! du chantier, et celle qu'on oublie.
//!
//! ⚠️ **Rien n'est recalculé ici.** Les taux, la couverture et l'historique des
//! adresses arrivent tels que `sla_payload::build` les rend. Le classeur remis
//! au client et le CSV doivent annoncer la même complétude ; deux calculs
//! finiraient par diverger d'un point, et c'est celui du document qu'on ne
//! saurait plus expliquer.
//!
//! ⚠️ Trois partis pris hérités du générateur du navigateur, à ne pas perdre :
//! la fenêtre est écrite **en toutes lettres** sur chaque feuille ; les
//! coupures sont listées **une par une**, avec début, fin et durée ; une
//! coupure **en cours** est marquée comme telle, jamais close sur l'instant de
//! l'export — écrire une heure de fin qui n'a pas eu lieu fabriquerait une
//! durée fausse.
//!
//! ⚠️ **À trancher, et ce n'est pas un détail** : le navigateur dessine les
//! graphiques sur un `<canvas>` et les insère en PNG — délibérément une image,
//! pour qu'un tri de colonne ne les recalcule pas. En Rust il faut soit les
//! redessiner, soit accepter un classeur sans images, **ce qui change le
//! document remis au client**.

// Une souche n'a pas encore d'appelant.
#![allow(dead_code)]

use crate::report_i18n::Catalog;

/// Ce qu'un classeur porte : un site, une fenêtre, et les relevés de chacune
/// de ses sondes.
///
/// ⚠️ **Un site, une fenêtre.** Le générateur lit le site et la période du
/// premier relevé et suppose qu'ils valent pour tous : il ne sait pas produire
/// un classeur à deux sites ni à deux périodes. C'est vrai par construction
/// dans l'interface — on ne peut pas cocher des sondes de sites différents — et
/// c'est écrit dans le contrat de route (§ 23).
pub(crate) struct Workbook {
    pub site_name: String,
    pub range_start: i64,
    pub range_stop: i64,
    /// Un élément par sonde, tel que `sla_payload::build` le rend.
    pub payloads: Vec<serde_json::Value>,
}

/// Ce que la construction produit — le fichier et de quoi le vérifier.
pub(crate) struct BuiltFile {
    pub file_name: String,
    pub bytes: Vec<u8>,
    /// 🔴 Calculé **sur les octets écrits**, jamais sur une valeur intermédiaire.
    /// C'est le seul contrôle qui attrape une corruption silencieuse, et non
    /// seulement une coupure (contrat § 23).
    pub sha256: String,
}

/// Construit le classeur.
///
/// ⚠️ Rend le fichier **en mémoire** et non un flux : la route doit annoncer un
/// `Content-Length` sur une réponse **non chunkée**. Une réponse construite en
/// flux part volontiers en `Transfer-Encoding: chunked`, auquel cas il n'y a
/// aucune taille à comparer et la garantie contre un fichier tronqué tombe en
/// silence. Un XLSX de SLA pèse quelques centaines de kilo-octets.
pub(crate) fn build(_workbook: &Workbook, _catalog: &Catalog) -> Result<BuiltFile, String> {
    todo!("tâche « classeur » : six familles de feuilles, modèle sla-report.ts")
}

/// Nom du fichier remis, dérivé du site et de la fenêtre.
///
/// ⚠️ Il part chez un client : il ne doit porter ni identifiant technique, ni
/// caractère que Windows refuse dans un nom de fichier.
pub(crate) fn file_name(_site_name: &str, _range_start: i64, _range_stop: i64) -> String {
    todo!("tâche « classeur »")
}
