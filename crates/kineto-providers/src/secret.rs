//! Provider credentials.
//!
//! A credential exists in Rust memory only as a [`SecretString`], which cannot
//! be printed, logged or serialized; the plaintext is taken out with
//! [`SecretString::expose`] only where a request header is built. Credentials
//! are never read from or written to a project directory.
//!
//! [`EnvSecretSource`] is the development source: it reads
//! `KINETO_<PROVIDER>_API_KEY` from the process environment. Desktop secure
//! storage is a separate source behind the same [`SecretSource`] interface.

use std::{error::Error, ffi::OsString, fmt};

/// A credential whose text never appears in `Debug` or `Display` output.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(Box<str>);

impl SecretString {
    /// Wrap a credential. Empty or whitespace-only text is not a credential.
    #[must_use]
    pub fn new(value: String) -> Option<Self> {
        if value.trim().is_empty() {
            None
        } else {
            Some(Self(value.into_boxed_str()))
        }
    }

    /// The plaintext. Call only to build the request that carries it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString([redacted])")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[redacted]")
    }
}

/// A provider name that can be turned into a credential lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidProviderName;

impl fmt::Display for InvalidProviderName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("provider name must be 1..=64 lowercase ASCII letters, digits or '_'")
    }
}

impl Error for InvalidProviderName {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretError {
    InvalidProviderName,
    /// The variable is set but is not valid Unicode. Its value is not kept.
    NotUnicode {
        variable: String,
    },
}

impl fmt::Display for SecretError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidProviderName => InvalidProviderName.fmt(formatter),
            Self::NotUnicode { variable } => {
                write!(formatter, "{variable} is set but is not valid Unicode")
            }
        }
    }
}

impl Error for SecretError {}

/// Where provider credentials come from.
pub trait SecretSource {
    /// The provider's API key, or `None` when none is configured.
    fn api_key(&self, provider: &str) -> Result<Option<SecretString>, SecretError>;
}

/// The environment variable that holds `provider`'s API key, for example
/// `KINETO_MINIMAX_API_KEY` for `minimax`.
pub fn api_key_variable(provider: &str) -> Result<String, InvalidProviderName> {
    if provider.is_empty()
        || provider.len() > 64
        || !provider
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(InvalidProviderName);
    }
    Ok(format!("KINETO_{}_API_KEY", provider.to_ascii_uppercase()))
}

/// Development credentials from environment variables.
pub struct EnvSecretSource<L = fn(&str) -> Option<OsString>> {
    lookup: L,
}

impl EnvSecretSource {
    /// Read from this process's environment.
    #[must_use]
    pub fn process() -> Self {
        Self {
            lookup: |name| std::env::var_os(name),
        }
    }
}

impl<L: Fn(&str) -> Option<OsString>> EnvSecretSource<L> {
    /// Read through `lookup` instead of the process environment.
    pub const fn with_lookup(lookup: L) -> Self {
        Self { lookup }
    }
}

impl<L: Fn(&str) -> Option<OsString>> SecretSource for EnvSecretSource<L> {
    fn api_key(&self, provider: &str) -> Result<Option<SecretString>, SecretError> {
        let variable = api_key_variable(provider).map_err(|_| SecretError::InvalidProviderName)?;
        match (self.lookup)(&variable) {
            None => Ok(None),
            Some(value) => value
                .into_string()
                .map(SecretString::new)
                .map_err(|_| SecretError::NotUnicode { variable }),
        }
    }
}

impl<L> fmt::Debug for EnvSecretSource<L> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EnvSecretSource")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "sk-test-0123456789abcdef";

    fn source(value: Option<&'static str>) -> EnvSecretSource<impl Fn(&str) -> Option<OsString>> {
        EnvSecretSource::with_lookup(move |name| {
            assert_eq!(name, "KINETO_MINIMAX_API_KEY");
            value.map(OsString::from)
        })
    }

    #[test]
    fn a_secret_never_formats_its_text() {
        let secret = SecretString::new(KEY.to_owned()).unwrap();
        assert_eq!(secret.expose(), KEY);
        for rendered in [
            format!("{secret}"),
            format!("{secret:?}"),
            format!("{secret:#?}"),
            format!("{:?}", Some(&secret)),
            format!("{:?}", source(Some(KEY)).api_key("minimax")),
        ] {
            assert!(!rendered.contains(KEY), "{rendered}");
            assert!(!rendered.contains("0123456789"), "{rendered}");
        }
    }

    #[test]
    fn missing_and_blank_keys_are_not_configured() {
        assert_eq!(source(None).api_key("minimax").unwrap(), None);
        assert_eq!(source(Some("")).api_key("minimax").unwrap(), None);
        assert_eq!(source(Some("  \n")).api_key("minimax").unwrap(), None);
        assert_eq!(
            source(Some(KEY))
                .api_key("minimax")
                .unwrap()
                .unwrap()
                .expose(),
            KEY
        );
    }

    #[test]
    fn provider_names_map_to_one_variable_each() {
        assert_eq!(
            api_key_variable("minimax").unwrap(),
            "KINETO_MINIMAX_API_KEY"
        );
        assert_eq!(
            api_key_variable("open_ai2").unwrap(),
            "KINETO_OPEN_AI2_API_KEY"
        );
        for invalid in ["", "MiniMax", "mini-max", "a.b", &"x".repeat(65)] {
            assert_eq!(api_key_variable(invalid), Err(InvalidProviderName));
            assert_eq!(
                EnvSecretSource::with_lookup(|_| unreachable!("never looked up")).api_key(invalid),
                Err(SecretError::InvalidProviderName)
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_non_unicode_value_is_reported_without_its_bytes() {
        use std::os::unix::ffi::OsStringExt;
        let source = EnvSecretSource::with_lookup(|_| Some(OsString::from_vec(vec![0x73, 0xff])));
        let error = source.api_key("minimax").unwrap_err();
        assert_eq!(
            error.to_string(),
            "KINETO_MINIMAX_API_KEY is set but is not valid Unicode"
        );
    }
}
