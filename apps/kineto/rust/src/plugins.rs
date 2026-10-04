//! The provider plugins the user registered (ADR-0006), as settings show
//! them.
//!
//! The listing is read from the user's configuration, not a project, and
//! each enabled plugin is started once to describe itself, so loading it
//! may take as long as a slow plugin's deadline: Dart loads it off the UI
//! thread. Nothing here starts a paid call or changes the configuration;
//! executables are never entered through the app.

use std::ptr;

use crate::{contract::*, documents::at, guarded, project_session::copy_utf8};
use kineto_core::plugin::{self, PluginListing, SELECTION_PREFIX};
use kineto_plugins::PluginConfig;
use kineto_providers::ReferenceRole;

/// Opaque listing handle. Dart never dereferences this allocation.
pub struct KinetoPlugins {
    listed: Vec<PluginListing>,
    /// The plugin `KINETO_IMAGE_PROVIDER` names, if any.
    selected: Option<String>,
}

/// Read the user's plugin configuration and describe each enabled plugin.
/// On `PROJECT_OK` a handle is written to `out_plugins`; it must be
/// destroyed. An unreadable or invalid configuration answers
/// `plugin_config_invalid`; no configuration lists nothing.
///
/// # Safety
/// `out_plugins` must point to writable storage for one pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_plugins_load(out_plugins: *mut *mut KinetoPlugins) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_plugins.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: checked non-null; the caller guarantees it is writable.
        unsafe { ptr::write(out_plugins, ptr::null_mut()) };
        let Ok(config) = PluginConfig::load_default() else {
            return PROJECT_ERR_PLUGIN_CONFIG_INVALID;
        };
        let selected = std::env::var(crate::project_session::IMAGE_PROVIDER_VARIABLE)
            .ok()
            .and_then(|selection| {
                selection
                    .trim()
                    .strip_prefix(SELECTION_PREFIX)
                    .map(str::to_owned)
            });
        let plugins = Box::new(KinetoPlugins {
            listed: plugin::list(&config),
            selected,
        });
        // SAFETY: as above.
        unsafe { ptr::write(out_plugins, Box::into_raw(plugins)) };
        PROJECT_OK
    })
}

/// How many plugins are registered; 0 for a null handle.
///
/// # Safety
/// `plugins` must be null or a live listing handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_plugins_count(plugins: *const KinetoPlugins) -> u32 {
    guarded(0, || {
        // SAFETY: the caller guarantees a live handle or null.
        unsafe { plugins.as_ref() }.map_or(0, |plugins| {
            u32::try_from(plugins.listed.len()).unwrap_or(u32::MAX)
        })
    })
}

fn value(plugins: &KinetoPlugins, listing: &PluginListing, field: &str) -> Option<u64> {
    let described = listing
        .described
        .as_ref()
        .and_then(|found| found.as_ref().ok());
    let images = described.and_then(|plugin| plugin.image_capabilities());
    let speech = described.and_then(|plugin| plugin.speech_capabilities());
    Some(match field {
        "enabled" => u64::from(listing.entry.enabled),
        "described" => u64::from(described.is_some()),
        "selected" => u64::from(plugins.selected.as_deref() == Some(listing.entry.id.as_str())),
        "makes_images" => u64::from(images.is_some()),
        "speaks" => u64::from(speech.is_some()),
        "identity_references" => images.map_or(0, |images| {
            u64::from(images.reference_limit(ReferenceRole::Identity))
        }),
        "composition_reference" => u64::from(
            images.is_some_and(|images| images.reference_limit(ReferenceRole::Composition) > 0),
        ),
        "image_price_known" => {
            u64::from(images.is_some_and(|images| images.price_per_image.is_some()))
        }
        "image_price_micros" => images
            .and_then(|images| images.price_per_image)
            .map_or(0, |price| price.amount_micros),
        "speech_price_known" => {
            u64::from(speech.is_some_and(|speech| speech.price_per_character.is_some()))
        }
        "speech_character_price_micros" => speech
            .and_then(|speech| speech.price_per_character)
            .map_or(0, |price| price.amount_micros),
        _ => return None,
    })
}

/// Number field `field` (a contract `plugin_value` code) of plugin `index`.
///
/// # Safety
/// `plugins` must be a live listing handle and `out_value` writable storage
/// for one `u64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_plugins_value(
    plugins: *const KinetoPlugins,
    index: u32,
    field: u32,
    out_value: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: the caller guarantees a live handle or null.
        let Some(plugins) = (unsafe { plugins.as_ref() }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        if out_value.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        let Some(found) = at(&plugins.listed, index)
            .zip(name(&PLUGIN_VALUES, field))
            .and_then(|(listing, field)| value(plugins, listing, field))
        else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: checked non-null; the caller guarantees it is writable.
        unsafe { out_value.write(found) };
        PROJECT_OK
    })
}

fn text(listing: &PluginListing, field: &str) -> Option<String> {
    let described = listing
        .described
        .as_ref()
        .and_then(|found| found.as_ref().ok());
    let lines = |items: Vec<String>| items.join("\n");
    Some(match field {
        "id" => listing.entry.id.clone(),
        "name" => described
            .map(|plugin| plugin.name().to_owned())
            .unwrap_or_default(),
        "version" => described
            .map(|plugin| plugin.version().to_owned())
            .unwrap_or_default(),
        "executable" => listing.entry.executable.to_string_lossy().into_owned(),
        "secrets" => lines(listing.entry.secrets.clone()),
        "image_model" => described
            .and_then(|plugin| plugin.image_model())
            .unwrap_or_default()
            .to_owned(),
        "speech_model" => described
            .and_then(|plugin| plugin.speech_model())
            .unwrap_or_default()
            .to_owned(),
        "problem" => match &listing.described {
            Some(Err(problem)) => problem.clone(),
            _ => String::new(),
        },
        "aspect_ratios" => lines(
            described
                .and_then(|plugin| plugin.image_capabilities())
                .map(|images| {
                    images
                        .aspect_ratios
                        .iter()
                        .map(|ratio| format!("{}:{}", ratio.width, ratio.height))
                        .collect()
                })
                .unwrap_or_default(),
        ),
        "languages" => lines(
            described
                .and_then(|plugin| plugin.speech_capabilities())
                .map(|speech| speech.languages.iter().cloned().collect())
                .unwrap_or_default(),
        ),
        _ => return None,
    })
}

/// Copy text field `field` (a contract `plugin_text` code) of plugin
/// `index`. Same sizing contract as project metadata.
///
/// # Safety
/// `plugins` must be a live listing handle. `out_len` must point to writable
/// `u64` storage. If `out_cap` is non-zero, `out_buf` must be writable for at
/// least `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_plugins_text_copy(
    plugins: *const KinetoPlugins,
    index: u32,
    field: u32,
    out_buf: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: the caller guarantees a live handle or null.
        let Some(plugins) = (unsafe { plugins.as_ref() }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let Some(found) = at(&plugins.listed, index)
            .zip(name(&PLUGIN_TEXTS, field))
            .and_then(|(listing, field)| text(listing, field))
        else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: forwarded caller-owned output contract.
        unsafe { copy_utf8(found.as_bytes(), out_buf, out_cap, out_len) }
    })
}

/// Release a listing.
///
/// # Safety
/// `plugins` must be null or a handle from [`kineto_plugins_load`] not yet
/// destroyed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_plugins_destroy(plugins: *mut KinetoPlugins) {
    if plugins.is_null() {
        return;
    }
    guarded((), || {
        // SAFETY: the caller contract makes this the one release of a live
        // allocation from `kineto_plugins_load`.
        unsafe { drop(Box::from_raw(plugins)) };
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_text(plugins: &KinetoPlugins, index: u32, field: &str) -> Result<String, i32> {
        let mut buffer = [0_u8; 256];
        let mut len = 0_u64;
        // SAFETY: a live listing, a writable buffer of its stated size and a
        // writable length.
        let status = unsafe {
            kineto_plugins_text_copy(
                plugins,
                index,
                code(&PLUGIN_TEXTS, field),
                buffer.as_mut_ptr(),
                buffer.len() as u64,
                &raw mut len,
            )
        };
        if status == PROJECT_OK {
            Ok(String::from_utf8(buffer[..len as usize].to_vec()).unwrap())
        } else {
            Err(status)
        }
    }

    fn read_value(plugins: &KinetoPlugins, index: u32, field: &str) -> Result<u64, i32> {
        let mut value = 0_u64;
        // SAFETY: a live listing and a writable value.
        let status = unsafe {
            kineto_plugins_value(plugins, index, code(&PLUGIN_VALUES, field), &raw mut value)
        };
        if status == PROJECT_OK {
            Ok(value)
        } else {
            Err(status)
        }
    }

    #[test]
    fn registered_plugins_are_read_field_by_field() {
        let config = PluginConfig::parse(
            "[[plugin]]\nid = \"gone\"\nexecutable = \"/nonexistent/kineto-plugin\"\n\
             secrets = [\"GONE_KEY\", \"GONE_REGION\"]\n\n\
             [[plugin]]\nid = \"off\"\nexecutable = \"/nonexistent/off\"\nenabled = false\n",
        )
        .unwrap();
        let plugins = KinetoPlugins {
            listed: plugin::list(&config),
            selected: Some("gone".to_owned()),
        };
        // SAFETY: a live listing.
        assert_eq!(unsafe { kineto_plugins_count(&plugins) }, 2);
        assert_eq!(unsafe { kineto_plugins_count(ptr::null()) }, 0);

        assert_eq!(read_text(&plugins, 0, "id").unwrap(), "gone");
        assert_eq!(
            read_text(&plugins, 0, "secrets").unwrap(),
            "GONE_KEY\nGONE_REGION"
        );
        assert_eq!(read_value(&plugins, 0, "enabled"), Ok(1));
        assert_eq!(read_value(&plugins, 0, "selected"), Ok(1));
        // An enabled plugin that cannot start says why and declares nothing.
        assert_eq!(read_value(&plugins, 0, "described"), Ok(0));
        assert_eq!(read_value(&plugins, 0, "makes_images"), Ok(0));
        assert!(
            read_text(&plugins, 0, "problem")
                .unwrap()
                .contains("could not be started")
        );
        assert_eq!(read_text(&plugins, 0, "aspect_ratios").unwrap(), "");

        // A disabled plugin is listed without being started.
        assert_eq!(read_value(&plugins, 1, "enabled"), Ok(0));
        assert_eq!(read_value(&plugins, 1, "selected"), Ok(0));
        assert_eq!(read_text(&plugins, 1, "problem").unwrap(), "");

        assert_eq!(
            read_value(&plugins, 2, "enabled"),
            Err(PROJECT_ERR_INVALID_ARGUMENT)
        );
        let mut value = 0_u64;
        // SAFETY: as above; the field code is unknown.
        assert_eq!(
            unsafe { kineto_plugins_value(&plugins, 0, 999, &raw mut value) },
            PROJECT_ERR_INVALID_ARGUMENT
        );
        // SAFETY: destroying null is allowed.
        unsafe { kineto_plugins_destroy(ptr::null_mut()) };
    }
}
