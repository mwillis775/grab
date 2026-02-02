//! Ed25519 signing and verification

use ed25519_dalek::{SigningKey, VerifyingKey, Signer, Verifier};
use crate::types::{PublicKey, Signature};
use rand::rngs::OsRng;

/// Generate a new Ed25519 keypair
pub fn generate_keypair() -> (PublicKey, [u8; 32]) {
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();
    
    (verifying_key.to_bytes(), signing_key.to_bytes())
}

/// Sign a message with a private key
pub fn sign(message: &[u8], private_key: &[u8; 32]) -> Signature {
    let signing_key = SigningKey::from_bytes(private_key);
    let signature = signing_key.sign(message);
    signature.to_bytes().to_vec()
}

/// Verify a signature
pub fn verify(message: &[u8], signature: &Signature, public_key: &PublicKey) -> bool {
    if signature.len() != 64 {
        return false;
    }
    
    let Ok(verifying_key) = VerifyingKey::from_bytes(public_key) else {
        return false;
    };
    
    let mut sig_bytes = [0u8; 64];
    sig_bytes.copy_from_slice(signature);
    
    let sig = ed25519_dalek::Signature::from_bytes(&sig_bytes);
    
    verifying_key.verify(message, &sig).is_ok()
}

/// Sign bundle metadata for authentication
pub fn sign_bundle(
    site_id: &[u8; 32],
    revision: u64,
    root_hash: &[u8; 32],
    private_key: &[u8; 32],
) -> Signature {
    let mut message = Vec::with_capacity(72);
    message.extend_from_slice(site_id);
    message.extend_from_slice(&revision.to_le_bytes());
    message.extend_from_slice(root_hash);
    
    sign(&message, private_key)
}

/// Verify bundle signature
pub fn verify_bundle(
    site_id: &[u8; 32],
    revision: u64,
    root_hash: &[u8; 32],
    signature: &Signature,
    public_key: &PublicKey,
) -> bool {
    let mut message = Vec::with_capacity(72);
    message.extend_from_slice(site_id);
    message.extend_from_slice(&revision.to_le_bytes());
    message.extend_from_slice(root_hash);
    
    verify(&message, signature, public_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keypair_generation() {
        let (public, private) = generate_keypair();
        assert_eq!(public.len(), 32);
        assert_eq!(private.len(), 32);
        
        // Different each time
        let (public2, _) = generate_keypair();
        assert_ne!(public, public2);
    }

    #[test]
    fn test_sign_verify() {
        let (public, private) = generate_keypair();
        let message = b"hello grabnet";
        
        let signature = sign(message, &private);
        assert!(verify(message, &signature, &public));
        
        // Wrong message fails
        assert!(!verify(b"wrong message", &signature, &public));
        
        // Wrong key fails
        let (other_public, _) = generate_keypair();
        assert!(!verify(message, &signature, &other_public));
    }

    #[test]
    fn test_bundle_signature() {
        let (public, private) = generate_keypair();
        let site_id = [1u8; 32];
        let revision = 42u64;
        let root_hash = [2u8; 32];
        
        let signature = sign_bundle(&site_id, revision, &root_hash, &private);
        assert!(verify_bundle(&site_id, revision, &root_hash, &signature, &public));
        
        // Wrong revision fails
        assert!(!verify_bundle(&site_id, 43, &root_hash, &signature, &public));
    }
}
