//! Which providers the director chose, in `<config>/kineto/providers.toml`
//! beside `plugins.toml`.
//!
//! ```toml
//! image = "agy"
//! video = "veo:fast"
//! speech = "minimax"
//! language_model = "claude-code"
//!
//! [openai_compatible]
//! url = "https://api.deepseek.com/v1"
//! model = "deepseek-chat"
//! key = "deepseek"
//!
//! [agents]
//! claude-code = "/opt/claude/bin/claude"
//! ```
//!
//! Each choice is a selection a session reads at open, as its environment
//! variable would name it (`KINETO_IMAGE_PROVIDER`, `KINETO_VIDEO_PROVIDER`,
//! `KINETO_SPEECH_PROVIDER`, `KINETO_LLM_PROVIDER`); a variable that is set
//! wins, for development. `key` names a key in the system's secure storage,
//! never the key itself. `[agents]` says where an agent CLI is when it is
//! not where kineto looks. The file belongs to the user, never to a project,
//! holds no secret, and keeps whatever a newer build wrote that this one
//! does not know.

use std::{
    collections::BTreeMap,
    fmt, fs, io,
    net::IpAddr,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::ConfigError;

/// The settings file's name, in Kineto's configuration directory.
pub const SETTINGS_FILE: &str = "providers.toml";

/// The longest value a setting may hold.
const MAX_VALUE: usize = 1024;

/// An OpenAI-compatible server the director named.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Endpoint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The name its key is stored under, never the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(flatten)]
    extra: toml::Table,
}

impl Endpoint {
    fn is_empty(&self) -> bool {
        self.url.is_none() && self.model.is_none() && self.key.is_none() && self.extra.is_empty()
    }
}

/// The director's provider choices.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speech: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language_model: Option<String>,
    #[serde(default, skip_serializing_if = "Endpoint::is_empty")]
    pub openai_compatible: Endpoint,
    /// Where an agent CLI is, by agent name, when it is not where kineto
    /// looks.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<String, PathBuf>,
    #[serde(flatten)]
    extra: toml::Table,
}

/// One setting, as settings change them one at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Image,
    Video,
    Speech,
    LanguageModel,
    EndpointUrl,
    EndpointModel,
    EndpointKey,
}

/// Why a setting was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidSetting(pub String);

impl fmt::Display for InvalidSetting {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for InvalidSetting {}

/// Whether `name` can name the key a server takes: 1 to 64 lowercase ASCII
/// letters, digits and `_`, not starting with a digit, as provider keys are
/// named, so the environment's `KINETO_<NAME>_API_KEY` can stand in for it.
#[must_use]
pub fn key_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with(|character: char| character.is_ascii_digit())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

impl ProviderSettings {
    /// Read settings from TOML text.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        toml::from_str(text).map_err(|error| ConfigError::Unreadable(error.to_string()))
    }

    /// Read the settings at `path`; none chosen when there is no such file.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(ConfigError::Unreadable(error.to_string())),
        }
    }

    /// The user's settings, from [`default_path`].
    pub fn load_default() -> Result<Self, ConfigError> {
        default_path().map_or_else(|| Ok(Self::default()), |path| Self::load(&path))
    }

    /// Write the settings to `path` whole, replacing what was there only
    /// once the new file is complete.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let text = toml::to_string(self)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let staged = path.with_extension("toml.kineto-tmp");
        fs::write(&staged, text)?;
        fs::rename(&staged, path)
    }

    /// The value of `setting`, `None` when it is not chosen.
    #[must_use]
    pub fn get(&self, setting: Setting) -> Option<&str> {
        match setting {
            Setting::Image => self.image.as_deref(),
            Setting::Video => self.video.as_deref(),
            Setting::Speech => self.speech.as_deref(),
            Setting::LanguageModel => self.language_model.as_deref(),
            Setting::EndpointUrl => self.openai_compatible.url.as_deref(),
            Setting::EndpointModel => self.openai_compatible.model.as_deref(),
            Setting::EndpointKey => self.openai_compatible.key.as_deref(),
        }
    }

    /// Choose `value` for `setting`; empty clears it. A server address must
    /// be HTTPS, or HTTP to this computer; a key is named, never given.
    pub fn set(&mut self, setting: Setting, value: &str) -> Result<(), InvalidSetting> {
        let value = value.trim();
        if value.len() > MAX_VALUE || value.chars().any(char::is_control) {
            return Err(InvalidSetting(
                "the value is too long or not plain text".to_owned(),
            ));
        }
        match setting {
            Setting::EndpointUrl if !value.is_empty() && !acceptable_url(value) => {
                return Err(InvalidSetting(
                    "a server address must start with https://, or http:// to this computer"
                        .to_owned(),
                ));
            }
            Setting::EndpointKey if !value.is_empty() && !key_name(value) => {
                return Err(InvalidSetting(
                    "a key name is lowercase letters, digits and _, such as deepseek".to_owned(),
                ));
            }
            _ => {}
        }
        let value = (!value.is_empty()).then(|| value.to_owned());
        let slot = match setting {
            Setting::Image => &mut self.image,
            Setting::Video => &mut self.video,
            Setting::Speech => &mut self.speech,
            Setting::LanguageModel => &mut self.language_model,
            Setting::EndpointUrl => &mut self.openai_compatible.url,
            Setting::EndpointModel => &mut self.openai_compatible.model,
            Setting::EndpointKey => &mut self.openai_compatible.key,
        };
        *slot = value;
        Ok(())
    }

    /// Where `agent` is, when the director said.
    #[must_use]
    pub fn agent_executable(&self, agent: &str) -> Option<&Path> {
        self.agents.get(agent).map(PathBuf::as_path)
    }
}

/// HTTPS anywhere, or HTTP to a loopback address: the rule the requests
/// themselves are held to (`kineto_llm::http::is_permitted`), so an address
/// accepted here is never refused when used.
///
/// The host must be `localhost` or a loopback IP address written as one.
/// A name that only starts like one (`127.attacker.example`) may resolve
/// anywhere and would receive the key in the clear. An authority with user
/// information (`localhost:80@host`) is refused, because the host is what
/// follows the `@`.
fn acceptable_url(url: &str) -> bool {
    if let Some(rest) = url.strip_prefix("https://") {
        return !rest.is_empty();
    }
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains(['@', '\\']) {
        return false;
    }
    let host = match authority.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next().unwrap_or_default(),
        None => authority.split(':').next().unwrap_or_default(),
    };
    host == "localhost"
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// Where the user's provider settings are, beside their plugins.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    crate::config::default_path().map(|plugins| plugins.with_file_name(SETTINGS_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_are_read_and_written_with_what_a_newer_build_added() {
        let settings = ProviderSettings::parse(
            r#"
image = "agy"
language_model = "claude-code"
future = "kept"

[openai_compatible]
url = "https://api.deepseek.example/v1"
model = "deepseek-chat"
key = "deepseek"
tier = "kept too"

[agents]
claude-code = "/opt/claude/bin/claude"
"#,
        )
        .unwrap();
        assert_eq!(settings.get(Setting::Image), Some("agy"));
        assert_eq!(settings.get(Setting::Video), None);
        assert_eq!(settings.get(Setting::EndpointKey), Some("deepseek"));
        assert_eq!(
            settings.agent_executable("claude-code"),
            Some(Path::new("/opt/claude/bin/claude"))
        );
        let written = toml::to_string(&settings).unwrap();
        assert!(written.contains("future = \"kept\""), "{written}");
        assert!(written.contains("tier = \"kept too\""), "{written}");
        assert_eq!(ProviderSettings::parse(&written).unwrap(), settings);
    }

    #[test]
    fn a_setting_is_changed_cleared_or_refused() {
        let mut settings = ProviderSettings::default();
        settings.set(Setting::LanguageModel, " codex ").unwrap();
        assert_eq!(settings.get(Setting::LanguageModel), Some("codex"));
        settings.set(Setting::LanguageModel, "").unwrap();
        assert_eq!(settings.get(Setting::LanguageModel), None);

        for url in [
            "https://api.example.test/v1",
            "http://localhost:11434/v1",
            "http://127.0.0.1:8080",
            "http://127.0.0.2:8080",
            "http://[::1]:8080/v1",
        ] {
            settings.set(Setting::EndpointUrl, url).unwrap();
        }
        for url in [
            "http://api.example.test/v1",
            "ftp://x",
            "https://",
            "http://10.0.0.2/v1",
            // Names that only look local may resolve anywhere.
            "http://127.attacker.example/v1",
            "http://127.0.0.1.attacker.example/v1",
            "http://localhost.attacker.example/v1",
            // The host is what follows the user information.
            "http://localhost:80@attacker.example/v1",
            "http://127.0.0.1@attacker.example/v1",
            "http://localhost\\@attacker.example/v1",
            // Refused when used, so refused when chosen.
            "HTTPS://api.example.test/v1",
            "http://LOCALHOST:11434/v1",
        ] {
            assert!(settings.set(Setting::EndpointUrl, url).is_err(), "{url}");
        }
        settings.set(Setting::EndpointKey, "deepseek").unwrap();
        settings.set(Setting::EndpointKey, "open_router").unwrap();
        for name in ["sk-live-123", "deep seek", "1key", "OPENROUTER"] {
            assert!(settings.set(Setting::EndpointKey, name).is_err(), "{name}");
        }
        assert!(settings.set(Setting::Image, "a\nb").is_err());
    }

    #[test]
    fn settings_are_saved_whole_and_read_back() {
        let directory =
            std::env::temp_dir().join(format!("kineto-provider-settings-{}", std::process::id()));
        let path = directory.join("kineto").join(SETTINGS_FILE);
        assert_eq!(
            ProviderSettings::load(&path).unwrap(),
            ProviderSettings::default(),
            "none chosen before the file exists"
        );
        let mut settings = ProviderSettings::default();
        settings.set(Setting::Image, "agy").unwrap();
        settings.set(Setting::EndpointModel, "llama").unwrap();
        settings.save(&path).unwrap();
        assert_eq!(ProviderSettings::load(&path).unwrap(), settings);
        assert!(!path.with_extension("toml.kineto-tmp").exists());
        let _ = fs::remove_dir_all(&directory);
    }
}
