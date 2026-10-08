//! Provider keys, as settings manage them: kept in the system's secure
//! storage (`kineto-keychain`), never in a project or a file, and read
//! before the environment's `KINETO_<PROVIDER>_API_KEY`.
//!
//! These calls belong to no project. Each opens the secure storage, which
//! can ask the user for permission or wait on a locked keyring, so Dart makes
//! them off the UI thread. A key crosses the boundary once, into
//! [`kineto_keys_store`], and is never handed back.

use std::{ptr, slice};

use crate::{contract::*, guarded, project_session::copy_utf8};
use kineto_keychain::{Keychain, KeychainError, KeychainFirst};
use kineto_providers::secret::{EnvSecretSource, SecretSource, SecretString};

/// Where a session's provider keys come from: the system's secure storage,
/// then the environment.
pub(crate) fn provider_keys() -> KeychainFirst<EnvSecretSource> {
    KeychainFirst {
        keychain: Keychain::open().ok(),
        fallback: EnvSecretSource::process(),
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

fn provider(code: u32) -> Option<&'static str> {
    KEY_PROVIDERS
        .iter()
        .find(|(_, known)| *known == code)
        .map(|(name, _)| *name)
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

/// Where provider `code`'s key is (a `KEY_PROVIDERS` code), as the
/// `KEY_STATES` bits the contract table publishes: in the secure storage,
/// in the environment, both or neither. A secure storage that refuses or
/// fails answers `keychain_failed`; none at all leaves its bit unset.
///
/// # Safety
/// `out_state` must point to writable storage for one `u32`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_keys_state(code: u32, out_state: *mut u32) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        let Some(provider) = provider(code).filter(|_| !out_state.is_null()) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let stored = match Keychain::open() {
            Ok(keychain) => match keychain.get(provider) {
                Ok(key) => key.is_some(),
                Err(error) => return keychain_error_code(&error),
            },
            Err(_) => false,
        };
        let environment = matches!(EnvSecretSource::process().api_key(provider), Ok(Some(_)));
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

/// Store the UTF-8 key at `key_ptr` as provider `code`'s, in place of any it
/// had. The bytes are read for this call only; the caller clears them.
/// Takes effect for projects opened after it.
///
/// # Safety
/// `key_ptr` must be readable for `key_len` bytes for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_keys_store(code: u32, key_ptr: *const u8, key_len: u64) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        let Some(provider) = provider(code) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
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
                .store(provider, &key)
                .map_err(|error| keychain_error_code(&error))
        }) {
            Ok(()) => PROJECT_OK,
            Err(code) => code,
        }
    })
}

/// Remove provider `code`'s key from the secure storage. Removing one that
/// is not there is not an error. Takes effect for projects opened after it.
#[unsafe(no_mangle)]
pub extern "C" fn kineto_keys_delete(code: u32) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        let Some(provider) = provider(code) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        match keychain().and_then(|keychain| {
            keychain
                .delete(provider)
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
    fn every_provider_the_engine_reads_has_a_code() {
        let named: Vec<&str> = KEY_PROVIDERS.iter().map(|(name, _)| *name).collect();
        assert_eq!(named, kineto_keychain::PROVIDERS);
        assert_eq!(provider(0), None);
        assert_eq!(provider(99), None);
    }

    #[test]
    fn what_the_calls_cannot_use_is_refused_before_the_storage_is_opened() {
        let mut state = 0;
        assert_eq!(
            unsafe { kineto_keys_state(99, &raw mut state) },
            PROJECT_ERR_INVALID_ARGUMENT
        );
        assert_eq!(
            unsafe { kineto_keys_state(1, ptr::null_mut()) },
            PROJECT_ERR_INVALID_ARGUMENT
        );
        let key = b"sk-test";
        assert_eq!(
            unsafe { kineto_keys_store(99, key.as_ptr(), 7) },
            PROJECT_ERR_INVALID_ARGUMENT
        );
        assert_eq!(
            unsafe { kineto_keys_store(1, key.as_ptr(), 0) },
            PROJECT_ERR_INVALID_ARGUMENT
        );
        assert_eq!(
            unsafe { kineto_keys_store(1, b"  ".as_ptr(), 2) },
            PROJECT_ERR_INVALID_ARGUMENT
        );
        assert_eq!(
            unsafe { kineto_keys_store(1, [0xff, 0xfe].as_ptr(), 2) },
            PROJECT_ERR_INVALID_UTF8
        );
        assert_eq!(kineto_keys_delete(99), PROJECT_ERR_INVALID_ARGUMENT);
    }
}
