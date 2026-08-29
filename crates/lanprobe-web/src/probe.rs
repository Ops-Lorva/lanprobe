//! Sonde vue par le hub, et dérivation de son statut.

/// Au-delà de ce silence, une sonde n'est plus « en retard » mais absente.
///
/// ⚠️ Deux heures, pas vingt-quatre. Le seuil ne décrit pas une panne, il
/// décide **quand on doit se déplacer** : un redémarrage ou une mise à jour
/// dépassent rarement vingt minutes, et une sonde muette depuis deux heures
/// est un problème qu'on veut régler dans la journée, pas retrouver le
/// lendemain matin.
const OFFLINE_AFTER_SECS: i64 = 2 * 3600;

/// Statut d'une sonde. **Dérivé du dernier battement, jamais stocké** :
/// une colonne `status` obligerait quelqu'un à la tenir à jour, et une valeur
/// dérivée ne peut pas mentir sur sa propre fraîcheur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeStatus {
    Online,
    Stale,
    Offline,
}

impl ProbeStatus {
    pub fn derive(last_seen: Option<i64>, now: i64, heartbeat_interval_secs: i64) -> Self {
        let Some(last_seen) = last_seen else {
            // Enrôlée mais jamais vue : elle n'a rien à dire de rassurant.
            return ProbeStatus::Offline;
        };
        // Une sonde dont l'horloge avance sur celle du hub donnerait un âge
        // négatif ; elle vient de parler, c'est tout ce qui compte.
        let age = (now - last_seen).max(0);
        if age < 3 * heartbeat_interval_secs {
            ProbeStatus::Online
        } else if age < OFFLINE_AFTER_SECS {
            ProbeStatus::Stale
        } else {
            ProbeStatus::Offline
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ProbeStatus::Online => "online",
            ProbeStatus::Stale => "stale",
            ProbeStatus::Offline => "offline",
        }
    }
}

impl serde::Serialize for ProbeStatus {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERVAL: i64 = 60;

    #[test]
    fn a_probe_never_seen_is_offline() {
        assert_eq!(ProbeStatus::derive(None, 1_000_000, INTERVAL), ProbeStatus::Offline);
    }

    #[test]
    fn status_is_online_just_under_three_intervals() {
        // 3 × 60 = 180 s : à 179 s la sonde a encore raté moins de trois
        // battements, on la considère en ligne.
        let now = 1_000_000;
        assert_eq!(
            ProbeStatus::derive(Some(now - 179), now, INTERVAL),
            ProbeStatus::Online
        );
    }

    #[test]
    fn status_becomes_stale_at_exactly_three_intervals() {
        let now = 1_000_000;
        assert_eq!(
            ProbeStatus::derive(Some(now - 180), now, INTERVAL),
            ProbeStatus::Stale
        );
    }

    #[test]
    fn status_is_still_stale_just_under_the_offline_threshold() {
        let now = 1_000_000;
        assert_eq!(
            ProbeStatus::derive(Some(now - (OFFLINE_AFTER_SECS - 1)), now, INTERVAL),
            ProbeStatus::Stale
        );
    }

    #[test]
    fn a_reboot_or_an_update_never_reaches_offline() {
        // Vingt minutes de silence, c'est une mise à jour. Passer « hors
        // ligne » à ce stade enverrait quelqu'un sur place pour rien.
        let now = 1_000_000;
        assert_eq!(
            ProbeStatus::derive(Some(now - 20 * 60), now, INTERVAL),
            ProbeStatus::Stale
        );
    }

    #[test]
    fn status_becomes_offline_at_exactly_the_threshold() {
        let now = 1_000_000;
        assert_eq!(
            ProbeStatus::derive(Some(now - OFFLINE_AFTER_SECS), now, INTERVAL),
            ProbeStatus::Offline
        );
    }

    #[test]
    fn a_clock_skewed_future_heartbeat_is_online() {
        // L'horloge d'une sonde peut avancer sur celle du hub. Un âge négatif
        // ne doit pas basculer la sonde en `offline` par débordement de signe.
        let now = 1_000_000;
        assert_eq!(
            ProbeStatus::derive(Some(now + 30), now, INTERVAL),
            ProbeStatus::Online
        );
    }

    #[test]
    fn status_serializes_to_the_contract_wording() {
        assert_eq!(ProbeStatus::Online.as_str(), "online");
        assert_eq!(ProbeStatus::Stale.as_str(), "stale");
        assert_eq!(ProbeStatus::Offline.as_str(), "offline");
    }
}
