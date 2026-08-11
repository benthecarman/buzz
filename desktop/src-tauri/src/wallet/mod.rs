pub(crate) mod conformance;
mod lexe_provider;
mod manager;
pub mod models;
pub(crate) mod offer_conformance;
pub(crate) mod provider;
mod seed;
pub(crate) mod send;
pub(crate) mod zap;

pub(crate) use manager::WalletManager;

#[cfg(test)]
pub(crate) const VALID_INVOICE: &str =
    "lnbc1gcssw9pdqqpp54dkfmzgm5cqz4hzz24mpl7xtgz55dsuh430ap4rlugvywlm4syhqsp5qqtk8n0x2wa6ajl32mp6hj8u9vs55s5lst4s2rws3he4622w08es9qyysgqcqypt3ffpp36sw424yacusmj3hy32df9g97nlwm0a3e0yxw4nd8uau2zdw85lfl5w0h3mggd5g3qswxr9lje0el8g98vul9yec59gf0zxu3eg9rhda09ducxpupsfh36ks9jez7aamsn7hpkxqpw2xyek";

#[cfg(test)]
pub(crate) const VALID_OFFER: &str =
    "lno1pgx9getnwss8vetrw3hhyuckyypwa3eyt44h6txtxquqh7lz5djge4afgfjn7k4rgrkuag0jsd5xvxg";

#[cfg(test)]
mod tests {
    #[test]
    fn lexe_sdk_types_stay_inside_the_adapter() {
        let wallet_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/wallet");
        let forbidden_path = ["lexe", "::"].concat();
        for entry in std::fs::read_dir(wallet_dir).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().and_then(|name| name.to_str()) == Some("lexe_provider.rs")
                || path.extension().and_then(|extension| extension.to_str()) != Some("rs")
            {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            // Lexe deliberately re-exports rust-lightning. Protocol parsing
            // may use that re-export directly without coupling wallet-domain
            // code to Lexe's provider-specific SDK types.
            let source = source.replace("lexe::lightning::", "lightning::");
            assert!(
                !source.contains(&forbidden_path),
                "Lexe SDK type leaked outside adapter: {}",
                path.display()
            );
        }
    }

    #[test]
    fn background_reconciliation_does_not_contact_the_wallet() {
        let source = include_str!("../commands/wallet/enabled/zap_commands.rs");
        let background_start = source
            .find("pub(crate) async fn reconcile_wallet_background_once")
            .expect("background reconciler exists");
        let background_end = source[background_start..]
            .find("async fn reconcile_paying_zap_attempts")
            .map(|offset| background_start + offset)
            .expect("background reconciler has a boundary");
        let background_source = &source[background_start..background_end];
        for forbidden in ["provider_for(", "provider.poll_updates("] {
            assert!(
                !background_source.contains(forbidden),
                "background zap sync contacted the wallet through {forbidden}"
            );
        }
    }
}
