//! Keys, as settings manage them: kept in the system's secure storage
//! (`kineto-keychain`) by name, never in a project or a file, and read
//! before the environment. A key is named for its provider (`minimax`,
//! read from `KINETO_MINIMAX_API_KEY` when the storage holds none), for the
//! OpenAI-compatible server the director named it for (`deepseek`), or as a
//! plugin's entry lists it (`STUDIO_IMAGES_KEY`, read from the variable of
//! that name).
//!
//! These calls belong to no project. Each opens the secure storage, which
//! can ask the user for permission or wait on a locked keyring, so Dart makes
//! them off the UI thread. A key crosses the boundary once, into
//! [`kineto_keys_store`], and is never handed back.

use std::{ptr, slice, sync::Once};

use crate::{
    contract::*,
    guarded,
    project_session::{copy_utf8, utf8_owned},
};
use kineto_keychain::{Keychain, KeychainError, KeychainFirst, environment_variable, secret_name};
use kineto_providers::secret::{EnvSecretSource, SecretString};

/// Where a session's provider keys come from: the system's secure storage,
/// then the environment. A session calls this on its worker, never on the
/// thread that opens it, since opening and reading the storage can wait.
pub(crate) fn provider_keys() -> KeychainFirst<EnvSecretSource> {
    read_plugin_secrets_from_storage();
    KeychainFirst {
        keychain: Keychain::open().ok(),
        fallback: EnvSecretSource::process(),
    }
}

/// Have plugins read the secrets their entries list from the secure
/// storage first, then the environment. Done once, before any plugin is
/// started: by a session choosing its providers, or by settings listing
/// the plugins.
pub(crate) fn read_plugin_secrets_from_storage() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        kineto_core::plugin::read_secrets_with(Box::new(|name| {
            KeychainFirst {
                keychain: Keychain::open().ok(),
                fallback: EnvSecretSource::process(),
            }
            .secret(name)
        }));
    });
}

/// The key name a call gives, as UTF-8.
///
/// # Safety
/// `name_ptr` must be readable for `name_len` bytes when `name_len` is not 0.
unsafe fn key_name(name_ptr: *const u8, name_len: u64) -> Result<String, i32> {
    // SAFETY: forwarded caller contract.
    let name = unsafe { utf8_owned(name_ptr, name_len) }?;
    if secret_name(&name) {
        Ok(name)
    } else {
        Err(PROJECT_ERR_INVALID_ARGUMENT)
    }
}

fn keychain() -> Result<Keychain, i32> {
    Keychain::open().map_err(|error| keychain_error_code(&error))
}

fn keychain_error_code(error: &KeychainError) -> i32 {
    match error {
        KeychainError::Unavailable(_) => PROJECT_ERR_KEYCHAIN_UNAVAILABLE,
        KeychainError::Failed(_) => PROJECT_ERR_KEYCHAIN_FAILED,
        KeychainError::InvalidProvider => PROJECT_ERR_INVALID_ARGUMENT,
    }
}

/// Why there is no secure storage, copied as UTF-8 into the caller's buffer;
/// empty when there is one.
///
/// # Safety
/// `out_len` must point to writable storage for one `u64`, and `out_buf` to
/// `out_cap` writable bytes when `out_cap` is not 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_keys_unavailable_copy(
    out_buf: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        let reason = match Keychain::open() {
            Ok(_) => String::new(),
            Err(error) => error.to_string(),
        };
        // SAFETY: forwarded caller-owned output contract.
        unsafe { copy_utf8(reason.as_bytes(), out_buf, out_cap, out_len) }
    })
}

/// Where the key named by the UTF-8 `name` is, as the `KEY_STATES` bits the
/// contract table publishes: in the secure storage, in the environment,
/// both or neither. A secure storage that refuses or fails answers
/// `keychain_failed`; none at all leaves its bit unset.
///
/// # Safety
/// `name_ptr` must be readable for `name_len` bytes when `name_len` is not
/// 0; `out_state` must point to writable storage for one `u32`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_keys_state(
    name_ptr: *const u8,
    name_len: u64,
    out_state: *mut u32,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_state.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: forwarded caller contract.
        let name = match unsafe { key_name(name_ptr, name_len) } {
            Ok(name) => name,
            Err(code) => return code,
        };
        let stored = match Keychain::open() {
            Ok(keychain) => match keychain.get(&name) {
                Ok(key) => key.is_some(),
                Err(error) => return keychain_error_code(&error),
            },
            Err(_) => false,
        };
        let environment = environment_variable(&name)
            .and_then(|variable| std::env::var(variable).ok())
            .is_some_and(|value| !value.trim().is_empty());
        let state = (if stored { KEY_STATE_STORED } else { 0 })
            | (if environment {
                KEY_STATE_ENVIRONMENT
            } else {
                0
            });
        // SAFETY: checked non-null; the caller guarantees it is writable.
        unsafe { ptr::write(out_state, state) };
        PROJECT_OK
    })
}

/// Store the UTF-8 key at `key_ptr` under the UTF-8 `name`, in place of
/// any it had. The key's bytes are read for this call only; the caller
/// clears them. Takes effect for projects opened after it.
///
/// # Safety
/// `name_ptr` must be readable for `name_len` bytes, and `key_ptr` for
/// `key_len` bytes, for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_keys_store(
    name_ptr: *const u8,
    name_len: u64,
    key_ptr: *const u8,
    key_len: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let name = match unsafe { key_name(name_ptr, name_len) } {
            Ok(name) => name,
            Err(code) => return code,
        };
        let Ok(len) = usize::try_from(key_len) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        if key_ptr.is_null() || len == 0 {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: the caller guarantees `key_len` readable bytes.
        let bytes = unsafe { slice::from_raw_parts(key_ptr, len) };
        let Ok(text) = std::str::from_utf8(bytes) else {
            return PROJECT_ERR_INVALID_UTF8;
        };
        let Some(key) = SecretString::new(text.trim().to_owned()) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        match keychain().and_then(|keychain| {
            keychain
                .store(&name, &key)
                .map_err(|error| keychain_error_code(&error))
        }) {
            Ok(()) => PROJECT_OK,
            Err(code) => code,
        }
    })
}

/// Remove the key named by the UTF-8 `name` from the secure storage.
/// Removing one that is not there is not an error. Takes effect for projects
/// opened after it.
///
/// # Safety
/// `name_ptr` must be readable for `name_len` bytes when `name_len` is not 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_keys_delete(name_ptr: *const u8, name_len: u64) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let name = match unsafe { key_name(name_ptr, name_len) } {
            Ok(name) => name,
            Err(code) => return code,
        };
        match keychain().and_then(|keychain| {
            keychain
                .delete(&name)
                .map_err(|error| keychain_error_code(&error))
        }) {
            Ok(_) => PROJECT_OK,
            Err(code) => code,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_provider_is_listed_by_the_name_it_is_stored_under() {
        let named: Vec<&str> = KEY_PROVIDERS.iter().map(|(name, _)| *name).collect();
        assert_eq!(named, kineto_keychain::PROVIDERS);
    }

    #[test]
    fn what_the_calls_cannot_use_is_refused_before_the_storage_is_opened() {
        let mut state = 0;
        let bad = b"../x";
        assert_eq!(
            unsafe { kineto_keys_state(bad.as_ptr(), 4, &raw mut state) },
            PROJECT_ERR_INVALID_ARGUMENT
        );
        assert_eq!(
            unsafe { kineto_keys_state(b"minimax".as_ptr(), 7, ptr::null_mut()) },
            PROJECT_ERR_INVALID_ARGUMENT
        );
        let key = b"sk-test";
        assert_eq!(
            unsafe { kineto_keys_store(bad.as_ptr(), 4, key.as_ptr(), 7) },
            PROJECT_ERR_INVALID_ARGUMENT
        );
        assert_eq!(
            unsafe { kineto_keys_store(b"minimax".as_ptr(), 7, key.as_ptr(), 0) },
            PROJECT_ERR_INVALID_ARGUMENT
        );
        assert_eq!(
            unsafe { kineto_keys_store(b"minimax".as_ptr(), 7, b"  ".as_ptr(), 2) },
            PROJECT_ERR_INVALID_ARGUMENT
        );
        assert_eq!(
            unsafe { kineto_keys_store(b"minimax".as_ptr(), 7, [0xff, 0xfe].as_ptr(), 2) },
            PROJECT_ERR_INVALID_UTF8
        );
        assert_eq!(
            unsafe { kineto_keys_delete(b"".as_ptr(), 0) },
            PROJECT_ERR_INVALID_ARGUMENT
        );
    }
}
