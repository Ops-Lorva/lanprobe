//! Second facteur par code à usage unique (TOTP, RFC 6238).
//!
//! Écrit ici plutôt que tiré d'une caisse : l'algorithme tient en quarante
//! lignes au-dessus du HMAC-SHA1 que `ring` fournit déjà, et une dépendance de
//! plus dans un binaire qu'on demande à l'utilisateur d'auto-héberger est une
//! surface de plus à surveiller. Ce qui est délicat dans le TOTP n'est pas le
//! calcul — c'est la fenêtre de tolérance et la réutilisation d'un code, et
//! ces deux-là se traitent ici, pas dans une bibliothèque.

use ring::hmac;

/// Pas de temps, en secondes. Trente : ce que font Google Authenticator,
/// Aegis, 1Password et Bitwarden. Le changer rendrait les codes faux partout.
pub const STEP_SECS: u64 = 30;

/// Nombre de chiffres. Six, pour la même raison.
pub const DIGITS: u32 = 6;

/// Fenêtre acceptée, en pas, de part et d'autre du pas courant.
///
/// ⚠️ Un pas de tolérance et pas trois. L'horloge d'un téléphone est
/// synchronisée ; ce qu'on couvre ici, c'est le code tapé à cheval sur un
/// changement de pas. Élargir la fenêtre multiplie mécaniquement les codes
/// valides à un instant donné — c'est-à-dire affaiblit le second facteur pour
/// rattraper une horloge qu'il faudrait plutôt remettre à l'heure.
pub const SKEW_STEPS: i64 = 1;

/// Longueur du secret partagé, en octets. 20 = 160 bits, la taille native de
/// SHA-1 et ce que produisent les générateurs des applications courantes.
pub const SECRET_BYTES: usize = 20;

/// Tire un secret et le rend en base32, prêt pour une URI `otpauth://`.
pub fn generate_secret() -> Result<String, String> {
    let mut bytes = [0u8; SECRET_BYTES];
    getrandom::getrandom(&mut bytes).map_err(|e| e.to_string())?;
    Ok(base32_encode(&bytes))
}

/// Base32 RFC 4648, sans remplissage.
///
/// Sans remplissage parce que les applications d'authentification l'acceptent
/// toutes ainsi, et que les `=` finaux se perdent au copier-coller.
pub fn base32_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::new();
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for byte in data {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 0x1f) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 0x1f) as usize] as char);
    }
    out
}

/// Décode une base32 tolérante : espaces et minuscules acceptés.
///
/// L'utilisateur recopie parfois le secret d'une application à l'autre, et
/// celles-ci l'affichent par groupes de quatre. Refuser sur une espace serait
/// un refus sans cause compréhensible.
pub fn base32_decode(s: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for c in s.chars() {
        if c == ' ' || c == '-' || c == '=' {
            continue;
        }
        let value = match c.to_ascii_uppercase() {
            c @ 'A'..='Z' => c as u32 - 'A' as u32,
            c @ '2'..='7' => c as u32 - '2' as u32 + 26,
            other => return Err(format!("caractère inattendu dans le secret : {other}")),
        };
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    if out.is_empty() {
        return Err("secret vide".into());
    }
    Ok(out)
}

/// Le code attendu pour un pas donné.
fn code_at(secret: &[u8], step: u64) -> u32 {
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret);
    let tag = hmac::sign(&key, &step.to_be_bytes());
    let digest = tag.as_ref();
    // Troncature dynamique de la RFC 4226 : le dernier quartet donne le
    // décalage des quatre octets à retenir.
    let offset = (digest[digest.len() - 1] & 0x0f) as usize;
    let binary = ((u32::from(digest[offset]) & 0x7f) << 24)
        | (u32::from(digest[offset + 1]) << 16)
        | (u32::from(digest[offset + 2]) << 8)
        | u32::from(digest[offset + 3]);
    binary % 10u32.pow(DIGITS)
}

/// Le pas de temps courant pour un instant donné.
pub fn step_of(unix_secs: i64) -> u64 {
    (unix_secs.max(0) as u64) / STEP_SECS
}

/// Vérifie un code et rend le pas qui l'a validé.
///
/// 🔴 Le pas est rendu, et non un simple booléen, parce que **l'appelant doit
/// le retenir**. Un code TOTP reste valide trente secondes : sans mémoire du
/// dernier pas accepté, un code intercepté se rejoue autant de fois qu'on veut
/// dans sa fenêtre. C'est le seul point de cette implémentation où une erreur
/// se paie en accès.
pub fn verify(secret_b32: &str, code: &str, now: i64, last_used_step: Option<u64>) -> Option<u64> {
    let secret = base32_decode(secret_b32).ok()?;
    let typed: String = code.chars().filter(|c| c.is_ascii_digit()).collect();
    if typed.len() != DIGITS as usize {
        return None;
    }
    let typed: u32 = typed.parse().ok()?;
    let current = step_of(now) as i64;
    for delta in -SKEW_STEPS..=SKEW_STEPS {
        let step = (current + delta).max(0) as u64;
        if let Some(used) = last_used_step {
            if step <= used {
                continue;
            }
        }
        // Comparaison en temps constant : le code fait six chiffres, un
        // attaquant qui mesure la différence entre « faux au premier chiffre »
        // et « faux au dernier » réduit l'espace de recherche.
        if ring::constant_time::verify_slices_are_equal(
            &code_at(&secret, step).to_be_bytes(),
            &typed.to_be_bytes(),
        )
        .is_ok()
        {
            return Some(step);
        }
    }
    None
}

/// URI `otpauth://` à mettre dans le QR code.
///
/// L'émetteur est répété dans le libellé ET dans le paramètre : les
/// applications anciennes ne lisent que le premier, les récentes préfèrent le
/// second, et sans les deux le compte s'affiche sans nom chez la moitié des
/// gens.
pub fn provisioning_uri(issuer: &str, account: &str, secret_b32: &str) -> String {
    let enc = |s: &str| {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    (b as char).to_string()
                }
                other => format!("%{other:02X}"),
            })
            .collect::<String>()
    };
    format!(
        "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm=SHA1&digits={}&period={}",
        enc(issuer),
        enc(account),
        secret_b32,
        enc(issuer),
        DIGITS,
        STEP_SECS
    )
}

/// QR code du provisionnement, en SVG.
///
/// Rendu côté hub et non par une bibliothèque JavaScript : le secret n'a alors
/// aucune raison de traverser une dépendance de plus, et l'interface reste
/// lisible sans réseau vers un CDN — ce qui compte pour un produit
/// auto-hébergé sur un réseau fermé.
pub fn qr_svg(uri: &str) -> Result<String, String> {
    use qrcode::render::svg;
    let code = qrcode::QrCode::new(uri.as_bytes()).map_err(|e| e.to_string())?;
    Ok(code
        .render()
        .min_dimensions(200, 200)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vecteur de la RFC 6238 : secret ASCII « 12345678901234567890 », SHA-1.
    const RFC_SECRET: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

    #[test]
    fn the_rfc_6238_vectors_hold() {
        // Si ce test tombe, tous les codes du produit sont faux : c'est le
        // seul point où l'on peut se comparer à une référence extérieure.
        let secret = base32_decode(RFC_SECRET).unwrap();
        assert_eq!(secret, b"12345678901234567890");
        assert_eq!(code_at(&secret, step_of(59)), 287_082);
        assert_eq!(code_at(&secret, step_of(1_111_111_109)), 81_804);
        assert_eq!(code_at(&secret, step_of(1_234_567_890)), 5_924);
    }

    #[test]
    fn base32_round_trips() {
        let bytes: Vec<u8> = (0u8..=40).collect();
        assert_eq!(base32_decode(&base32_encode(&bytes)).unwrap(), bytes);
    }

    #[test]
    fn a_secret_copied_with_spaces_and_lowercase_still_decodes() {
        // Les applications affichent le secret par groupes de quatre.
        let spaced = "gezd gnbv gy3t qojq gezd gnbv gy3t qojq";
        assert_eq!(base32_decode(spaced).unwrap(), b"12345678901234567890");
    }

    #[test]
    fn a_code_is_accepted_one_step_late_but_not_two() {
        let now = 1_234_567_890i64;
        let secret = base32_decode(RFC_SECRET).unwrap();
        let previous = format!("{:06}", code_at(&secret, step_of(now) - 1));
        let ancient = format!("{:06}", code_at(&secret, step_of(now) - 3));

        assert!(verify(RFC_SECRET, &previous, now, None).is_some());
        assert!(
            verify(RFC_SECRET, &ancient, now, None).is_none(),
            "élargir la fenêtre multiplierait les codes valides à un instant donné"
        );
    }

    #[test]
    fn a_code_cannot_be_replayed_within_its_own_window() {
        // 🔴 Sans mémoire du dernier pas accepté, un code intercepté se rejoue
        // pendant trente secondes. C'est le seul défaut de cette
        // implémentation qui se paierait en accès.
        let now = 1_234_567_890i64;
        let secret = base32_decode(RFC_SECRET).unwrap();
        let code = format!("{:06}", code_at(&secret, step_of(now)));

        let used = verify(RFC_SECRET, &code, now, None).expect("premier usage accepté");
        assert!(
            verify(RFC_SECRET, &code, now, Some(used)).is_none(),
            "le même code ne doit pas passer deux fois"
        );
    }

    #[test]
    fn a_malformed_code_is_refused_without_panicking() {
        let now = 1_234_567_890i64;
        for bad in ["", "12345", "1234567", "abcdef", "  "] {
            assert!(verify(RFC_SECRET, bad, now, None).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn the_provisioning_uri_names_the_issuer_twice() {
        let uri = provisioning_uri("LanProbe Hub", "claire", RFC_SECRET);
        assert!(uri.starts_with("otpauth://totp/LanProbe%20Hub:claire?"));
        assert!(uri.contains("issuer=LanProbe%20Hub"));
        assert!(uri.contains(&format!("secret={RFC_SECRET}")));
    }

    #[test]
    fn the_qr_is_a_real_svg() {
        let svg = qr_svg(&provisioning_uri("LanProbe", "claire", RFC_SECRET)).unwrap();
        assert!(svg.contains("<svg"));
    }
}
