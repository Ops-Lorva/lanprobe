//! Chiffrement au repos des secrets applicatifs.
//!
//! Le mécanisme vit dans [`lanprobe_core::secrets`] : le hub web en a besoin
//! pour sceller les identifiants de notification, et deux implémentations du
//! même format `enc:v1:` finiraient par diverger. Ce module reste comme point
//! d'entrée historique de `lanprobe-server` — les appelants n'ont pas à savoir
//! que le code a déménagé.

pub use lanprobe_core::secrets::*;
