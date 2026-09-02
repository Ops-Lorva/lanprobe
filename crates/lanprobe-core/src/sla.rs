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

    #[test]
    fn test_sla_empty() {
        // Aucune mesure du tout : indéterminé, jamais « 0 % ».
        let stats = compute_sla("8.8.8.8", &[]);
        assert_eq!(stats.total_samples, 0);
        assert_eq!(stats.uptime_pct, None);
        assert_eq!(stats.undetermined_samples, 0);
    }
}
