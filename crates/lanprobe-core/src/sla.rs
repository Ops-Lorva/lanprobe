use serde::Serialize;

/// Disponibilité d'une cible sur une période.
#[derive(Debug, Serialize, Default)]
pub struct SlaStats {
    pub ip: String,
    /// Part des relevés DÉTERMINÉS où la cible répondait.
    ///
    /// 🔴 `None` = aucun relevé déterminé sur la période : on ne sait pas.
    ///
    /// ⚠️ C'est un `Option` et pas un `f64` accompagné d'un drapeau, parce que
    /// le type doit INTERDIRE d'imprimer un pourcentage qu'on n'a pas. Rendre
    /// `0.0` affirmait une panne totale — pire que le faux 100 % qu'on
    /// corrigeait, et dans un document remis au client. Un drapeau à côté
    /// finit toujours par ne pas être lu.
    pub uptime_pct: Option<f64>,
    pub avg_latency_ms: Option<f64>,
    pub min_latency_ms: Option<u64>,
    pub max_latency_ms: Option<u64>,
    pub p95_latency_ms: Option<u64>,
    /// Relevés DÉTERMINÉS — le dénominateur de `uptime_pct`.
    pub total_samples: usize,
    pub failed_samples: usize,
    /// Relevés sans verdict de disponibilité.
    ///
    /// ⚠️ Compté et rendu, jamais tu : « 100 % sur trois relevés » et « 100 %
    /// sur trois relevés et neuf mille indéterminés » ne se lisent pas pareil.
    #[serde(default)]
    pub undetermined_samples: usize,
}

#[derive(Debug)]
pub struct PingSample {
    /// `None` = mesure **indéterminée** : la sonde n'a pas écrit `alive`.
    ///
    /// ⚠️ Ce n'est ni « en panne » ni « disponible ». On le déduisait de la
    /// présence d'une latence (`alive.unwrap_or(latency_ms.is_some())`), ce
    /// qui faisait d'un silence un fait — la faute que ce dépôt refuse.
    pub alive: Option<bool>,
    pub latency_ms: Option<u64>,
}

/// ⚠️ **Ce qui n'a pas été mesuré sort du dénominateur**, il ne le gonfle pas
/// et ne le déflate pas. Une période sans relevé n'est ni disponible ni en
/// panne : elle est indéterminée, et `uptime_pct` vaut alors `None`.
///
/// Les latences, elles, restent comptées même sur un relevé indéterminé : une
/// latence écrite EST une mesure vraie, l'écarter perdrait de l'information
/// réelle pour un verdict manquant.
pub fn compute_sla(ip: &str, samples: &[PingSample]) -> SlaStats {
    let determines: Vec<bool> = samples.iter().filter_map(|s| s.alive).collect();
    let total = determines.len();
    let failed = determines.iter().filter(|alive| !**alive).count();
    let uptime_pct = (total > 0).then(|| ((total - failed) as f64 / total as f64) * 100.0);

    let latencies: Vec<u64> = samples.iter().filter_map(|s| s.latency_ms).collect();
    let (avg, min, max, p95) = if latencies.is_empty() {
        (None, None, None, None)
    } else {
        let avg = latencies.iter().sum::<u64>() as f64 / latencies.len() as f64;
        let min = *latencies.iter().min().unwrap();
        let max = *latencies.iter().max().unwrap();
        let mut sorted = latencies.clone();
        sorted.sort();
        let p95_idx = (sorted.len() as f64 * 0.95) as usize;
        let p95 = sorted[p95_idx.min(sorted.len() - 1)];
        (Some(avg), Some(min), Some(max), Some(p95))
    };

    SlaStats {
        ip: ip.to_string(),
        uptime_pct,
        avg_latency_ms: avg,
        min_latency_ms: min,
        max_latency_ms: max,
        p95_latency_ms: p95,
        total_samples: total,
        failed_samples: failed,
        undetermined_samples: samples.len() - total,
    }
}


// ── Couverture d'une fenêtre ───────────────────────────────────────────────

/// Multiple de l'intervalle médian au-delà duquel un espacement est un TROU.
///
/// ⚠️ Un facteur, jamais une durée : c'est tout l'objet du calcul. Écrire
/// « au-delà de 3 s » supposerait une cadence d'une seconde, et redeviendrait
/// faux — en silence — le jour où la cadence sera réglable. Le seuil se déduit
/// des données elles-mêmes.
///
/// Trois, comme le seuil « en ligne » du parc (dernier battement < 3 ×
/// l'intervalle annoncé) : deux relevés manqués peuvent arriver, trois d'un
/// coup décrivent autre chose.
const GAP_FACTOR: i64 = 3;

/// Ce qui a réellement été mesuré sur une fenêtre.
///
/// ⚠️ **Information de complétude, pas d'alerte.** Les trous d'aujourd'hui
/// viennent surtout d'une période antérieure à la fonctionnalité ; ils doivent
/// tendre vers zéro. Un affichage alarmant ferait chercher une panne
/// inexistante — la faute qu'on corrige, dans l'autre sens.
#[derive(Debug, Serialize, Default, PartialEq)]
pub struct Coverage {
    pub window_secs: i64,
    pub covered_secs: i64,
    pub gap_secs: i64,
}

impl Coverage {
    /// Part couverte, ou `None` si la fenêtre est vide — jamais une division
    /// par zéro, jamais un « 100 % » de complaisance.
    pub fn pct(&self) -> Option<f64> {
        (self.window_secs > 0).then(|| self.covered_secs as f64 / self.window_secs as f64 * 100.0)
    }

    /// ⚠️ Rien à afficher quand c'est complet. Un « 0 % indéterminé » permanent
    /// est du bruit qui finit par masquer le cas où ça compte.
    ///
    /// ⚠️ Une fenêtre VIDE n'est pas complète, elle est vide. Sans la première
    /// condition, `gap_secs == 0` la déclarait « complète » — un « tout va
    /// bien » sur une période qui n'existe pas.
    pub fn is_complete(&self) -> bool {
        self.window_secs > 0 && self.gap_secs == 0
    }
}

/// Couverture d'une fenêtre, déduite des **trous réellement observés**.
///
/// 🔴 Aucune cadence n'est supposée nulle part. Le seuil au-delà duquel un
/// espacement devient un trou est l'intervalle MÉDIAN observé sur la fenêtre,
/// multiplié par [`GAP_FACTOR`]. À 60 s de cadence, 70 s d'écart est normal ;
/// à 1 s, c'est un trou. Seules les données tranchent.
///
/// ⚠️ **Les bords comptent.** Entre le début de la fenêtre et la première
/// mesure, puis entre la dernière et la fin, ce sont aussi des trous. Les
/// oublier ferait passer une sonde enrôlée hier pour couvrant 100 % d'une
/// fenêtre de 87 jours — le faux le plus gros possible, et le plus facile à
/// laisser passer.
///
/// ⚠️ Moins de deux relevés ne donne AUCUN intervalle, donc aucune médiane :
/// rien ne permet de dire quelle durée un point isolé représente. On ne lui en
/// attribue aucune, plutôt que d'inventer la cadence que ce calcul refuse
/// justement de supposer.
pub fn compute_coverage(sample_times: &[i64], from: i64, to: i64) -> Coverage {
    let window_secs = (to - from + 1).max(0);
    if window_secs == 0 {
        return Coverage::default();
    }
    if sample_times.len() < 2 {
        return Coverage { window_secs, covered_secs: 0, gap_secs: window_secs };
    }

    let mut times = sample_times.to_vec();
    times.sort_unstable();

    let mut intervals: Vec<i64> = times.windows(2).map(|w| w[1] - w[0]).collect();
    intervals.sort_unstable();
    let median = intervals[intervals.len() / 2].max(1);
    let threshold = median.saturating_mul(GAP_FACTOR);

    // Un espacement anormal ne manque que de son EXCÉDENT : à cadence
    // médiane, le pas normal est déjà couvert par le relevé qui l'ouvre.
    let mut gap_secs: i64 = intervals.iter().filter(|d| **d > threshold).map(|d| d - median).sum();

    // Les bords, eux, ne sont ouverts par aucun relevé : ils comptent en
    // entier.
    let lead = times[0] - from;
    if lead > threshold {
        gap_secs += lead;
    }
    let tail = to - times[times.len() - 1];
    if tail > threshold {
        gap_secs += tail;
    }

    let gap_secs = gap_secs.clamp(0, window_secs);
    Coverage { window_secs, covered_secs: window_secs - gap_secs, gap_secs }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sla_perfect_uptime() {
        let samples = vec![
            PingSample { alive: Some(true), latency_ms: Some(10) },
            PingSample { alive: Some(true), latency_ms: Some(20) },
            PingSample { alive: Some(true), latency_ms: Some(15) },
        ];
        let stats = compute_sla("8.8.8.8", &samples);
        assert_eq!(stats.uptime_pct, Some(100.0));
        assert_eq!(stats.min_latency_ms, Some(10));
        assert_eq!(stats.max_latency_ms, Some(20));
    }

    #[test]
    fn test_sla_partial_uptime() {
        let samples = vec![
            PingSample { alive: Some(true), latency_ms: Some(10) },
            PingSample { alive: Some(false), latency_ms: None },
        ];
        let stats = compute_sla("8.8.8.8", &samples);
        assert_eq!(stats.uptime_pct, Some(50.0));
        assert_eq!(stats.failed_samples, 1);
    }

    #[test]
    fn an_undetermined_sample_leaves_the_denominator_instead_of_deflating_it() {
        // ⚠️ Une mesure sans `alive` n'est PAS une indisponibilité : la sonde
        // n'a rien dit. La compter comme un échec donnait 50 % là où la seule
        // mesure existante dit 100 %.
        let samples = vec![
            PingSample { alive: Some(true), latency_ms: Some(10) },
            PingSample { alive: None, latency_ms: None },
        ];
        let stats = compute_sla("8.8.8.8", &samples);
        assert_eq!(stats.uptime_pct, Some(100.0));
        assert_eq!(stats.total_samples, 1, "seuls les relevés déterminés comptent");
        assert_eq!(stats.failed_samples, 0);
        assert_eq!(stats.undetermined_samples, 1, "et l'indétermination se DIT");
    }

    #[test]
    fn a_window_without_a_single_determinate_sample_has_no_percentage_at_all() {
        // 🔴 Le cœur de l'arbitrage. Rendre `0.0` ici affirmait une panne
        // totale — pire que le faux 100 % qu'on venait de corriger, et dans
        // un document remis au client.
        //
        // ⚠️ C'est le TYPE qui l'interdit, pas la vigilance de l'appelant :
        // `Option<f64>` ne s'imprime pas sans qu'on ait décidé quoi faire du
        // cas « rien à dire ».
        let samples = vec![
            PingSample { alive: None, latency_ms: Some(12) },
            PingSample { alive: None, latency_ms: None },
        ];
        let stats = compute_sla("8.8.8.8", &samples);
        assert_eq!(stats.uptime_pct, None);
        assert_eq!(stats.total_samples, 0);
        assert_eq!(stats.undetermined_samples, 2);
        // La latence relevée reste une mesure vraie : elle n'est pas perdue.
        assert_eq!(stats.min_latency_ms, Some(12));
    }

    // ── Couverture ─────────────────────────────────────────────────────

    /// Fenêtre d'une heure, relevés à la seconde.
    fn seconde_par_seconde(from: i64, combien: i64) -> Vec<i64> {
        (0..combien).map(|i| from + i).collect()
    }

    #[test]
    fn a_window_measured_end_to_end_has_nothing_undetermined() {
        // Cadence régulière, pas de trou : rien à signaler. ⚠️ C'est le cas
        // NORMAL, et l'écran ne doit alors rien afficher du tout.
        let c = compute_coverage(&seconde_par_seconde(1_000, 3_600), 1_000, 4_599);
        assert_eq!(c.gap_secs, 0);
        assert!(c.is_complete());
    }

    #[test]
    fn a_real_hole_is_counted_for_its_full_duration() {
        // Une heure à la seconde, moins vingt minutes au milieu.
        let mut t = seconde_par_seconde(0, 1_200);
        t.extend(seconde_par_seconde(2_400, 1_200));
        let c = compute_coverage(&t, 0, 3_599);
        assert_eq!(c.gap_secs, 1_200, "le trou vaut sa durée réelle");
        assert_eq!(c.window_secs, 3_600);
    }

    #[test]
    fn the_edges_of_the_window_are_holes_too() {
        // 🔴 Le faux le plus gros possible : une sonde enrôlée hier passerait
        // pour couvrir 100 % d'une fenêtre de 87 jours. Entre le début de la
        // fenêtre et la première mesure, il ne s'est rien passé de mesuré.
        let t = seconde_par_seconde(86_000, 400);
        let c = compute_coverage(&t, 0, 86_399);
        assert_eq!(c.gap_secs, 86_000, "tout ce qui précède la première mesure");
        assert!(!c.is_complete());
    }

    #[test]
    fn no_sample_at_all_covers_nothing_and_divides_by_nothing() {
        // ⚠️ Ni 100 %, ni division par zéro : on n'a rien mesuré.
        let c = compute_coverage(&[], 0, 3_599);
        assert_eq!(c.gap_secs, 3_600);
        assert_eq!(c.covered_secs, 0);
        assert_eq!(c.pct(), Some(0.0));
        assert!(!c.is_complete());
    }

    #[test]
    fn a_single_sample_establishes_no_cadence_so_it_covers_nothing() {
        // ⚠️ Un point isolé ne donne AUCUN intervalle, donc aucune médiane :
        // rien ne permet de dire quelle durée il représente. Lui en attribuer
        // une serait inventer la cadence que ce calcul refuse justement de
        // supposer.
        let c = compute_coverage(&[1_800], 0, 3_599);
        assert_eq!(c.covered_secs, 0);
        assert_eq!(c.gap_secs, 3_600);
    }

    #[test]
    fn a_window_shorter_than_the_median_interval_is_not_a_hole() {
        // Deux relevés espacés de 8 s dans une fenêtre de 10 s : le seuil vaut
        // 24 s, aucun bord ne le dépasse. Rien à signaler.
        let c = compute_coverage(&[1, 9], 0, 10);
        assert_eq!(c.gap_secs, 0);
    }

    #[test]
    fn an_empty_window_is_not_a_percentage() {
        // Bornes inversées ou nulles : aucune période à couvrir.
        // Bornes inversées : aucune période, donc aucun pourcentage — et
        // surtout pas « complète », qui se lirait comme « tout va bien ».
        assert_eq!(compute_coverage(&[], 200, 100).pct(), None);
        assert!(!compute_coverage(&[], 200, 100).is_complete());
        // Une seconde sans relevé, en revanche, est bien 0 % de couverture.
        assert_eq!(compute_coverage(&[], 100, 100).pct(), Some(0.0));
    }

    #[test]
    fn the_threshold_comes_from_the_data_never_from_an_assumed_rate() {
        // 🔴 Le cœur du choix : AUCUNE cadence n'est écrite en dur. À 60 s
        // d'intervalle, un espacement de 70 s est normal ; à 1 s, c'est un
        // trou. Les deux jeux ci-dessous ont le même espacement anormal et
        // des verdicts opposés — c'est la médiane observée qui tranche.
        let lent: Vec<i64> = vec![0, 60, 120, 180, 250, 310, 370];
        assert_eq!(compute_coverage(&lent, 0, 370).gap_secs, 0, "70 s à cadence 60 s");

        let rapide: Vec<i64> = vec![0, 1, 2, 3, 73, 74, 75];
        // 70 s d'écart, dont 1 s déjà couverte par le relevé qui l'ouvre : 69.
        assert_eq!(compute_coverage(&rapide, 0, 75).gap_secs, 69, "trou à cadence 1 s");
    }

    #[test]
    fn test_sla_empty() {
        // Aucune mesure du tout : indéterminé, jamais « 0 % ».
        let stats = compute_sla("8.8.8.8", &[]);
        assert_eq!(stats.total_samples, 0);
        assert_eq!(stats.uptime_pct, None);
        assert_eq!(stats.undetermined_samples, 0);
    }
}
