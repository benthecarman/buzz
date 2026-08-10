use super::RespondTo;
use buzz_core_pkg::agent_runtime_payment::MAX_RUNTIME_RATE_SATS_PER_MINUTE;

/// Validate the paid-runtime configuration attached to one live instance.
pub fn validate_runtime_price(
    respond_to: RespondTo,
    allowlist: &[String],
    price_per_minute_sats: Option<u64>,
) -> Result<Option<u64>, String> {
    let Some(price) = price_per_minute_sats else {
        return Ok(None);
    };
    if price == 0 {
        return Err("runtime price must be greater than zero satoshis per minute".into());
    }
    if price > MAX_RUNTIME_RATE_SATS_PER_MINUTE {
        return Err(format!(
            "runtime price must not exceed {MAX_RUNTIME_RATE_SATS_PER_MINUTE} satoshis per minute"
        ));
    }
    match respond_to {
        RespondTo::Allowlist if allowlist.is_empty() => {
            return Err(
                "runtime pricing in respond-to mode 'allowlist' requires at least one pubkey"
                    .into(),
            );
        }
        RespondTo::Allowlist | RespondTo::Anyone => {}
        RespondTo::OwnerOnly => {
            return Err("runtime pricing requires respond-to mode 'allowlist' or 'anyone'".into());
        }
    }
    Ok(Some(price))
}
