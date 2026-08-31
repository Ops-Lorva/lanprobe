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

/// Ramène `::ffff:a.b.c.d` à `a.b.c.d`.
///
/// ⚠️ Un hub qui écoute sur `[::]` — le montage dual-stack courant — reçoit les
/// sondes IPv4 sous cette forme. Sans cette normalisation, un `trusted_proxies`
/// écrit en IPv4 ne reconnaissait jamais le proxy : `X-Forwarded-For` restait
/// ignoré et la détection par adresse source était **muette en production alors
/// qu'elle passait en test**. On ne relâche rien d'autre — une IPv6 réelle ne
/// devient pas comparable à un préfixe IPv4.
fn unmap(addr: IpAddr) -> IpAddr {
    match addr {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(addr, IpAddr::V4),
        v4 => v4,
    }
}

impl IpNet {
    fn contains(&self, addr: IpAddr) -> bool {
        match (unmap(self.base), unmap(addr)) {
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

/// Vrai si l'entrée est un préfixe fourre-tout (`0.0.0.0/0`, `::/0`).
///
/// ⚠️ Un tel préfixe rend **tous** les sauts de confiance : `resolve` ne casse
/// jamais sa boucle et retourne le saut le plus à gauche de `X-Forwarded-For`,
/// que le client écrit lui-même. Le réglage a alors l'air posé sans protéger
/// quoi que ce soit — c'est exactement la valeur plausible et fausse que ce
/// dépôt refuse. La validation de `trusted_proxies` s'en sert pour dire non.
pub fn is_catch_all(entry: &str) -> bool {
    parse_cidrs(entry).iter().any(|n| n.bits == 0)
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
    fn an_ipv4_mapped_peer_is_recognised_by_an_ipv4_cidr() {
        // ⚠️ Un hub qui écoute sur `[::]` — le montage dual-stack courant —
        // voit une sonde IPv4 arriver en `::ffff:10.0.0.9`. Sans cette
        // équivalence, un `trusted_proxies` en IPv4 ne matchait jamais : la
        // détection par adresse source passait en test et restait MUETTE en
        // production, sans rien signaler.
        let trusted = parse_cidrs("10.0.0.0/8");
        let got = resolve(ip("::ffff:10.0.0.9"), Some("88.120.0.1"), &trusted);
        assert_eq!(got, ip("88.120.0.1"));
    }

    #[test]
    fn an_ipv4_mapped_hop_in_the_header_is_treated_as_its_ipv4_form() {
        let trusted = parse_cidrs("10.0.0.0/8");
        let got = resolve(
            ip("10.0.0.5"),
            Some("203.0.113.7, ::ffff:10.0.0.9"),
            &trusted,
        );
        assert_eq!(got, ip("203.0.113.7"));
    }

    #[test]
    fn a_real_ipv6_address_still_does_not_match_an_ipv4_cidr() {
        // On ne relâche rien d'autre : seule la forme mappée est ramenée à
        // l'IPv4 qu'elle représente réellement.
        let trusted = parse_cidrs("10.0.0.0/8");
        let got = resolve(ip("2001:db8::9"), Some("88.120.0.1"), &trusted);
        assert_eq!(got, ip("2001:db8::9"), "une v6 réelle n'est pas de confiance");
    }

    #[test]
    fn an_ipv4_mapped_cidr_entry_matches_a_bare_ipv4_peer() {
        // La symétrie compte : un opérateur qui écrit la forme mappée dans son
        // réglage doit couvrir la même machine qu'en IPv4 nue.
        let trusted = parse_cidrs("::ffff:10.0.0.9");
        let got = resolve(ip("10.0.0.9"), Some("88.120.0.1"), &trusted);
        assert_eq!(got, ip("88.120.0.1"));
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
