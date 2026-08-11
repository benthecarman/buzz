//! Buzz paid Agent invocation terms.
//!
//! Pricing is advertised independently from BOLT12 offer kind `10058`.
//! A settled zap against the pricing event grants a short invocation window.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Current wire-format version.
pub const VERSION: u8 = 2;
/// Fixed period in which a settled zap can start Agent invocations.
pub const INVOCATION_WINDOW_SECONDS: u64 = 5 * 60;
/// Largest price that remains an exact JavaScript integer after msat conversion.
pub const MAX_INVOCATION_PRICE_SATS: u64 = 9_007_199_254_740;

/// Public, Agent-authored pricing terms carried by replaceable kind `10101`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimePricing {
    /// Schema version. Must equal [`VERSION`].
    pub version: u8,
    /// Whether paid Agent access is currently offered.
    pub enabled: bool,
    /// Whole satoshis charged for one invocation window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_sats: Option<u64>,
    /// Period after settlement in which the payer can start invocations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation_window_seconds: Option<u64>,
}

impl RuntimePricing {
    /// Construct enabled pricing with Buzz's fixed invocation window.
    pub fn enabled(price_sats: u64) -> Result<Self, RuntimePaymentError> {
        let pricing = Self {
            version: VERSION,
            enabled: true,
            price_sats: Some(price_sats),
            invocation_window_seconds: Some(INVOCATION_WINDOW_SECONDS),
        };
        pricing.validate()?;
        Ok(pricing)
    }

    /// Construct a disabled tombstone that supersedes stale pricing.
    pub fn disabled() -> Self {
        Self {
            version: VERSION,
            enabled: false,
            price_sats: None,
            invocation_window_seconds: None,
        }
    }

    /// Validate canonical pricing terms.
    pub fn validate(&self) -> Result<(), RuntimePaymentError> {
        if self.version != VERSION {
            return Err(RuntimePaymentError::UnsupportedVersion);
        }
        if self.enabled {
            let price = self.price_sats.ok_or(RuntimePaymentError::InvalidPricing(
                "enabled pricing requires a price",
            ))?;
            if price == 0 || price > MAX_INVOCATION_PRICE_SATS {
                return Err(RuntimePaymentError::InvalidPricing(
                    "invocation price is outside the supported whole-satoshi range",
                ));
            }
            if self.invocation_window_seconds != Some(INVOCATION_WINDOW_SECONDS) {
                return Err(RuntimePaymentError::InvalidPricing(
                    "invocation window must be exactly 300 seconds",
                ));
            }
        } else if self.price_sats.is_some() || self.invocation_window_seconds.is_some() {
            return Err(RuntimePaymentError::InvalidPricing(
                "disabled pricing must not carry a price or invocation window",
            ));
        }
        Ok(())
    }
}

/// Validation error for paid Agent pricing.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RuntimePaymentError {
    /// Unknown schema version.
    #[error("unsupported runtime-payment version")]
    UnsupportedVersion,
    /// Invalid public pricing terms.
    #[error("invalid runtime pricing: {0}")]
    InvalidPricing(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_pricing_uses_a_flat_five_minute_window() {
        let pricing = RuntimePricing::enabled(255).unwrap();
        assert_eq!(pricing.price_sats, Some(255));
        assert_eq!(
            pricing.invocation_window_seconds,
            Some(INVOCATION_WINDOW_SECONDS)
        );
        pricing.validate().unwrap();
    }

    #[test]
    fn disabled_pricing_has_no_payment_terms() {
        let pricing = RuntimePricing::disabled();
        assert_eq!(pricing.price_sats, None);
        assert_eq!(pricing.invocation_window_seconds, None);
        pricing.validate().unwrap();
    }

    #[test]
    fn enabled_pricing_rejects_noncanonical_terms() {
        assert!(RuntimePricing::enabled(0).is_err());
        assert!(RuntimePricing::enabled(MAX_INVOCATION_PRICE_SATS + 1).is_err());
        assert!(RuntimePricing {
            version: VERSION,
            enabled: true,
            price_sats: Some(255),
            invocation_window_seconds: Some(60),
        }
        .validate()
        .is_err());
    }
}
