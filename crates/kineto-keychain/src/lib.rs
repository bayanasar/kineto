//! Provider keys in the operating system's secure storage: the macOS
//! Keychain, the Windows Credential Manager, or a Secret Service (GNOME
//! Keyring, KWallet, KeePassXC) on Linux.
//!
//! Each key is one credential: service [`SERVICE`], account its name: a
//! provider's, as in `minimax` or `gemini`, a name the director gave the key
//! an OpenAI-compatible server takes, as in `deepseek`, or the name of a
//! secret a plugin's entry lists, as in `STUDIO_IMAGES_KEY`. Nothing is kept
//! anywhere else. Where
//! the system offers no secure storage, [`Keychain::open`] says why, and
//! Kineto reads keys from the environment only; it never falls back to a
//! file.
//!
//! A key read here is a [`SecretString`] at once; one stored here is taken
//! as one. Neither is printed, logged or kept beyond the call.

use std::{error::Error, fmt, sync::Arc};

use keyring_core::{CredentialStore, Entry};
use kineto_providers::secret::{SecretError, SecretSource, SecretString, api_key_variable};

/// The service every Kineto key is stored under.
pub const SERVICE: &str = "kineto";

/// The built-in providers whose keys Kineto reads, by the name each is
/// stored under.
pub const PROVIDERS: [&str; 4] = ["minimax", "gemini", "kimi", "veo"];

/// Whether `name` can name a key: 1 to 64 ASCII letters, digits and `_`,
/// not starting with a digit.
#[must_use]
pub fn secret_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with(|character: char| character.is_ascii_digit())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// Where a key named `name` is read from when the secure storage holds
/// none: a provider's (lowercase) from `KINETO_<NAME>_API_KEY`, a plugin's
/// secret from the variable of its own name.
#[must_use]
pub fn environment_variable(name: &str) -> Option<String> {
    if !secret_name(name) {
        return None;
    }
    api_key_variable(name)
        .ok()
        .or_else(|| Some(name.to_owned()))
}

/// Why a key could not be read, stored or removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeychainError {
    /// The system offers no secure storage Kineto can use, and why.
    Unavailable(String),
    /// The secure storage refused or failed, as when it is locked, and what
    /// it said.
    Failed(String),
    /// The name is not one a key can be stored under.
    InvalidProvider,
}

impl fmt::Display for KeychainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(reason) => {
                write!(formatter, "no secure storage is available: {reason}")
            }
            Self::Failed(reason) => write!(formatter, "the secure storage failed: {reason}"),
            Self::InvalidProvider => formatter.write_str("that is not a provider's name"),
        }
    }
}

impl Error for KeychainError {}

/// The system's secure storage.
#[derive(Clone)]
pub struct Keychain {
    store: Arc<CredentialStore>,
}

impl fmt::Debug for Keychain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Keychain")
            .field("store", &self.store.vendor())
            .finish()
    }
}

impl Keychain {
    /// The system's secure storage, or why there is none.
    pub fn open() -> Result<Self, KeychainError> {
        platform_store()
            .map(Self::with_store)
            .map_err(|error| KeychainError::Unavailable(reason(&error)))
    }

    /// Keys kept in `store`, as tests keep them in memory.
    #[must_use]
    pub fn with_store(store: Arc<CredentialStore>) -> Self {
        Self { store }
    }

    /// `provider`'s key, or `None` when none is stored.
    pub fn get(&self, provider: &str) -> Result<Option<SecretString>, KeychainError> {
        match self.entry(provider)?.get_password() {
            Ok(key) => Ok(SecretString::new(key)),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(error) => Err(failure(&error)),
        }
    }

    /// Store `key` as `provider`'s, in place of any it had.
    pub fn store(&self, provider: &str, key: &SecretString) -> Result<(), KeychainError> {
        self.entry(provider)?
            .set_password(key.expose())
            .map_err(|error| failure(&error))
    }

    /// Remove `provider`'s key. Whether it had one.
    pub fn delete(&self, provider: &str) -> Result<bool, KeychainError> {
        match self.entry(provider)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring_core::Error::NoEntry) => Ok(false),
            Err(error) => Err(failure(&error)),
        }
    }

    fn entry(&self, name: &str) -> Result<Entry, KeychainError> {
        if !secret_name(name) {
            return Err(KeychainError::InvalidProvider);
        }
        self.store
            .build(SERVICE, name, None)
            .map_err(|error| failure(&error))
    }
}

impl SecretSource for Keychain {
    fn api_key(&self, provider: &str) -> Result<Option<SecretString>, SecretError> {
        self.get(provider).map_err(|error| match error {
            KeychainError::InvalidProvider => SecretError::InvalidProviderName,
            KeychainError::Unavailable(reason) | KeychainError::Failed(reason) => {
                SecretError::Storage(reason)
            }
        })
    }
}

/// Keys from the system's secure storage, then from `fallback` (the
/// environment, for development) for a provider it holds none for, or when
/// it cannot be read. Its own failures are reported where keys are managed,
/// not by refusing to open a project.
#[derive(Debug)]
pub struct KeychainFirst<F> {
    pub keychain: Option<Keychain>,
    pub fallback: F,
}

impl<F> KeychainFirst<F> {
    /// The secret `name` a plugin's entry lists: from the secure storage,
    /// else from the environment variable of its own name.
    #[must_use]
    pub fn secret(&self, name: &str) -> Option<String> {
        if !secret_name(name) {
            return None;
        }
        if let Some(Ok(Some(key))) = self.keychain.as_ref().map(|keychain| keychain.get(name)) {
            return Some(key.expose().to_owned());
        }
        std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
    }
}

impl<F: SecretSource> SecretSource for KeychainFirst<F> {
    fn api_key(&self, provider: &str) -> Result<Option<SecretString>, SecretError> {
        if let Some(Ok(Some(key))) = self
            .keychain
            .as_ref()
            .map(|keychain| keychain.get(provider))
        {
            return Ok(Some(key));
        }
        self.fallback.api_key(provider)
    }
}

fn failure(error: &keyring_core::Error) -> KeychainError {
    KeychainError::Failed(reason(error))
}

/// What the storage said, without anything it held: a secret it returned
/// with an error is never repeated.
fn reason(error: &keyring_core::Error) -> String {
    use keyring_core::Error as E;
    match error {
        E::PlatformFailure(cause) => format!("the platform failed ({cause})"),
        E::NoStorageAccess(cause) => format!("access was refused ({cause})"),
        E::NoEntry => "no key is stored".to_owned(),
        E::BadEncoding(_) | E::BadDataFormat(..) => "a stored key is unreadable".to_owned(),
        E::BadStoreFormat(what) => format!("the store is malformed ({what})"),
        E::TooLong(what, limit) => format!("{what} is longer than {limit}"),
        E::Invalid(what, why) => format!("{what} is invalid ({why})"),
        E::Ambiguous(_) => "more than one key matches".to_owned(),
        E::NoDefaultStore => "no store is configured".to_owned(),
        E::NotSupportedByStore(what) => format!("the store cannot do that ({what})"),
        _ => "an unknown error".to_owned(),
    }
}

#[cfg(target_os = "macos")]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(apple_native_keyring_store::keychain::Store::new()?)
}

#[cfg(target_os = "windows")]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(windows_native_keyring_store::Store::new()?)
}

#[cfg(target_os = "linux")]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(zbus_secret_service_keyring_store::Store::new()?)
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Err(keyring_core::Error::NotSupportedByStore(
        "this system has no secure storage Kineto supports".to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use kineto_providers::secret::EnvSecretSource;

    use super::*;

    fn memory() -> Keychain {
        Keychain::with_store(keyring_core::mock::Store::new().unwrap())
    }

    fn key(text: &str) -> SecretString {
        SecretString::new(text.to_owned()).unwrap()
    }

    #[test]
    fn a_key_is_stored_read_replaced_and_removed_by_its_provider() {
        let keychain = memory();
        assert_eq!(keychain.get("minimax").unwrap(), None);
        keychain.store("minimax", &key("sk-one")).unwrap();
        assert_eq!(keychain.get("minimax").unwrap(), Some(key("sk-one")));
        keychain.store("minimax", &key("sk-two")).unwrap();
        assert_eq!(keychain.get("minimax").unwrap(), Some(key("sk-two")));
        assert_eq!(keychain.get("gemini").unwrap(), None);
        assert!(keychain.delete("minimax").unwrap());
        assert!(!keychain.delete("minimax").unwrap());
        assert_eq!(keychain.get("minimax").unwrap(), None);
        for name in ["", "1key", "../kineto", "a b", "sk-live"] {
            assert_eq!(keychain.get(name), Err(KeychainError::InvalidProvider));
        }
        assert!(!format!("{keychain:?}").contains("sk-"));
    }

    #[test]
    fn any_named_key_is_kept_and_read_from_its_own_variable_when_absent() {
        let keychain = memory();
        keychain.store("deepseek", &key("sk-endpoint")).unwrap();
        keychain
            .store("STUDIO_IMAGES_KEY", &key("sk-plugin"))
            .unwrap();
        assert_eq!(keychain.get("deepseek").unwrap(), Some(key("sk-endpoint")));
        let keys = KeychainFirst {
            keychain: Some(keychain),
            fallback: EnvSecretSource::with_lookup(|_: &str| None),
        };
        assert_eq!(
            keys.secret("STUDIO_IMAGES_KEY").as_deref(),
            Some("sk-plugin")
        );
        assert_eq!(keys.secret("../x"), None);
        assert_eq!(
            environment_variable("deepseek").as_deref(),
            Some("KINETO_DEEPSEEK_API_KEY")
        );
        assert_eq!(
            environment_variable("STUDIO_IMAGES_KEY").as_deref(),
            Some("STUDIO_IMAGES_KEY")
        );
        assert_eq!(environment_variable("a b"), None);
    }

    #[test]
    fn the_keychain_comes_first_and_the_environment_after_it() {
        let keychain = memory();
        keychain.store("minimax", &key("sk-keychain")).unwrap();
        let environment = EnvSecretSource::with_lookup(|name: &str| match name {
            "KINETO_MINIMAX_API_KEY" => Some(OsString::from("sk-env")),
            "KINETO_GEMINI_API_KEY" => Some(OsString::from("AQ.env")),
            _ => None,
        });
        let keys = KeychainFirst {
            keychain: Some(keychain),
            fallback: environment,
        };
        assert_eq!(keys.api_key("minimax").unwrap(), Some(key("sk-keychain")));
        assert_eq!(keys.api_key("gemini").unwrap(), Some(key("AQ.env")));
        assert_eq!(keys.api_key("kimi").unwrap(), None);

        // Without secure storage, only the environment.
        let keys = KeychainFirst {
            keychain: None,
            fallback: EnvSecretSource::with_lookup(|_: &str| Some(OsString::from("sk-env"))),
        };
        assert_eq!(keys.api_key("minimax").unwrap(), Some(key("sk-env")));
    }

    #[test]
    fn a_storage_that_fails_says_why_and_the_environment_still_answers() {
        let keychain = memory();
        keychain.store("minimax", &key("sk-keychain")).unwrap();
        let entry = keychain.entry("minimax").unwrap();
        let mock: &keyring_core::mock::Cred = entry.as_any().downcast_ref().unwrap();
        mock.set_error(keyring_core::Error::NoStorageAccess(Box::new(
            std::io::Error::other("the keyring is locked"),
        )));
        let keys = KeychainFirst {
            keychain: Some(keychain.clone()),
            fallback: EnvSecretSource::with_lookup(|_: &str| Some(OsString::from("sk-env"))),
        };
        // The mock fails the next call on that credential only.
        assert_eq!(keys.api_key("minimax").unwrap(), Some(key("sk-env")));
        assert_eq!(keychain.get("minimax").unwrap(), Some(key("sk-keychain")));
        let failed = failure(&keyring_core::Error::BadEncoding(b"sk-secret".to_vec()));
        assert_eq!(
            failed,
            KeychainError::Failed("a stored key is unreadable".to_owned())
        );
    }
}
