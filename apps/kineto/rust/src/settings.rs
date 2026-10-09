//! The director's provider choices (`providers.toml`, see
//! `kineto_plugins::settings`) and the agent CLIs installed, as settings
//! show and change them, and as a session reads them when it chooses its
//! providers.
//!
//! These calls belong to no project. Each reads or writes the file or looks
//! for programs, so Dart makes them off the UI thread. A choice an
//! environment variable makes wins over the file, for development, and
//! settings say so.

use std::{
    path::{Path, PathBuf},
    ptr,
};

use kineto_core::llm::{
    LLM_PROVIDER_VARIABLE, LOCAL_KEY_VARIABLE, LOCAL_MODEL_VARIABLE, LOCAL_URL_VARIABLE,
    agent_executable_variable,
};
use kineto_llm::agents::{AgentKind, locate};
use kineto_plugins::settings::{ProviderSettings, Setting};

use crate::{
    contract::*,
    guarded,
    project_session::{IMAGE_PROVIDER_VARIABLE, copy_utf8, utf8_owned},
};

pub(crate) const VIDEO_PROVIDER_VARIABLE: &str = "KINETO_VIDEO_PROVIDER";
pub(crate) const SPEECH_PROVIDER_VARIABLE: &str = "KINETO_SPEECH_PROVIDER";

/// Where the settings file is. Tests never read the developer's own.
fn settings_path() -> Option<PathBuf> {
    if cfg!(test) {
        None
    } else {
        kineto_plugins::settings::default_path()
    }
}

fn load_settings() -> Result<ProviderSettings, String> {
    settings_path().map_or_else(
        || Ok(ProviderSettings::default()),
        |path| ProviderSettings::load(&path).map_err(|error| error.to_string()),
    )
}

/// The variable that makes `setting`'s choice for a run, if it is one.
const fn variable(setting: Setting) -> Option<&'static str> {
    match setting {
        Setting::Image => Some(IMAGE_PROVIDER_VARIABLE),
        Setting::Video => Some(VIDEO_PROVIDER_VARIABLE),
        Setting::Speech => Some(SPEECH_PROVIDER_VARIABLE),
        Setting::LanguageModel => Some(LLM_PROVIDER_VARIABLE),
        Setting::EndpointUrl => Some(LOCAL_URL_VARIABLE),
        Setting::EndpointModel => Some(LOCAL_MODEL_VARIABLE),
        Setting::EndpointKey => Some(LOCAL_KEY_VARIABLE),
    }
}

fn setting(code: u32) -> Option<Setting> {
    let name = PROVIDER_SETTINGS
        .iter()
        .find(|(_, known)| *known == code)
        .map(|(name, _)| *name)?;
    Some(match name {
        "image" => Setting::Image,
        "video" => Setting::Video,
        "speech" => Setting::Speech,
        "language_model" => Setting::LanguageModel,
        "endpoint_url" => Setting::EndpointUrl,
        "endpoint_model" => Setting::EndpointModel,
        "endpoint_key" => Setting::EndpointKey,
        _ => return None,
    })
}

fn agent(code: u32) -> Option<AgentKind> {
    let name = AGENTS
        .iter()
        .find(|(_, known)| *known == code)
        .map(|(name, _)| *name)?;
    AgentKind::from_name(&name.replace('_', "-"))
}

/// What a session reads its providers from: the environment first, then
/// the director's settings.
pub(crate) struct Choices {
    settings: ProviderSettings,
}

impl Choices {
    /// The director's settings; why they cannot be read, when they cannot.
    pub(crate) fn load() -> Result<Self, String> {
        load_settings().map(|settings| Self { settings })
    }

    /// What `setting` names: its variable when that is set, else the
    /// settings' choice, else nothing. A variable that is not Unicode, as
    /// text, names nothing this build has.
    pub(crate) fn selection(&self, setting: Setting) -> Result<Option<String>, String> {
        if let Some(variable) = variable(setting) {
            match std::env::var(variable) {
                Ok(value) => return Ok(Some(value)),
                Err(std::env::VarError::NotUnicode(value)) => {
                    return Err(value.to_string_lossy().into_owned());
                }
                Err(std::env::VarError::NotPresent) => {}
            }
        }
        Ok(self.settings.get(setting).map(str::to_owned))
    }

    /// A value a provider's configuration reads by its variable's name: the
    /// variable when it is set, else what the settings say for it.
    pub(crate) fn lookup(&self, name: &str) -> Option<String> {
        if let Some(value) = std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            return Some(value);
        }
        let endpoint = [
            (LOCAL_URL_VARIABLE, Setting::EndpointUrl),
            (LOCAL_MODEL_VARIABLE, Setting::EndpointModel),
            (LOCAL_KEY_VARIABLE, Setting::EndpointKey),
        ];
        if let Some((_, setting)) = endpoint.iter().find(|(variable, _)| *variable == name) {
            return self.settings.get(*setting).map(str::to_owned);
        }
        AgentKind::ALL
            .into_iter()
            .find(|agent| agent_executable_variable(*agent) == name)
            .and_then(|agent| self.settings.agent_executable(agent.name()))
            .map(|path| path.to_string_lossy().into_owned())
    }

    /// Where `agent` is: where the settings or its variable say, else where
    /// kineto finds it.
    fn agent_path(&self, agent: AgentKind) -> Option<PathBuf> {
        self.lookup(&agent_executable_variable(agent))
            .map(PathBuf::from)
            .filter(|path| path.is_file())
            .or_else(|| locate(agent, &|name| std::env::var(name).ok()))
    }
}

/// The director's provider settings and installed agents, read once.
pub struct KinetoSettings {
    settings: ProviderSettings,
    /// Why the file cannot be read; nothing is changed while it cannot.
    problem: String,
    agents: Vec<(AgentKind, Option<PathBuf>)>,
}

/// Read the settings and look for each agent CLI. Writes a handle the
/// caller destroys with [`kineto_settings_destroy`]; a file that cannot be
/// read still gives one, which says why.
///
/// # Safety
/// `out_settings` must point to writable storage for one pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_settings_load(out_settings: *mut *mut KinetoSettings) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_settings.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        let (settings, problem) = match load_settings() {
            Ok(settings) => (settings, String::new()),
            Err(problem) => (ProviderSettings::default(), problem),
        };
        let choices = Choices {
            settings: settings.clone(),
        };
        let agents = AgentKind::ALL
            .into_iter()
            .map(|agent| (agent, choices.agent_path(agent)))
            .collect();
        let handle = Box::into_raw(Box::new(KinetoSettings {
            settings,
            problem,
            agents,
        }));
        // SAFETY: checked non-null; the caller guarantees it is writable.
        unsafe { ptr::write(out_settings, handle) };
        PROJECT_OK
    })
}

/// # Safety
/// `settings` must be null or a live handle from [`kineto_settings_load`].
unsafe fn settings_ref<'a>(settings: *const KinetoSettings) -> Option<&'a KinetoSettings> {
    // SAFETY: forwarded caller contract.
    unsafe { settings.as_ref() }
}

/// The choice for `field` (a `PROVIDER_SETTINGS` code), copied as UTF-8; empty when
/// none is made.
///
/// # Safety
/// `settings` must be a live handle; `out_len` must point to writable
/// storage for one `u64`, and `out_buf` to `out_cap` writable bytes when
/// `out_cap` is not 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_settings_text_copy(
    settings: *const KinetoSettings,
    field: u32,
    out_buf: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let (Some(settings), Some(field)) = (unsafe { settings_ref(settings) }, setting(field))
        else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let value = settings.settings.get(field).unwrap_or_default();
        // SAFETY: forwarded caller-owned output contract.
        unsafe { copy_utf8(value.as_bytes(), out_buf, out_cap, out_len) }
    })
}

/// 1 when `field`'s environment variable is set, so it, not the settings,
/// makes the choice for this run; 0 when it is not; 0 for an unknown field.
#[unsafe(no_mangle)]
pub extern "C" fn kineto_settings_overridden(field: u32) -> u8 {
    guarded(0, || {
        setting(field)
            .and_then(variable)
            .is_some_and(|variable| std::env::var_os(variable).is_some())
            .into()
    })
}

/// Why the settings file cannot be read, copied as UTF-8; empty when it
/// can.
///
/// # Safety
/// As [`kineto_settings_text_copy`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_settings_problem_copy(
    settings: *const KinetoSettings,
    out_buf: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let Some(settings) = (unsafe { settings_ref(settings) }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: forwarded caller-owned output contract.
        unsafe { copy_utf8(settings.problem.as_bytes(), out_buf, out_cap, out_len) }
    })
}

/// Where agent `code` (an `AGENTS` code) is installed, copied as UTF-8;
/// empty when it was not found.
///
/// # Safety
/// As [`kineto_settings_text_copy`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_settings_agent_path_copy(
    settings: *const KinetoSettings,
    code: u32,
    out_buf: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let (Some(settings), Some(agent)) = (unsafe { settings_ref(settings) }, agent(code)) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let path = settings
            .agents
            .iter()
            .find(|(found, _)| *found == agent)
            .and_then(|(_, path)| path.as_deref())
            .map(Path::to_string_lossy)
            .unwrap_or_default();
        // SAFETY: forwarded caller-owned output contract.
        unsafe { copy_utf8(path.as_bytes(), out_buf, out_cap, out_len) }
    })
}

/// Release a handle from [`kineto_settings_load`]. Null is ignored.
///
/// # Safety
/// `settings` must be null or a live handle, destroyed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_settings_destroy(settings: *mut KinetoSettings) {
    guarded((), || {
        if !settings.is_null() {
            // SAFETY: the caller hands back a live handle once.
            drop(unsafe { Box::from_raw(settings) });
        }
    });
}

/// Choose the UTF-8 `value` for `field` (a `PROVIDER_SETTINGS` code) and save the
/// settings; empty clears the choice. Takes effect for projects opened
/// after it. Refused, and nothing written, when the value is not one the
/// field takes or the file cannot be read.
///
/// # Safety
/// `value_ptr` must be readable for `value_len` bytes when `value_len` is
/// not 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_settings_set(
    field: u32,
    value_ptr: *const u8,
    value_len: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        let Some(field) = setting(field) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: forwarded caller contract.
        let value = match unsafe { utf8_owned(value_ptr, value_len) } {
            Ok(value) => value,
            Err(code) => return code,
        };
        let Some(path) = settings_path() else {
            return PROJECT_ERR_SETTINGS_UNWRITABLE;
        };
        let Ok(mut settings) = ProviderSettings::load(&path) else {
            return PROJECT_ERR_SETTING_INVALID;
        };
        if settings.set(field, &value).is_err() {
            return PROJECT_ERR_SETTING_INVALID;
        }
        match settings.save(&path) {
            Ok(()) => PROJECT_OK,
            Err(_) => PROJECT_ERR_SETTINGS_UNWRITABLE,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setting_and_agent_has_a_code() {
        for (name, code) in PROVIDER_SETTINGS {
            assert!(setting(code).is_some(), "{name}");
        }
        for (name, code) in AGENTS {
            assert_eq!(
                agent(code).map(AgentKind::name),
                Some(name.replace('_', "-").as_str())
            );
        }
        assert_eq!(setting(0), None);
        assert_eq!(agent(99), None);
    }

    #[test]
    fn settings_are_read_without_the_developers_own_file() {
        let mut handle = ptr::null_mut();
        assert_eq!(unsafe { kineto_settings_load(&raw mut handle) }, PROJECT_OK);
        let mut buffer = [0_u8; 64];
        let mut len = 0;
        assert_eq!(
            unsafe { kineto_settings_problem_copy(handle, buffer.as_mut_ptr(), 64, &raw mut len) },
            PROJECT_OK
        );
        assert_eq!(len, 0);
        assert_eq!(
            unsafe { kineto_settings_text_copy(handle, 99, buffer.as_mut_ptr(), 64, &raw mut len) },
            PROJECT_ERR_INVALID_ARGUMENT
        );
        unsafe { kineto_settings_destroy(handle) };
        assert_eq!(
            unsafe { kineto_settings_set(1, b"agy".as_ptr(), 3) },
            PROJECT_ERR_SETTINGS_UNWRITABLE,
            "tests never write the developer's settings"
        );
    }

    #[test]
    fn the_environment_wins_over_the_settings_only_when_set() {
        let mut settings = ProviderSettings::default();
        settings.set(Setting::EndpointModel, "llama").unwrap();
        settings.agents.insert(
            "claude-code".to_owned(),
            PathBuf::from("/opt/claude/bin/claude"),
        );
        let choices = Choices { settings };
        assert_eq!(
            choices.lookup(LOCAL_MODEL_VARIABLE).as_deref(),
            Some("llama")
        );
        assert_eq!(
            choices.lookup("KINETO_CLAUDE_CODE_BIN").as_deref(),
            Some("/opt/claude/bin/claude")
        );
        assert_eq!(choices.selection(Setting::Speech), Ok(None));
    }
}
