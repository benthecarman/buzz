mod lexe_provider;
mod manager;
pub mod models;
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
            assert!(
                !source.contains(&forbidden_path),
                "Lexe SDK type leaked outside adapter: {}",
                path.display()
            );
        }
    }
}
