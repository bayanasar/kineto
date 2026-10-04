//! The plugins the user registered, in `<config>/kineto/plugins.toml`.
//!
//! ```toml
//! [[plugin]]
//! id = "studio-images"
//! executable = "/opt/studio/bin/kineto-images"
//! args = ["--profile", "film"]
//! secrets = ["STUDIO_IMAGES_KEY"]
//! enabled = true
//! ```
//!
//! The file belongs to the user, never to a project. A project names a
//! plugin by ID; the ID is looked up here and nowhere else.

use std::{
    collections::BTreeSet,
    env, fmt, fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

/// The configuration file's name, in Kineto's configuration directory.
pub const CONFIG_FILE: &str = "plugins.toml";

/// The longest plugin ID, which also names its jobs (`plugin.<id>`).
const MAX_ID: usize = 48;

/// One registered plugin.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginEntry {
    pub id: String,
    /// Absolute path of the executable to start.
    pub executable: PathBuf,
    /// Fixed arguments it is started with.
    #[serde(default)]
    pub args: Vec<String>,
    /// Names of the secrets passed to it, as environment variables of the
    /// same names.
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

const fn enabled() -> bool {
    true
}

impl PluginEntry {
    /// The key its jobs and records are kept under.
    #[must_use]
    pub fn provider_key(&self) -> String {
        format!("plugin.{}", self.id)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default, rename = "plugin")]
    plugins: Vec<PluginEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// The file is not valid TOML of the expected shape.
    Unreadable(String),
    /// An entry breaks a rule; the reason says which.
    Invalid(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable(reason) => write!(
                formatter,
                "the plugin configuration is unreadable: {reason}"
            ),
            Self::Invalid(reason) => {
                write!(formatter, "the plugin configuration is invalid: {reason}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// The registered plugins.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginConfig {
    entries: Vec<PluginEntry>,
}

fn portable(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}

fn secret_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        && !name.starts_with(|first: char| first.is_ascii_digit())
}

impl PluginConfig {
    /// Read the configuration in `text`. Every ID is portable and unique,
    /// every executable an absolute path, every secret a variable name.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let file: File =
            toml::from_str(text).map_err(|error| ConfigError::Unreadable(error.to_string()))?;
        let mut seen = BTreeSet::new();
        for entry in &file.plugins {
            if !portable(&entry.id) {
                return Err(ConfigError::Invalid(format!(
                    "{:?} is not a plugin ID: lowercase letters, digits, - and _, at most {MAX_ID}",
                    entry.id
                )));
            }
            if !seen.insert(entry.id.as_str()) {
                return Err(ConfigError::Invalid(format!(
                    "plugin {:?} is registered twice",
                    entry.id
                )));
            }
            if !entry.executable.is_absolute() {
                return Err(ConfigError::Invalid(format!(
                    "plugin {:?} names a relative executable; it must be an absolute path",
                    entry.id
                )));
            }
            if let Some(name) = entry.secrets.iter().find(|name| !secret_name(name)) {
                return Err(ConfigError::Invalid(format!(
                    "plugin {:?} names {name:?} as a secret, which is not a variable name",
                    entry.id
                )));
            }
        }
        Ok(Self {
            entries: file.plugins,
        })
    }

    /// Read the configuration at `path`; none when the file does not exist.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(ConfigError::Unreadable(error.to_string())),
        }
    }

    /// The user's configuration, from [`default_path`]; none when there is
    /// no such file or no place for one.
    pub fn load_default() -> Result<Self, ConfigError> {
        default_path().map_or_else(|| Ok(Self::default()), |path| Self::load(&path))
    }

    #[must_use]
    pub fn entries(&self) -> &[PluginEntry] {
        &self.entries
    }

    /// The enabled plugin `id`, if one is registered.
    #[must_use]
    pub fn enabled(&self, id: &str) -> Option<&PluginEntry> {
        self.entries
            .iter()
            .find(|entry| entry.id == id && entry.enabled)
    }
}

/// Where the user's plugin configuration is: `KINETO_CONFIG_HOME`, else
/// `$XDG_CONFIG_HOME/kineto`, else `~/.config/kineto` (`%APPDATA%\kineto`
/// on Windows), with [`CONFIG_FILE`] in it.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    let directory = env::var_os("KINETO_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("XDG_CONFIG_HOME").map(|root| PathBuf::from(root).join("kineto")))
        .or_else(|| {
            if cfg!(windows) {
                env::var_os("APPDATA").map(|root| PathBuf::from(root).join("kineto"))
            } else {
                env::var_os("HOME").map(|home| PathBuf::from(home).join(".config").join("kineto"))
            }
        })?;
    Some(directory.join(CONFIG_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugins_are_read_with_their_rules() {
        let config = PluginConfig::parse(
            r#"
            [[plugin]]
            id = "studio-images"
            executable = "/opt/studio/bin/kineto-images"
            args = ["--profile", "film"]
            secrets = ["STUDIO_IMAGES_KEY"]

            [[plugin]]
            id = "old"
            executable = "/opt/old"
            enabled = false
            "#,
        )
        .unwrap();
        let studio = config.enabled("studio-images").unwrap();
        assert_eq!(studio.args, ["--profile", "film"]);
        assert_eq!(studio.provider_key(), "plugin.studio-images");
        assert!(config.enabled("old").is_none(), "disabled");
        assert!(config.enabled("missing").is_none());

        for (text, why) in [
            (
                "[[plugin]]\nid = \"Bad ID\"\nexecutable = \"/x\"",
                "is not a plugin ID",
            ),
            (
                "[[plugin]]\nid = \"a\"\nexecutable = \"bin/x\"",
                "relative executable",
            ),
            (
                "[[plugin]]\nid = \"a\"\nexecutable = \"/x\"\n[[plugin]]\nid = \"a\"\nexecutable = \"/y\"",
                "registered twice",
            ),
            (
                "[[plugin]]\nid = \"a\"\nexecutable = \"/x\"\nsecrets = [\"key=1\"]",
                "not a variable name",
            ),
        ] {
            let error = PluginConfig::parse(text).unwrap_err().to_string();
            assert!(error.contains(why), "{error}");
        }
        assert!(matches!(
            PluginConfig::parse("[[plugin]]\nid = \"a\"\nexecutable = \"/x\"\ncommand = \"rm\""),
            Err(ConfigError::Unreadable(_))
        ));
        assert_eq!(
            PluginConfig::load(Path::new("/nonexistent/kineto/plugins.toml")).unwrap(),
            PluginConfig::default()
        );
    }
}
