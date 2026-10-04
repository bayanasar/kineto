use kineto_core::Engine;

mod contract;
mod documents;
mod handoff;
mod planning;
mod plugins;
mod production;
mod project_session;
mod voices;

const KINETO_NATIVE_ABI_VERSION: u32 = 41;

/// Stop a Rust panic at the C ABI.
///
/// Every exported entry point runs inside this guard. A caught panic is
/// reported best-effort to stderr and converted to the caller-provided fallback
/// so unwinding never crosses into Dart. Logging must itself never panic.
pub(crate) fn guarded<T>(fallback: T, operation: impl FnOnce() -> T) -> T {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)) {
        Ok(value) => value,
        Err(_) => {
            use std::io::Write as _;
            let _ = std::io::stderr()
                .lock()
                .write_all(b"Kineto native panic caught at FFI boundary\n");
            fallback
        }
    }
}

/// Opaque engine handle. Dart never dereferences this type; it only passes the
/// pointer back to this library.
pub struct KinetoEngine {
    _engine: Engine,
}

#[cfg(feature = "ownership-test-hooks")]
fn record_engine_destroy_for_test() {
    let Some(path) = std::env::var_os("KINETO_OWNERSHIP_PROBE_FILE") else {
        return;
    };
    let Ok(mut marker) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    use std::io::Write as _;
    let _ = marker.write_all(b"destroy\n");
}

#[unsafe(no_mangle)]
pub extern "C" fn kineto_engine_create() -> *mut KinetoEngine {
    guarded(std::ptr::null_mut(), || {
        Box::into_raw(Box::new(KinetoEngine {
            _engine: Engine::new(),
        }))
    })
}

/// # Safety
///
/// `engine` must either be null or a pointer returned exactly once by
/// [`kineto_engine_create`] that has not already been destroyed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_engine_destroy(engine: *mut KinetoEngine) {
    if engine.is_null() {
        return;
    }
    guarded((), || {
        // SAFETY: the caller contract requires a live pointer allocated by
        // `kineto_engine_create`, and this function consumes that allocation once.
        unsafe { drop(Box::from_raw(engine)) };
        #[cfg(feature = "ownership-test-hooks")]
        record_engine_destroy_for_test();
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn kineto_engine_abi_version() -> u32 {
    KINETO_NATIVE_ABI_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffi_handle_round_trip_is_minimal_and_typed() {
        let engine = kineto_engine_create();
        assert!(!engine.is_null());
        assert_eq!(kineto_engine_abi_version(), 41);

        // SAFETY: `engine` was created above and has not yet been destroyed.
        unsafe { kineto_engine_destroy(engine) };
    }
}
