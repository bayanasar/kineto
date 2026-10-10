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
    ptr, slice,
};

use kineto_core::llm::{LLM_PROVIDER_VARIABLE, agent_executable_variable};
use kineto_llm::agents::{AgentKind, locate};
use kineto_plugins::settings::{
    CONNECTION_PREFIX, Connection, MAX_REFERENCES, Protocol, ProviderSettings, Setting,
};

use crate::{
    contract::{name as name_of, *},
    guarded,
    production::KinetoText,
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

/// The variable that makes `setting`'s choice for a run.
const fn variable(setting: Setting) -> &'static str {
    match setting {
        Setting::Image => IMAGE_PROVIDER_VARIABLE,
        Setting::Video => VIDEO_PROVIDER_VARIABLE,
        Setting::Speech => SPEECH_PROVIDER_VARIABLE,
        Setting::LanguageModel => LLM_PROVIDER_VARIABLE,
    }
}

fn setting(code: u32) -> Option<Setting> {
    Some(match name_of(&PROVIDER_SETTINGS, code)? {
        "image" => Setting::Image,
        "video" => Setting::Video,
        "speech" => Setting::Speech,
        "language_model" => Setting::LanguageModel,
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
        match std::env::var(variable(setting)) {
            Ok(value) => return Ok(Some(value)),
            Err(std::env::VarError::NotUnicode(value)) => {
                return Err(value.to_string_lossy().into_owned());
            }
            Err(std::env::VarError::NotPresent) => {}
        }
        Ok(self.settings.get(setting).map(str::to_owned))
    }

    /// The connection a selection names, when it names one
    /// (`connection:<name>`): `Ok(None)` for a selection of anything else,
    /// and why not when the settings hold no connection of that name.
    pub(crate) fn connection(
        &self,
        selection: Option<&str>,
    ) -> Result<Option<&Connection>, String> {
        let Some(name) =
            selection.and_then(|selection| selection.trim().strip_prefix(CONNECTION_PREFIX))
        else {
            return Ok(None);
        };
        self.settings
            .connection(name)
            .map(Some)
            .ok_or_else(|| format!("there is no connection named {name:?} in settings"))
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
            .is_some_and(|setting| std::env::var_os(variable(setting)).is_some())
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

/// How many connections the settings hold.
///
/// # Safety
/// `settings` must be null or a live handle from [`kineto_settings_load`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_settings_connection_count(settings: *const KinetoSettings) -> u64 {
    guarded(0, || {
        // SAFETY: forwarded caller contract.
        unsafe { settings_ref(settings) }
            .map_or(0, |settings| settings.settings.connections().len() as u64)
    })
}

/// `field` (a `CONNECTION_FIELDS` code) of connection `index`, copied as
/// UTF-8 text: a price in US dollars, the references as a whole number,
/// empty when not given.
///
/// # Safety
/// As [`kineto_settings_text_copy`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_settings_connection_text_copy(
    settings: *const KinetoSettings,
    index: u64,
    field: u32,
    out_buf: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let Some(settings) = (unsafe { settings_ref(settings) }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let Some(connection) = usize::try_from(index)
            .ok()
            .and_then(|index| settings.settings.connections().get(index))
        else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let Some(value) =
            name_of(&CONNECTION_FIELDS, field).map(|field| field_text(connection, field))
        else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: forwarded caller-owned output contract.
        unsafe { copy_utf8(value.as_bytes(), out_buf, out_cap, out_len) }
    })
}

fn field_text(connection: &Connection, field: &str) -> String {
    let price = |price: Option<f64>| price.map(|price| price.to_string()).unwrap_or_default();
    match field {
        "name" => connection.name.clone(),
        "protocol" => connection.protocol.clone(),
        "url" => connection.url.clone(),
        "model" => connection.model.clone(),
        "key" => connection.key.clone().unwrap_or_default(),
        "price_per_million_input" => price(connection.price_per_million_input),
        "price_per_million_output" => price(connection.price_per_million_output),
        "price_per_image" => price(connection.price_per_image),
        "references" if connection.references > 0 => connection.references.to_string(),
        _ => String::new(),
    }
}

/// One field of a connection, as Dart lays it out.
#[repr(C)]
pub struct KinetoConnectionField {
    /// A contract `connection_field` code.
    pub field: u32,
    /// Its value as text: a price in US dollars, a number in decimal, empty
    /// for none.
    pub value: KinetoText,
}

/// The connection `fields` describe; `None` when a field is unknown, given
/// twice, or a number is not one.
fn connection_from(fields: &[(&str, String)]) -> Option<Connection> {
    let mut seen = Vec::with_capacity(fields.len());
    let mut connection = Connection::new("", Protocol::OpenAiChat, "", "");
    let price = |value: &str| -> Option<Option<f64>> {
        if value.is_empty() {
            Some(None)
        } else {
            value.parse::<f64>().ok().map(Some)
        }
    };
    for (field, value) in fields {
        if seen.contains(field) {
            return None;
        }
        seen.push(field);
        let value = value.trim();
        match *field {
            "name" => value.clone_into(&mut connection.name),
            "protocol" => value.clone_into(&mut connection.protocol),
            "url" => value.clone_into(&mut connection.url),
            "model" => value.clone_into(&mut connection.model),
            "key" => connection.key = (!value.is_empty()).then(|| value.to_owned()),
            "price_per_million_input" => connection.price_per_million_input = price(value)?,
            "price_per_million_output" => connection.price_per_million_output = price(value)?,
            "price_per_image" => connection.price_per_image = price(value)?,
            "references" => {
                connection.references = if value.is_empty() {
                    0
                } else {
                    value
                        .parse::<u16>()
                        .ok()
                        .filter(|count| *count <= MAX_REFERENCES)?
                };
            }
            _ => return None,
        }
    }
    Some(connection)
}

/// Read, change with `change` and save the settings: refused, and nothing
/// written, when the file cannot be read or the change is not one it takes.
fn change_settings(change: impl FnOnce(&mut ProviderSettings) -> bool) -> i32 {
    let Some(path) = settings_path() else {
        return PROJECT_ERR_SETTINGS_UNWRITABLE;
    };
    let Ok(mut settings) = ProviderSettings::load(&path) else {
        return PROJECT_ERR_SETTING_INVALID;
    };
    if !change(&mut settings) {
        return PROJECT_ERR_SETTING_INVALID;
    }
    match settings.save(&path) {
        Ok(()) => PROJECT_OK,
        Err(_) => PROJECT_ERR_SETTINGS_UNWRITABLE,
    }
}

/// Add the connection `fields` describe, or put it in place of the one
/// named by the UTF-8 `previous` (empty to add), and save the settings. A
/// choice of the one replaced follows it. Takes effect for projects opened
/// after it. Refused, and nothing written, when a field is unknown or the
/// connection cannot be used as it is.
///
/// # Safety
/// `previous_ptr` must be readable for `previous_len` bytes when
/// `previous_len` is not 0; `fields` must be readable for `field_count`
/// elements, each of whose non-empty texts is readable for its length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_settings_connection_put(
    previous_ptr: *const u8,
    previous_len: u64,
    fields: *const KinetoConnectionField,
    field_count: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let previous = match unsafe { utf8_owned(previous_ptr, previous_len) } {
            Ok(previous) => previous,
            Err(code) => return code,
        };
        if field_count > CONNECTION_FIELDS.len() as u64 || (field_count > 0 && fields.is_null()) {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        let raw = if field_count == 0 {
            &[][..]
        } else {
            // SAFETY: the caller guarantees `field_count` readable elements.
            unsafe { slice::from_raw_parts(fields, field_count as usize) }
        };
        let mut named = Vec::with_capacity(raw.len());
        for field in raw {
            let Some(name) = name_of(&CONNECTION_FIELDS, field.field) else {
                return PROJECT_ERR_INVALID_ARGUMENT;
            };
            // SAFETY: forwarded caller contract.
            match unsafe { utf8_owned(field.value.ptr, field.value.len) } {
                Ok(value) => named.push((name, value)),
                Err(code) => return code,
            }
        }
        let Some(connection) = connection_from(&named) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let previous = previous.trim();
        change_settings(|settings| {
            settings
                .put_connection((!previous.is_empty()).then_some(previous), connection)
                .is_ok()
        })
    })
}

/// Remove the connection named by the UTF-8 `name`, and any choice of it,
/// and save the settings. Its key stays where it is kept.
///
/// # Safety
/// `name_ptr` must be readable for `name_len` bytes when `name_len` is not
/// 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_settings_connection_remove(
    name_ptr: *const u8,
    name_len: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let name = match unsafe { utf8_owned(name_ptr, name_len) } {
            Ok(name) if !name.trim().is_empty() => name,
            Ok(_) => return PROJECT_ERR_INVALID_ARGUMENT,
            Err(code) => return code,
        };
        change_settings(|settings| settings.remove_connection(name.trim()).is_ok())
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
        change_settings(|settings| settings.set(field, &value).is_ok())
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
        settings.agents.insert(
            "claude-code".to_owned(),
            PathBuf::from("/opt/claude/bin/claude"),
        );
        settings
            .put_connection(
                None,
                Connection::new(
                    "glm",
                    Protocol::OpenAiChat,
                    "https://glm.example/v4",
                    "glm-5",
                ),
            )
            .unwrap();
        let choices = Choices { settings };
        assert_eq!(
            choices.lookup("KINETO_CLAUDE_CODE_BIN").as_deref(),
            Some("/opt/claude/bin/claude")
        );
        assert_eq!(choices.selection(Setting::Speech), Ok(None));
        assert_eq!(
            choices
                .connection(Some(" connection:glm "))
                .unwrap()
                .map(|found| found.model.as_str()),
            Some("glm-5")
        );
        assert_eq!(choices.connection(Some("codex")), Ok(None));
        assert!(choices.connection(Some("connection:gone")).is_err());
    }

    #[test]
    fn a_connection_crosses_as_text_fields() {
        let fields = |pairs: &[(&'static str, &str)]| -> Vec<(&'static str, String)> {
            pairs
                .iter()
                .map(|(field, value)| (*field, (*value).to_owned()))
                .collect()
        };
        let connection = connection_from(&fields(&[
            ("name", "seedream"),
            ("protocol", "openai-images"),
            ("url", " https://ark.example/api/v3 "),
            ("model", "seedream-4-0"),
            ("key", ""),
            ("price_per_image", "0.03"),
            ("references", "4"),
            ("price_per_million_input", ""),
        ]))
        .unwrap();
        assert_eq!(connection.check(), Ok(Protocol::OpenAiImages));
        assert_eq!(connection.url, "https://ark.example/api/v3");
        assert_eq!(
            (connection.key.as_deref(), connection.references),
            (None, 4)
        );
        assert_eq!(field_text(&connection, "price_per_image"), "0.03");
        assert_eq!(field_text(&connection, "references"), "4");
        assert_eq!(field_text(&connection, "price_per_million_input"), "");
        assert_eq!(field_text(&connection, "protocol"), "openai-images");

        for bad in [
            fields(&[("name", "a"), ("name", "b")]),
            fields(&[("price_per_image", "cheap")]),
            fields(&[("references", "99")]),
            fields(&[("references", "-1")]),
            fields(&[("colour", "red")]),
        ] {
            assert!(connection_from(&bad).is_none(), "{bad:?}");
        }
        for (name, _) in CONNECTION_FIELDS {
            assert!(
                connection_from(&fields(&[(name, "")])).is_some(),
                "every published field is taken: {name}"
            );
        }
        for (name, _) in CONNECTION_PROTOCOLS {
            assert!(
                Protocol::from_name(&name.replace('_', "-")).is_some(),
                "every published protocol is spoken: {name}"
            );
        }
        assert_eq!(CONNECTION_PROTOCOLS.len(), Protocol::ALL.len());
    }

    #[test]
    fn connections_are_never_written_to_the_developers_settings() {
        let field = KinetoConnectionField {
            field: 1,
            value: KinetoText {
                ptr: b"glm".as_ptr(),
                len: 3,
            },
        };
        assert_eq!(
            unsafe { kineto_settings_connection_put(ptr::null(), 0, &raw const field, 1) },
            PROJECT_ERR_SETTINGS_UNWRITABLE
        );
        assert_eq!(
            unsafe { kineto_settings_connection_put(ptr::null(), 0, ptr::null(), 1) },
            PROJECT_ERR_INVALID_ARGUMENT
        );
        let unknown = KinetoConnectionField { field: 99, ..field };
        assert_eq!(
            unsafe { kineto_settings_connection_put(ptr::null(), 0, &raw const unknown, 1) },
            PROJECT_ERR_INVALID_ARGUMENT
        );
        assert_eq!(
            unsafe { kineto_settings_connection_remove(b"glm".as_ptr(), 3) },
            PROJECT_ERR_SETTINGS_UNWRITABLE
        );
        assert_eq!(
            unsafe { kineto_settings_connection_remove(ptr::null(), 0) },
            PROJECT_ERR_INVALID_ARGUMENT
        );
        let mut handle = ptr::null_mut();
        assert_eq!(unsafe { kineto_settings_load(&raw mut handle) }, PROJECT_OK);
        assert_eq!(unsafe { kineto_settings_connection_count(handle) }, 0);
        let mut buffer = [0_u8; 8];
        let mut len = 0;
        assert_eq!(
            unsafe {
                kineto_settings_connection_text_copy(
                    handle,
                    0,
                    1,
                    buffer.as_mut_ptr(),
                    8,
                    &raw mut len,
                )
            },
            PROJECT_ERR_INVALID_ARGUMENT
        );
        unsafe { kineto_settings_destroy(handle) };
    }
}
