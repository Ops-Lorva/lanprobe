//! Déduire l'adresse réelle d'un client derrière un reverse proxy.
//!
//! ⚠️ `X-Forwarded-For` est écrit par le client. Il n'est lu QUE pour les sauts
//! qui figurent dans la liste des proxies de confiance. Liste vide = en-tête
//! ignoré. Sans cette règle, deux échecs symétriques : derrière un proxy toutes
//! les sondes portent l'adresse du proxy, et en faisant confiance à l'en-tête
//! n'importe qui forge la sienne.

use std::net::IpAddr;

/// Un préfixe réseau, réduit à ce dont on a besoin ici.
#[derive(Debug, Clone, Copy)]
pub struct IpNet {
    base: IpAddr,
    bits: u8,
}

impl IpNet {
    fn contains(&self, addr: IpAddr) -> bool {
        match (self.base, addr) {
            (IpAddr::V4(b), IpAddr::V4(a)) => {
                if self.bits == 0 {
                    return true;
                }
                let mask = u32::MAX << (32 - self.bits.min(32));
                u32::from(b) & mask == u32::from(a) & mask
            }
            (IpAddr::V6(b), IpAddr::V6(a)) => {
                if self.bits == 0 {
                    return true;
                }
                let mask = u128::MAX << (128 - self.bits.min(128));
                u128::from(b) & mask == u128::from(a) & mask
            }
            // Un préfixe v4 ne dit rien d'une adresse v6, et réciproquement :
            // les comparer reviendrait à faire confiance à un saut qu'on n'a
            // pas décrit.
            _ => false,
        }
    }
}

/// Analyse une liste de CIDR séparés par des virgules. Une entrée illisible est
/// ignorée : un réglage à moitié fautif ne doit pas désactiver tout le reste.
/// Une adresse nue vaut un préfixe complet (`/32` ou `/128`).
pub fn parse_cidrs(value: &str) -> Vec<IpNet> {
    value
        .split(',')
        .filter_map(|entry| {
            let entry = entry.trim();
            if entry.is_empty() {
                return None;
            }
            let (addr, bits) = match entry.split_once('/') {
                Some((a, b)) => (a.trim(), b.trim().parse::<u8>().ok()?),
                None => (entry, if entry.contains(':') { 128 } else { 32 }),
            };
            let base: IpAddr = addr.parse().ok()?;
            Some(IpNet { base, bits })
        })
        .collect()
}

/// Adresse du client. On part de la socket et on remonte `X-Forwarded-For` de
/// droite à gauche tant que le saut examiné est un proxy de confiance.
pub fn resolve(peer: IpAddr, forwarded: Option<&str>, trusted: &[IpNet]) -> IpAddr {
    if trusted.is_empty() {
        return peer;
    }
    // La socket elle-même doit être un proxy de confiance : sinon l'en-tête
    // vient d'un client qui parle au hub en direct, et il vaut ce qu'il vaut.
    if !trusted.iter().any(|n| n.contains(peer)) {
        return peer;
    }
    let Some(header) = forwarded else {
        return peer;
    };
    let hops: Vec<IpAddr> = header
        .split(',')
        .filter_map(|h| h.trim().parse().ok())
        .collect();
    let mut candidate = peer;
    for hop in hops.into_iter().rev() {
        candidate = hop;
        if !trusted.iter().any(|n| n.contains(hop)) {
            break;
        }
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> std::net::IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn without_trusted_proxies_the_header_is_ignored() {
        // Sinon n'importe quelle sonde forge son adresse et fausse la
        // détection de changement pour tout le parc.
        let got = resolve(ip("10.0.0.5"), Some("1.2.3.4"), &[]);
        assert_eq!(got, ip("10.0.0.5"));
    }

    #[test]
    fn a_trusted_proxy_hands_over_the_address_it_forwarded() {
        let trusted = parse_cidrs("10.0.0.0/8");
        let got = resolve(ip("10.0.0.5"), Some("88.120.0.1"), &trusted);
        assert_eq!(got, ip("88.120.0.1"));
    }

    #[test]
    fn the_walk_stops_at_the_first_untrusted_hop() {
        // Chaîne « client, proxy forgé, proxy de confiance » : on ne remonte
        // pas au-delà du premier saut hors liste, sinon un client interpose
        // ce qu'il veut.
        let trusted = parse_cidrs("10.0.0.0/8");
        let got = resolve(
            ip("10.0.0.5"),
            Some("9.9.9.9, 203.0.113.7, 10.0.0.9"),
            &trusted,
        );
        assert_eq!(got, ip("203.0.113.7"));
    }

    #[test]
    fn a_malformed_header_falls_back_to_the_socket() {
        let trusted = parse_cidrs("10.0.0.0/8");
        assert_eq!(
            resolve(ip("10.0.0.5"), Some("pas-une-ip"), &trusted),
            ip("10.0.0.5")
        );
        assert_eq!(resolve(ip("10.0.0.5"), Some(""), &trusted), ip("10.0.0.5"));
    }

    #[test]
    fn cidrs_are_parsed_leniently_and_bad_entries_dropped() {
        let nets = parse_cidrs("10.0.0.0/8, pas-un-cidr , 192.168.1.1");
        assert_eq!(
            nets.len(),
            2,
            "une entrée illisible ne doit pas tuer la liste"
        );
    }
}
