//! Les libellés du classeur SLA, dans la langue de l'opérateur (contrat § 23).
//!
//! ⚠️ **Souche.** Signatures réelles, corps à écrire.
//!
//! 🔴 **La langue est celle de l'OPÉRATEUR, pas celle du client.** Tranché par
//! Benjamin. Le parcours est *télécharger → ouvrir → vérifier → envoyer* : on
//! ne peut relire que ce qu'on lit, et un classeur produit dans une langue que
//! le demandeur ne parle pas ne serait vérifié par personne. Le commentaire de
//! `web-ui/src/lib/sla-report.ts` disait l'inverse alors que le code y passait
//! déjà la locale de l'interface ; il a été corrigé.
//!
//! ⚠️ **La locale est celle de la DEMANDE**, figée dans `reports.locale` : la
//! relire au moment de produire donnerait un document dans la langue de qui
//! regarde l'écran cinq minutes plus tard.
//!
//! ⚠️ **Les clés sont les mêmes que celles du navigateur** — le préfixe `sla.`
//! et `report.` de `web-ui/src/lib/i18n/*.json`. C'est ce qui garantit que le
//! classeur du hub et celui du hub web portent les mêmes intitulés : deux
//! catalogues finiraient par diverger d'un mot, et c'est le document remis au
//! client qui porterait le mauvais.

// Une souche n'a pas encore d'appelant.
#![allow(dead_code)]

/// Les trois langues du produit — les mêmes que celles de l'app et du hub web.
/// ⚠️ Une sonde et le hub qui la pilote doivent parler la même.
pub(crate) const LOCALES: [&str; 3] = ["fr", "en", "es"];

/// Langue de repli quand celle qui est demandée n'existe pas.
///
/// ⚠️ Un repli, **jamais** la clé brute. `sla.summary` en tête de colonne d'un
/// document remis à un client est le genre de défaut qu'on ne découvre que
/// devant lui : c'est arrivé côté navigateur, où une clé manquante s'affiche
/// telle quelle.
pub(crate) const FALLBACK_LOCALE: &str = "fr";

/// Le catalogue d'une langue, prêt à traduire.
pub(crate) struct Catalog {
    /// La langue réellement retenue — pas forcément celle demandée.
    pub locale: String,
}

impl Catalog {
    /// Charge le catalogue d'une locale, avec repli sur [`FALLBACK_LOCALE`].
    pub(crate) fn load(_locale: &str) -> Self {
        todo!("tâche « rapports » : catalogues embarqués, mêmes clés que web-ui")
    }

    /// Traduit une clé. `args` porte les substitutions (`{n}`, `{ip}`…).
    ///
    /// ⚠️ Une clé inconnue ne rend **pas** la clé : elle rend le libellé de
    /// repli. Voir [`FALLBACK_LOCALE`].
    pub(crate) fn t(&self, _key: &str, _args: &[(&str, String)]) -> String {
        todo!("tâche « rapports »")
    }

    /// Formate une date dans la langue du catalogue.
    ///
    /// ⚠️ Le fuseau est celui du **hub**, et il est écrit en toutes lettres
    /// dans le classeur. Une heure sans fuseau, dans un document qui liste des
    /// coupures datées, se conteste au premier décalage horaire.
    pub(crate) fn date(&self, _epoch_secs: i64) -> String {
        todo!("tâche « rapports »")
    }

    /// Formate une durée (« 3 min 12 s »), comme `duration()` côté navigateur.
    pub(crate) fn duration(&self, _secs: i64) -> String {
        todo!("tâche « rapports »")
    }

    /// Formate un pourcentage.
    ///
    /// 🔴 **`None` n'est pas `0`.** Sans un seul relevé déterminé, il n'y a pas
    /// de pourcentage : rendre `0` affirmerait une panne totale, sur le
    /// document qui part chez le client. Le repli est le mot « indéterminé »,
    /// jamais un chiffre (contrat § 3).
    pub(crate) fn percent(&self, _value: Option<f64>) -> String {
        todo!("tâche « rapports »")
    }
}
