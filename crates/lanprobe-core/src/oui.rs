use std::collections::HashMap;
use std::sync::OnceLock;

const OUI_TSV: &str = include_str!("../data/oui.tsv");

struct OuiDb {
    p24: HashMap<u32, &'static str>,
    p28: HashMap<u32, &'static str>,
    p36: HashMap<u64, &'static str>,
}

fn db() -> &'static OuiDb {
    static DB: OnceLock<OuiDb> = OnceLock::new();
    DB.get_or_init(|| {
        let mut p24 = HashMap::new();
        let mut p28 = HashMap::new();
        let mut p36 = HashMap::new();
        for line in OUI_TSV.lines() {
            let mut it = line.splitn(3, '\t');
            let (Some(prefix), Some(bits), Some(vendor)) = (it.next(), it.next(), it.next()) else { continue };
            let bits: u8 = match bits.parse() { Ok(b) => b, Err(_) => continue };
            // Le préfixe est en hex ; on le ramène à sa valeur entière.
            match bits {
                24 => { if let Ok(v) = u32::from_str_radix(prefix, 16) { p24.insert(v, vendor); } }
                28 => { if let Ok(v) = u32::from_str_radix(prefix, 16) { p28.insert(v, vendor); } }
                36 => { if let Ok(v) = u64::from_str_radix(prefix, 16) { p36.insert(v, vendor); } }
                _ => {}
            }
        }
        OuiDb { p24, p28, p36 }
    })
}

/// Résout le fabricant à partir d'une adresse MAC (accepte `:` ou `-`, casse libre).
/// Teste le préfixe le plus long d'abord (36 → 28 → 24 bits).
pub fn vendor_for_mac(mac: &str) -> Option<String> {
    let hex: String = mac.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() < 12 {
        return None;
    }
    let mac48 = u64::from_str_radix(&hex[..12], 16).ok()?;
    let db = db();
    let p36 = (mac48 >> 12) as u64;            // 36 bits de poids fort
    if let Some(v) = db.p36.get(&p36) { return Some((*v).to_string()); }
    let p28 = (mac48 >> 20) as u32;            // 28 bits
    if let Some(v) = db.p28.get(&p28) { return Some((*v).to_string()); }
    let p24 = (mac48 >> 24) as u32;            // 24 bits
    if let Some(v) = db.p24.get(&p24) { return Some((*v).to_string()); }
    None
}

#[cfg(test)]
mod tests {
    use super::vendor_for_mac;

    #[test]
    fn resolves_known_ma_l_apple() {
        assert!(vendor_for_mac("a4:5e:60:11:22:33").unwrap().to_lowercase().contains("apple"));
    }

    #[test]
    fn accepts_dash_and_uppercase() {
        let v1 = vendor_for_mac("A4-5E-60-AA-BB-CC");
        let v2 = vendor_for_mac("a4:5e:60:aa:bb:cc");
        assert_eq!(v1, v2);
        assert!(v1.is_some());
    }

    #[test]
    fn unknown_or_invalid_is_none() {
        // Préfixe non assigné (locally administered) + format invalide
        assert_eq!(vendor_for_mac("zz:zz:zz:zz:zz:zz"), None);
        assert_eq!(vendor_for_mac("02:00:00:00:00:01"), None);
        assert_eq!(vendor_for_mac("a4:5e"), None);
    }
}
