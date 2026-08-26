//! Signed payer-proof fixtures generated from public LDK APIs.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lightning_payer_proof_ldk::bitcoin::hashes::{sha256, Hash};
use lightning_payer_proof_ldk::bitcoin::secp256k1::{Keypair, PublicKey, Secp256k1, SecretKey};
use lightning_payer_proof_ldk::blinded_path::payment::{BlindedPayInfo, BlindedPaymentPath};
use lightning_payer_proof_ldk::blinded_path::BlindedHop;
use lightning_payer_proof_ldk::offers::invoice::UnsignedBolt12Invoice;
use lightning_payer_proof_ldk::offers::offer::OfferBuilder;
use lightning_payer_proof_ldk::offers::payer_proof::{PaidBolt12Invoice, UnsignedPayerProof};
use lightning_payer_proof_ldk::offers::refund::RefundBuilder;
use lightning_payer_proof_ldk::types::features::BlindedHopFeatures;
use lightning_payer_proof_ldk::types::payment::{PaymentHash, PaymentPreimage};

/// Millisatoshi amount used by [`payer_proof_for_note`].
pub const TEST_PAYMENT_MSATS: u64 = 42_000;

fn test_pubkey(byte: u8) -> PublicKey {
    let secp = Secp256k1::new();
    PublicKey::from_secret_key(
        &secp,
        &SecretKey::from_slice(&[byte; 32]).expect("valid path test key"),
    )
}

fn payment_path() -> BlindedPaymentPath {
    BlindedPaymentPath::from_blinded_path_and_payinfo(
        test_pubkey(40),
        test_pubkey(41),
        vec![BlindedHop {
            blinded_node_id: test_pubkey(43),
            encrypted_payload: vec![0; 43],
        }],
        BlindedPayInfo {
            fee_base_msat: 1,
            fee_proportional_millionths: 1_000,
            cltv_expiry_delta: 42,
            htlc_minimum_msat: 100,
            htlc_maximum_msat: 1_000_000_000_000,
            features: BlindedHopFeatures::empty(),
        },
    )
}

/// Create a canonical offer and valid payer proof with the specified signed note.
pub fn payer_proof_for_note(note: &str) -> (String, String, u64) {
    let secp = Secp256k1::new();
    let payer_keys = Keypair::from_secret_key(
        &secp,
        &SecretKey::from_slice(&[42; 32]).expect("valid payer test key"),
    );
    let recipient_keys = Keypair::from_secret_key(
        &secp,
        &SecretKey::from_slice(&[43; 32]).expect("valid recipient test key"),
    );
    let preimage = PaymentPreimage([44; 32]);
    let payment_hash = PaymentHash(sha256::Hash::hash(&preimage.0).to_byte_array());
    let invoice_created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("test clock after epoch")
        .as_secs();
    let invoice = RefundBuilder::new(vec![1; 32], payer_keys.public_key(), TEST_PAYMENT_MSATS)
        .expect("valid refund")
        .build()
        .expect("build refund")
        .respond_with_no_std(
            vec![payment_path()],
            payment_hash,
            recipient_keys.public_key(),
            Duration::from_secs(invoice_created_at),
        )
        .expect("build invoice response")
        .build()
        .expect("build invoice")
        .sign(|invoice: &UnsignedBolt12Invoice| {
            Ok(secp.sign_schnorr_no_aux_rand(invoice.as_ref().as_digest(), &recipient_keys))
        })
        .expect("sign invoice");
    let proof = PaidBolt12Invoice::Bolt12Invoice(invoice)
        .prove_payer(preimage)
        .expect("build payer proof")
        .include_invoice_amount()
        .include_invoice_created_at()
        .with_proof_note(note.to_string())
        .build()
        .expect("build unsigned payer proof")
        .sign(|proof: &UnsignedPayerProof| {
            Ok(secp.sign_schnorr_no_aux_rand(proof.as_ref().as_digest(), &payer_keys))
        })
        .expect("sign payer proof");
    let offer = OfferBuilder::new(recipient_keys.public_key())
        .build()
        .expect("build offer")
        .to_string();
    (offer, proof.to_string(), invoice_created_at)
}
