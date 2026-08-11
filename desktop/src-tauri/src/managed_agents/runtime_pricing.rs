use super::RespondTo;
use buzz_core_pkg::agent_runtime_payment::MAX_INVOCATION_PRICE_SATS;

/// Validate the paid-runtime configuration attached to one live instance.
pub fn validate_runtime_price(
    respond_to: RespondTo,
    allowlist: &[String],
    price_sats: Option<u64>,
) -> Result<Option<u64>, String> {
    let Some(price) = price_sats else {
        return Ok(None);
    };
    if price == 0 {
        return Err("Agent access price must be greater than zero satoshis".into());
    }
    if price > MAX_INVOCATION_PRICE_SATS {
        return Err(format!(
            "Agent access price must not exceed {MAX_INVOCATION_PRICE_SATS} satoshis"
        ));
    }
    match respond_to {
        RespondTo::Allowlist if allowlist.is_empty() => {
            return Err(
                "Agent access pricing in respond-to mode 'allowlist' requires at least one pubkey"
                    .into(),
            );
        }
        RespondTo::Allowlist | RespondTo::Anyone => {}
        RespondTo::OwnerOnly => {
            return Err(
                "Agent access pricing requires respond-to mode 'allowlist' or 'anyone'".into(),
            );
        }
    }
    Ok(Some(price))
}
