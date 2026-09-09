//! Nonce-bound mutual authentication over the standard HTTP transport.
//! The private key is never transmitted, including to a stale/rebound port.
use anyhow::{Result, ensure};
use hmac::{Hmac, Mac};
use sha2::Sha256;

pub(super) fn timestamp() -> Result<u64> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs())
}
fn mac(key: &str, domain: &str, nonce: &str, metadata: u64, body: &[u8]) -> Hmac<Sha256> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("HMAC accepts any key length");
    // Length framing prevents concatenation ambiguities; distinct domains prevent reflection.
    for part in [
        b"asp-resident-http-v1".as_slice(),
        domain.as_bytes(),
        nonce.as_bytes(),
        &metadata.to_be_bytes(),
        body,
    ] {
        mac.update(&(part.len() as u64).to_be_bytes());
        mac.update(part);
    }
    mac
}
pub(super) fn sign(key: &str, domain: &str, nonce: &str, metadata: u64, body: &[u8]) -> String {
    mac(key, domain, nonce, metadata, body)
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
pub(super) fn verify(
    key: &str,
    domain: &str,
    nonce: &str,
    metadata: u64,
    body: &[u8],
    signature: &str,
) -> Result<()> {
    ensure!(
        signature.len() == 64 && signature.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid host authentication signature"
    );
    let bytes = (0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(&signature[i..i + 2], 16))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    mac(key, domain, nonce, metadata, body)
        .verify_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("host authentication failed"))
}
