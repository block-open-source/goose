// tests/curves/p256_tests.rs

use super::super::curves::p256::P256Curve;
use p256::ecdsa::Signature;
use p256::EncodedPoint;
use hex::decode;
use std::str::FromStr;

#[test]
fn test_key_generation() {
    let curve = P256Curve::new();
    let public_key = curve.get_public_key();

    // Check that the public key is a valid encoded point
    assert!(EncodedPoint::from_bytes(public_key.as_bytes()).is_ok());

    // Ensure the public key is in compressed form (if using compressed encoding)
    assert_eq!(public_key.len(), 33);
}

#[test]
fn test_sign_and_verify() {
    let curve = P256Curve::new();
    let message = b"Test message for P256 signing";

    let signature: Signature = curve.sign(message);
    assert!(curve.verify(message, &signature));
}

#[test]
fn test_invalid_signature() {
    let curve = P256Curve::new();
    let message = b"Original message";
    let tampered_message = b"Tampered message";

    let signature: Signature = curve.sign(message);
    assert!(!curve.verify(tampered_message, &signature));
}

#[test]
fn test_signature_verification_with_different_key() {
    let curve1 = P256Curve::new();
    let curve2 = P256Curve::new();
    let message = b"Test message for P256 signing";

    let signature: Signature = curve1.sign(message);
    assert!(!curve2.verify(message, &signature));
}

#[test]
fn test_deterministic_signing() {
    // P256 ECDSA can use deterministic signatures as per RFC 6979
    // Ensure that signing the same message with the same key produces the same signature
    let mut rng = p256::ecdsa::SigningKey::random(&mut rand_core::OsRng);
    let message = b"Deterministic signing test message";

    let signature1 = rng.sign(message);
    let signature2 = rng.sign(message);

    assert_eq!(signature1.as_ref(), signature2.as_ref());
}

#[test]
fn test_public_key_serialization() {
    let curve = P256Curve::new();
    let public_key = curve.get_public_key();

    // Serialize to DER format
    let der = public_key.to_der();
    assert!(der.is_ok());

    // Deserialize back
    let decoded = EncodedPoint::from_der(der.unwrap().as_bytes());
    assert!(decoded.is_ok());
    let decoded = decoded.unwrap();

    // Check that the decoded key matches the original
    assert_eq!(decoded, public_key);
}

#[test]
fn test_invalid_public_key() {
    // Attempt to create a public key from invalid bytes
    let invalid_bytes = [0u8; 33];
    let result = EncodedPoint::from_bytes(&invalid_bytes);
    assert!(result.is_err());
}

#[test]
fn test_known_signatures() {
    // Test against known signatures and keys
    // Example using test vectors (Replace with real test vectors)

    let signing_key = p256::ecdsa::SigningKey::from_bytes(&decode("c9af...").unwrap()).unwrap();
    let verifying_key = signing_key.verifying_key();
    let message = b"Known message";

    let signature = signing_key.sign(message);
    let expected_signature = Signature::from_der(&decode("3045...").unwrap()).unwrap();

    assert_eq!(signature.as_ref(), expected_signature.as_ref());
    assert!(verifying_key.verify(message, &signature).is_ok());
}

#[test]
fn test_large_message() {
    let curve = P256Curve::new();
    // Generate a large message (e.g., 1MB)
    let message = vec![0u8; 1_000_000];
    let signature = curve.sign(&message);
    assert!(curve.verify(&message, &signature));
}

#[test]
fn test_empty_message() {
    let curve = P256Curve::new();
    let message = b"";
    let signature = curve.sign(message);
    assert!(curve.verify(message, &signature));
}

#[test]
fn test_multiple_signatures() {
    let curve = P256Curve::new();
    let messages = vec![
        b"First message",
        b"Second message",
        b"Third message",
        b"Fourth message",
        b"Fifth message",
    ];

    for message in messages {
        let signature = curve.sign(message);
        assert!(curve.verify(message, &signature));
    }
}
