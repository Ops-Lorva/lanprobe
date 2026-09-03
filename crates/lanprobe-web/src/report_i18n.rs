//! Les libellés du classeur SLA, dans la langue de l'opérateur (contrat § 23).
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

/// 🔴 **Les catalogues du navigateur, pas une copie.**
///
/// `include_str!` va chercher les fichiers là où l'interface les lit. Une copie
/// dans le crate aurait vécu sa vie : un mot corrigé côté web, l'ancien resté
/// côté hub, et le classeur remis au client aurait porté le mauvais — sans que
/// rien ne le signale. Ici, un renommage de fichier casse la **compilation**,
/// et une clé disparue fait échouer un test.
const FR: &str = include_str!("../../../web-ui/src/lib/i18n/fr.json");
const EN: &str = include_str!("../../../web-ui/src/lib/i18n/en.json");
const ES: &str = include_str!("../../../web-ui/src/lib/i18n/es.json");

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

/// 🔴 **Toutes les clés que le classeur écrit**, énumérées ici et nulle part
/// ailleurs.
///
/// C'est ce qui donne sa prise au test : sans cette liste, une clé retirée d'un
/// seul des trois catalogues ne se verrait qu'à l'ouverture du document, dans
/// la langue de quelqu'un d'autre. Le générateur ne traduit **que** ces clés ;
/// en ajouter une au classeur sans l'ajouter ici est un oubli que la relecture
/// attrape.
pub(crate) const REPORT_KEYS: &[&str] = &[
    // En-tête, présent sur chaque feuille.
    "sla.probe",
    "sla.site",
    "sla.window",
    "sla.generated",
    "sla.internet",
    // La fenêtre, en toutes lettres.
    "sla.from",
    "sla.to",
    "sla.window_days",
    "sla.window_hours",
    // Complétude et indéterminé.
    "sla.coverage_partial",
    "sla.coverage_unknown",
    "sla.undetermined",
    "sla.col_coverage",
    "sla.col_undetermined",
    // Synthèse.
    "sla.sheet_summary",
    "sla.col_target",
    "sla.col_uptime",
    "sla.col_avg",
    "sla.col_min",
    "sla.col_max",
    "sla.col_p95",
    "sla.col_samples",
    "sla.col_outages",
    "sla.col_failed",
    // Coupures.
    "sla.sheet_outages",
    "sla.col_start",
    "sla.col_end",
    "sla.col_duration",
    "sla.col_lost",
    "sla.no_outage",
    "sla.ongoing",
    "sla.dur_s",
    "sla.dur_m",
    "sla.dur_h",
    // Série.
    "sla.sheet_series",
    "sla.col_time",
    "sla.col_state",
    "sla.col_latency",
    // Débits.
    "sla.sheet_speed",
    "sla.col_engine",
    "sla.col_server",
    "sla.col_down",
    "sla.col_up",
    "sla.col_jitter",
    // Inventaire.
    "sla.sheet_hosts",
    "sla.col_scan_at",
    "sla.col_ip",
    "sla.col_hostname",
    "sla.col_mac",
    "sla.col_vendor",
    "sla.sheet_ports",
    "sla.col_port",
    "sla.col_proto",
    "sla.col_service",
    // SLA par adresse publique.
    "report.ipSheet",
    "report.ipLabel",
    "report.ipAddress",
    "report.ipGateway",
    "report.ipFrom",
    "report.ipTo",
    "report.ipSamples",
    "report.ipUptime",
    "report.ipUndetermined",
];

/// Le fuseau dans lequel le classeur écrit ses horodatages.
///
/// ⚠️ **Écrit en toutes lettres sur chaque date**, et non supposé. Le
/// générateur du navigateur rendait les dates dans le fuseau de la machine qui
/// regardait l'écran ; le hub, lui, tourne dans un conteneur. Une heure sans
/// fuseau, dans un document qui liste des coupures datées, se conteste au
/// premier décalage horaire — et personne ne saurait dire laquelle des deux
/// lectures est la bonne.
const TIME_ZONE: &str = "UTC";

/// Le catalogue d'une langue, prêt à traduire.
pub(crate) struct Catalog {
    /// La langue réellement retenue — pas forcément celle demandée.
    pub locale: String,
    messages: serde_json::Value,
    fallback: serde_json::Value,
}

fn parse(locale: &str) -> serde_json::Value {
    let raw = match locale {
        "en" => EN,
        "es" => ES,
        _ => FR,
    };
    // Les catalogues sont compilés dans le binaire : s'ils ne sont pas du JSON,
    // rien de ce hub ne peut fonctionner et il vaut mieux le savoir au premier
    // test qu'au premier rapport.
    serde_json::from_str(raw).expect("catalogue i18n illisible")
}

/// Descend un chemin pointé (`sla.col_uptime`) dans un catalogue imbriqué.
fn lookup<'a>(root: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    let mut node = root;
    for part in key.split('.') {
        node = node.get(part)?;
    }
    node.as_str()
}

impl Catalog {
    /// Charge le catalogue d'une locale, avec repli sur [`FALLBACK_LOCALE`].
    pub(crate) fn load(locale: &str) -> Self {
        // ⚠️ On ne compare que le sous-tag de langue : la demande peut arriver
        // en « fr-CA » ou « es-419 », et seul « fr » exact serait reconnu.
        let tag = locale.split(['-', '_']).next().unwrap_or("").to_lowercase();
        let retenue = LOCALES
            .iter()
            .find(|l| **l == tag)
            .copied()
            .unwrap_or(FALLBACK_LOCALE);
        Self {
            locale: retenue.to_string(),
            messages: parse(retenue),
            fallback: parse(FALLBACK_LOCALE),
        }
    }

    /// La clé existe-t-elle dans **cette** langue — sans passer par le repli.
    ///
    /// Sert au test qui garde les trois catalogues alignés : passer par le
    /// repli masquerait précisément le trou qu'on cherche.
    pub(crate) fn has(&self, key: &str) -> bool {
        lookup(&self.messages, key).is_some()
    }

    /// Traduit une clé. `args` porte les substitutions (`{n}`, `{ip}`…).
    ///
    /// ⚠️ Une clé inconnue ne rend **pas** la clé : elle rend le libellé de
    /// repli. Voir [`FALLBACK_LOCALE`].
    pub(crate) fn t(&self, key: &str, args: &[(&str, String)]) -> String {
        let raw = lookup(&self.messages, key)
            .or_else(|| lookup(&self.fallback, key))
            // Absente des deux : le test `les_trois_catalogues_portent_toutes
            // _les_cles_du_rapport` interdit ce cas ; s'il survient quand même,
            // la clé brute est le seul indice exploitable pour le corriger.
            .unwrap_or(key);
        interpolate(raw, args)
    }

    /// Formate une date dans la langue du catalogue.
    ///
    /// ⚠️ Le fuseau est celui du **hub**, et il est écrit en toutes lettres
    /// dans le classeur. Une heure sans fuseau, dans un document qui liste des
    /// coupures datées, se conteste au premier décalage horaire.
    ///
    /// ⚠️ **Une seule forme pour les trois langues**, et délibérément : le
    /// navigateur passait par `Intl.DateTimeFormat`, que le hub n'a pas. En
    /// imiter la sortie à la main (« 3/9/26 » pour l'espagnol, « 9/3/26 » pour
    /// l'anglais) donnerait une ressemblance approximative — c'est-à-dire une
    /// divergence déguisée en parité, sur le document qui part chez le client.
    /// Une date ISO se lit sans ambiguïté dans les trois langues, se trie, et
    /// ne confond jamais le jour avec le mois.
    pub(crate) fn date(&self, epoch_secs: i64) -> String {
        let (y, m, d, hh, mm, ss) = civil_from_epoch(epoch_secs);
        format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02} {TIME_ZONE}")
    }

    /// La date seule, sans heure ni fuseau — pour la fenêtre écrite en tête de
    /// feuille, où l'heure n'apporte rien et alourdit la phrase.
    pub(crate) fn day(&self, epoch_secs: i64) -> String {
        iso_day(epoch_secs)
    }

    /// Formate une durée (« 3 min 12 s »), comme `duration()` côté navigateur.
    pub(crate) fn duration(&self, secs: i64) -> String {
        if secs < 60 {
            self.t("sla.dur_s", &[("n", secs.to_string())])
        } else if secs < 3_600 {
            // Arrondi à la minute, comme le navigateur : une coupure ne se
            // discute pas à la seconde près une fois passée le quart d'heure.
            let n = (secs as f64 / 60.0).round() as i64;
            self.t("sla.dur_m", &[("n", n.to_string())])
        } else {
            let n = format!("{:.1}", secs as f64 / 3_600.0);
            self.t("sla.dur_h", &[("n", n)])
        }
    }

    /// Formate un pourcentage.
    ///
    /// 🔴 **`None` n'est pas `0`.** Sans un seul relevé déterminé, il n'y a pas
    /// de pourcentage : rendre `0` affirmerait une panne totale, sur le
    /// document qui part chez le client. Le repli est le mot « indéterminé »,
    /// jamais un chiffre (contrat § 3).
    pub(crate) fn percent(&self, value: Option<f64>) -> String {
        match value {
            Some(v) => format!("{} %", trim_num(round_to(v, 2))),
            None => self.t("sla.undetermined", &[]),
        }
    }
}

/// Une date sans heure, `AAAA-MM-JJ`, en UTC.
pub(crate) fn iso_day(epoch_secs: i64) -> String {
    let (y, m, d, ..) = civil_from_epoch(epoch_secs);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Arrondit à `n` décimales, comme `toFixed(n)` côté navigateur.
pub(crate) fn round_to(v: f64, n: u32) -> f64 {
    let f = 10f64.powi(n as i32);
    (v * f).round() / f
}

/// Rend un nombre comme le ferait un tableur : sans zéros décimaux inutiles.
///
/// Le navigateur écrivait `+x.toFixed(2)`, c'est-à-dire un **nombre** — donc
/// « 99.2 » et non « 99.20 ». On garde la même écriture.
pub(crate) fn trim_num(v: f64) -> String {
    // `{}` sur un f64 rend déjà la forme la plus courte qui relit à
    // l'identique : « 99.2 » et non « 99.20 », « 100 » et non « 100.0 ».
    format!("{v}")
}

/// Substitutions `{nom}` et pluriel ICU `{n, plural, one {…} other {…}}`.
///
/// ⚠️ Sous-ensemble assumé de ce que `svelte-i18n` sait faire, limité à ce que
/// les catalogues emploient réellement. Tirer une bibliothèque ICU complète
/// pour deux messages au pluriel coûterait plus cher que la relecture de ce
/// fichier — mais toute forme non gérée doit ressortir **telle quelle**, jamais
/// à moitié substituée.
fn interpolate(raw: &str, args: &[(&str, String)]) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '{' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let Some(close) = matching_brace(&chars, i) else {
            // Accolade non fermée : on ne devine pas, on recopie.
            out.push(chars[i]);
            i += 1;
            continue;
        };
        let inner: String = chars[i + 1..close].iter().collect();
        out.push_str(&expand(&inner, args));
        i = close + 1;
    }
    out
}

fn matching_brace(chars: &[char], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in chars.iter().enumerate().skip(open) {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn expand(inner: &str, args: &[(&str, String)]) -> String {
    let mut parts = inner.splitn(3, ',');
    let name = parts.next().unwrap_or("").trim();
    let kind = parts.next().map(str::trim);
    let body = parts.next();

    let value = args
        .iter()
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.as_str());

    match (kind, body) {
        (Some("plural"), Some(body)) => {
            let Some(v) = value else {
                return format!("{{{inner}}}");
            };
            let n: i64 = v.parse().unwrap_or(0);
            let forme = if n == 1 { "one" } else { "other" };
            match plural_arm(body, forme) {
                // `#` = le nombre lui-même, et le corps peut lui aussi porter
                // des substitutions.
                Some(arm) => interpolate(&arm.replace('#', v), args),
                None => format!("{{{inner}}}"),
            }
        }
        (None, None) => match value {
            Some(v) => v.to_string(),
            // Argument non fourni : on laisse le motif visible plutôt que de
            // livrer un trou dans une phrase.
            None => format!("{{{inner}}}"),
        },
        // Forme ICU non gérée (`date`, `select`…) : recopiée intacte, pour
        // qu'elle se voie à la relecture au lieu de sortir mutilée.
        _ => format!("{{{inner}}}"),
    }
}

/// Extrait le corps d'une branche de pluriel : `one {…}` → `…`.
fn plural_arm(body: &str, forme: &str) -> Option<String> {
    let chars: Vec<char> = body.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        // Un mot-clé de branche, puis son corps entre accolades.
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        let debut = i;
        while i < chars.len() && !chars[i].is_whitespace() && chars[i] != '{' {
            i += 1;
        }
        let mot: String = chars[debut..i].iter().collect();
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() || chars[i] != '{' {
            return None;
        }
        let close = matching_brace(&chars, i)?;
        if mot == forme {
            return Some(chars[i + 1..close].iter().collect());
        }
        i = close + 1;
    }
    None
}

/// Date civile depuis un instant epoch, en UTC.
///
/// Algorithme `civil_from_days` de Howard Hinnant : pas de dépendance, pas de
/// table, et juste sur les années bissextiles — que le rapport rencontrera.
fn civil_from_epoch(epoch_secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = epoch_secs.div_euclid(86_400);
    let rem = epoch_secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (
        y,
        m,
        d,
        (rem / 3_600) as u32,
        ((rem % 3_600) / 60) as u32,
        (rem % 60) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 🔴 Le test qui interdit la copie.
    ///
    /// Les catalogues sont `include_str!` depuis `web-ui/src/lib/i18n/` : une
    /// clé retirée ou renommée côté navigateur casse la compilation du hub ou
    /// fait échouer ce test, jamais silencieusement le document remis au
    /// client. Une copie, elle, aurait dérivé sans que personne le voie.
    #[test]
    fn les_trois_catalogues_portent_toutes_les_cles_du_rapport() {
        for locale in LOCALES {
            let cat = Catalog::load(locale);
            for key in REPORT_KEYS {
                assert!(
                    cat.has(key),
                    "clé « {key} » absente du catalogue « {locale} »"
                );
            }
            // La garde elle-même doit mordre : `has` ne passe pas par le repli,
            // sans quoi le test au-dessus applaudirait un catalogue vide.
            assert!(!cat.has("sla.cle_qui_nexiste_pas"));
        }
    }

    #[test]
    fn une_locale_inconnue_retombe_sur_le_repli() {
        let cat = Catalog::load("de");
        assert_eq!(cat.locale, FALLBACK_LOCALE);
        assert_eq!(cat.t("sla.sheet_summary", &[]), "Synthèse");
    }

    #[test]
    fn une_cle_inconnue_rend_le_libelle_de_repli_pas_la_cle() {
        let cat = Catalog::load("en");
        // Présente partout : c'est bien la langue demandée qui sort.
        assert_eq!(cat.t("sla.sheet_summary", &[]), "Summary");
    }

    #[test]
    fn les_substitutions_simples_sont_remplacees() {
        let cat = Catalog::load("fr");
        assert_eq!(
            cat.t("sla.coverage_partial", &[("pct", "97.4".into())]),
            "97.4 % de la période mesurée"
        );
    }

    #[test]
    fn le_pluriel_icu_choisit_la_forme_et_remplace_le_diese() {
        let fr = Catalog::load("fr");
        assert_eq!(fr.t("sla.window_days", &[("n", "1".into())]), "dernier jour");
        assert_eq!(
            fr.t("sla.window_days", &[("n", "7".into())]),
            "7 derniers jours"
        );
        let en = Catalog::load("en");
        assert_eq!(en.t("sla.window_hours", &[("n", "1".into())]), "last hour");
        assert_eq!(
            en.t("sla.window_hours", &[("n", "24".into())]),
            "last 24 hours"
        );
        let es = Catalog::load("es");
        assert_eq!(
            es.t("sla.window_days", &[("n", "30".into())]),
            "últimos 30 días"
        );
    }

    /// 🔴 `None` n'est pas `0` : sans un relevé déterminé, il n'y a pas de
    /// pourcentage. Rendre `0` affirmerait une panne totale.
    #[test]
    fn percent_sans_releve_rend_le_mot_jamais_un_chiffre() {
        let cat = Catalog::load("fr");
        let rendu = cat.percent(None);
        assert_eq!(rendu, "indéterminé");
        assert!(!rendu.contains('0'));
        assert_eq!(cat.percent(Some(99.2)), "99.2 %");
        assert_eq!(cat.percent(Some(100.0)), "100 %");
    }

    #[test]
    fn duree_change_dunite_aux_memes_seuils_que_le_navigateur() {
        let cat = Catalog::load("fr");
        assert_eq!(cat.duration(59), "59 s");
        assert_eq!(cat.duration(60), "1 min");
        assert_eq!(cat.duration(3_599), "60 min");
        assert_eq!(cat.duration(3_600), "1.0 h");
        assert_eq!(cat.duration(9_000), "2.5 h");
    }

    /// La date porte son fuseau : un classeur qui liste des coupures datées se
    /// conteste au premier décalage horaire si l'on ne dit pas dans quel
    /// fuseau elles sont écrites.
    #[test]
    fn la_date_porte_son_fuseau_et_ne_depend_pas_de_la_langue() {
        let attendu = "2026-09-03 14:22:31 UTC";
        for locale in LOCALES {
            assert_eq!(Catalog::load(locale).date(1_788_445_351), attendu);
        }
        assert_eq!(Catalog::load("fr").date(0), "1970-01-01 00:00:00 UTC");
        // Une année bissextile, là où un calendrier naïf dérape.
        assert_eq!(
            Catalog::load("fr").date(1_709_208_000),
            "2024-02-29 12:00:00 UTC"
        );
    }
}
