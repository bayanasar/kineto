//! Which providers the director chose, in `<config>/kineto/providers.toml`
//! beside `plugins.toml`.
//!
//! ```toml
//! image = "connection:seedream"
//! video = "veo:fast"
//! speech = "minimax"
//! language_model = "connection:deepseek"
//!
//! [[connection]]
//! name = "deepseek"
//! protocol = "openai-chat"
//! url = "https://api.deepseek.com/v1"
//! model = "deepseek-chat"
//! key = "deepseek"
//! price_per_million_input = 0.28
//! price_per_million_output = 0.42
//!
//! [[connection]]
//! name = "seedream"
//! protocol = "openai-images"
//! url = "https://ark.ap-southeast.bytepluses.com/api/v3"
//! model = "seedream-4-0-250828"
//! key = "seedream"
//! price_per_image = 0.03
//!
//! [agents]
//! claude-code = "/opt/claude/bin/claude"
//! ```
//!
//! Each choice is a selection a session reads at open, as its environment
//! variable would name it (`KINETO_IMAGE_PROVIDER`, `KINETO_VIDEO_PROVIDER`,
//! `KINETO_SPEECH_PROVIDER`, `KINETO_LLM_PROVIDER`); a variable that is set
//! wins, for development. A connection is any service the director adds by
//! the protocol it speaks, never by vendor: `connection:<name>` chooses it.
//! `key` names a key in the system's secure storage, never the key itself.
//! `[agents]` says where an agent CLI is when it is not where kineto looks.
//! The file belongs to the user, never to a project, holds no secret, and
//! keeps whatever a newer build wrote that this one does not know.
//!
//! The one OpenAI-compatible server an earlier build kept
//! (`[openai_compatible]`, chosen as `openai-compatible`) is read as a
//! connection.

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

/// What a selection starts with when it names a connection.
pub const CONNECTION_PREFIX: &str = "connection:";

/// The longest value a setting may hold.
const MAX_VALUE: usize = 1024;
/// The longest model name a connection may give.
const MAX_MODEL: usize = 200;
/// The most connections the file may hold.
const MAX_CONNECTIONS: usize = 64;
/// The most reference images a connection may say it takes.
pub const MAX_REFERENCES: u16 = 16;
/// The highest price, in USD, a connection may give: anything above is a
/// slip of the keyboard.
const MAX_PRICE: f64 = 10_000.0;

/// What a connection makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modality {
    /// Documents: the language model.
    Text,
    Image,
}

/// The protocol a connection speaks: how requests and answers are shaped,
/// which many vendors share, never who the vendor is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// OpenAI's chat completions (`POST <url>/chat/completions`).
    OpenAiChat,
    /// Anthropic's messages (`POST <url>/v1/messages`).
    AnthropicMessages,
    /// OpenAI's images (`POST <url>/images/generations`, and
    /// `<url>/images/edits` with references).
    OpenAiImages,
    /// Gemini's `generateContent` answering with an image.
    GeminiImages,
}

impl Protocol {
    pub const ALL: [Self; 4] = [
        Self::OpenAiChat,
        Self::AnthropicMessages,
        Self::OpenAiImages,
        Self::GeminiImages,
    ];

    /// Its name in the settings file.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::OpenAiChat => "openai-chat",
            Self::AnthropicMessages => "anthropic-messages",
            Self::OpenAiImages => "openai-images",
            Self::GeminiImages => "gemini-images",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|protocol| protocol.name() == name)
    }

    #[must_use]
    pub const fn modality(self) -> Modality {
        match self {
            Self::OpenAiChat | Self::AnthropicMessages => Modality::Text,
            Self::OpenAiImages | Self::GeminiImages => Modality::Image,
        }
    }
}

/// A service the director added by the protocol it speaks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Connection {
    /// How it is chosen (`connection:<name>`), and the key's name when it
    /// names none.
    pub name: String,
    /// A [`Protocol`] name. One a newer build added is kept, not used.
    pub protocol: String,
    /// Where it is: HTTPS, or HTTP to this computer.
    pub url: String,
    pub model: String,
    /// The name its key is stored under, never the key; none for a service
    /// that takes no key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// What a text connection costs, in USD per million tokens read and
    /// written; unknown when not given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_per_million_input: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_per_million_output: Option<f64>,
    /// What an image connection costs, in USD per image; unknown when not
    /// given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_per_image: Option<f64>,
    /// How many reference images an image connection takes; none, unless
    /// the director says.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub references: u16,
    #[serde(flatten)]
    extra: toml::Table,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_zero(value: &u16) -> bool {
    *value == 0
}

/// `usd` in USD micros, as prices are kept.
#[must_use]
pub fn micros(usd: f64) -> u64 {
    // Checked to be finite, positive and at most `MAX_PRICE` when read.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let micros = (usd * 1_000_000.0).round() as u64;
    micros
}

impl Connection {
    /// A connection named `name` speaking `protocol` at `url` to `model`,
    /// with no key, no price and no references until they are set.
    #[must_use]
    pub fn new(name: &str, protocol: Protocol, url: &str, model: &str) -> Self {
        Self {
            name: name.to_owned(),
            protocol: protocol.name().to_owned(),
            url: url.to_owned(),
            model: model.to_owned(),
            ..Self::default()
        }
    }

    /// The protocol it speaks, when this build knows it.
    #[must_use]
    pub fn protocol(&self) -> Option<Protocol> {
        Protocol::from_name(&self.protocol)
    }

    /// The name its key is stored under, if it takes one.
    #[must_use]
    pub fn key_name(&self) -> Option<&str> {
        self.key.as_deref()
    }

    /// The selection that chooses it.
    #[must_use]
    pub fn selection(&self) -> String {
        format!("{CONNECTION_PREFIX}{}", self.name)
    }

    /// Whether it can be used as it is, and the protocol it speaks.
    pub fn check(&self) -> Result<Protocol, InvalidSetting> {
        let refuse = |why: &str| Err(InvalidSetting(why.to_owned()));
        if !key_name(&self.name) {
            return refuse(
                "a connection's name is lowercase letters, digits and _, such as deepseek",
            );
        }
        let Some(protocol) = self.protocol() else {
            return Err(InvalidSetting(format!(
                "{:?} is not a protocol this version speaks",
                self.protocol
            )));
        };
        if !acceptable_url(&self.url) {
            return refuse("an address must start with https://, or http:// to this computer");
        }
        if protocol == Protocol::GeminiImages && !self.url.starts_with("https://") {
            return refuse("the Gemini protocol is spoken over https:// only");
        }
        if self.model.is_empty()
            || self.model.chars().count() > MAX_MODEL
            || self
                .model
                .chars()
                .any(|character| character.is_control() || character.is_whitespace())
        {
            return refuse("a model is named in one word, such as deepseek-chat");
        }
        if self.key.as_deref().is_some_and(|key| !key_name(key)) {
            return refuse("a key name is lowercase letters, digits and _, such as deepseek");
        }
        let prices = [
            self.price_per_million_input,
            self.price_per_million_output,
            self.price_per_image,
        ];
        if prices
            .into_iter()
            .flatten()
            .any(|price| !price.is_finite() || !(0.0..=MAX_PRICE).contains(&price))
        {
            return refuse("a price is a number of US dollars, 0 or more");
        }
        match protocol.modality() {
            Modality::Text if self.price_per_image.is_some() || self.references > 0 => {
                refuse("a text connection has no price per image and takes no reference images")
            }
            Modality::Image
                if self.price_per_million_input.is_some()
                    || self.price_per_million_output.is_some() =>
            {
                refuse("an image connection is priced per image, not per token")
            }
            Modality::Image if self.references > MAX_REFERENCES => Err(InvalidSetting(format!(
                "a connection takes at most {MAX_REFERENCES} reference images"
            ))),
            _ => Ok(protocol),
        }
    }
}

/// An OpenAI-compatible server an earlier build kept, read only to become
/// a connection.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Endpoint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
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
    /// Kept only while it cannot become a connection: without an address
    /// or a model it was never usable.
    #[serde(default, skip_serializing_if = "Endpoint::is_empty")]
    openai_compatible: Endpoint,
    /// Where an agent CLI is, by agent name, when it is not where kineto
    /// looks.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<String, PathBuf>,
    /// The services the director added, in the order they were added.
    #[serde(default, rename = "connection", skip_serializing_if = "Vec::is_empty")]
    connections: Vec<Connection>,
    #[serde(flatten)]
    extra: toml::Table,
}

/// One choice, as settings change them one at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Image,
    Video,
    Speech,
    LanguageModel,
}

impl Setting {
    /// What a connection chosen here must make; `None` where no connection
    /// can be chosen.
    #[must_use]
    pub const fn modality(self) -> Option<Modality> {
        match self {
            Self::Image => Some(Modality::Image),
            Self::LanguageModel => Some(Modality::Text),
            Self::Video | Self::Speech => None,
        }
    }
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

/// Whether `name` can name a connection or the key a service takes: 1 to
/// 64 lowercase ASCII letters, digits and `_`, not starting with a digit,
/// as provider keys are named, so the environment's `KINETO_<NAME>_API_KEY`
/// can stand in for it.
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
    /// Read settings from TOML text. The server an earlier build kept
    /// becomes a connection.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let mut settings: Self =
            toml::from_str(text).map_err(|error| ConfigError::Unreadable(error.to_string()))?;
        settings.adopt_endpoint();
        Ok(settings)
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

    /// The server an earlier build kept, as a connection named after its
    /// key, and the choice of it as that connection's.
    fn adopt_endpoint(&mut self) {
        let (Some(url), Some(model)) = (
            self.openai_compatible.url.clone(),
            self.openai_compatible.model.clone(),
        ) else {
            return;
        };
        let old = std::mem::take(&mut self.openai_compatible);
        let base = old
            .key
            .clone()
            .filter(|key| key_name(key))
            .unwrap_or_else(|| "server".to_owned());
        let name = (1..)
            .map(|n| {
                if n == 1 {
                    base.clone()
                } else {
                    format!("{base}_{n}")
                }
            })
            .find(|name| self.connection(name).is_none())
            .expect("some name is free");
        let connection = Connection {
            name,
            protocol: Protocol::OpenAiChat.name().to_owned(),
            url,
            model,
            key: old.key,
            extra: old.extra,
            ..Connection::default()
        };
        if matches!(
            self.language_model.as_deref(),
            Some("openai-compatible" | "local")
        ) {
            self.language_model = Some(connection.selection());
        }
        self.connections.push(connection);
    }

    /// The value of `setting`, `None` when it is not chosen.
    #[must_use]
    pub fn get(&self, setting: Setting) -> Option<&str> {
        self.slot(setting).as_deref()
    }

    fn slot(&self, setting: Setting) -> &Option<String> {
        match setting {
            Setting::Image => &self.image,
            Setting::Video => &self.video,
            Setting::Speech => &self.speech,
            Setting::LanguageModel => &self.language_model,
        }
    }

    fn slot_mut(&mut self, setting: Setting) -> &mut Option<String> {
        match setting {
            Setting::Image => &mut self.image,
            Setting::Video => &mut self.video,
            Setting::Speech => &mut self.speech,
            Setting::LanguageModel => &mut self.language_model,
        }
    }

    /// Choose `value` for `setting`; empty clears it. A connection must be
    /// one the settings hold, and make what `setting` is for.
    pub fn set(&mut self, setting: Setting, value: &str) -> Result<(), InvalidSetting> {
        let value = value.trim();
        if value.len() > MAX_VALUE || value.chars().any(char::is_control) {
            return Err(InvalidSetting(
                "the value is too long or not plain text".to_owned(),
            ));
        }
        if let Some(name) = value.strip_prefix(CONNECTION_PREFIX) {
            let makes = self
                .connection(name)
                .and_then(Connection::protocol)
                .map(Protocol::modality);
            match (makes, setting.modality()) {
                (None, _) => {
                    return Err(InvalidSetting(format!(
                        "there is no connection named {name:?}"
                    )));
                }
                (Some(makes), Some(wanted)) if makes == wanted => {}
                _ => {
                    return Err(InvalidSetting(format!(
                        "the connection {name:?} does not make what this is for"
                    )));
                }
            }
        }
        *self.slot_mut(setting) = (!value.is_empty()).then(|| value.to_owned());
        Ok(())
    }

    /// Every connection, in the order they were added.
    #[must_use]
    pub fn connections(&self) -> &[Connection] {
        &self.connections
    }

    /// The connection named `name`.
    #[must_use]
    pub fn connection(&self, name: &str) -> Option<&Connection> {
        self.connections
            .iter()
            .find(|connection| connection.name == name)
    }

    /// Add `connection`, or put it in place of the one named `previous`,
    /// keeping what a newer build wrote there. A choice of the one replaced
    /// follows it to its new name, and is cleared if it no longer makes
    /// what the choice is for.
    pub fn put_connection(
        &mut self,
        previous: Option<&str>,
        mut connection: Connection,
    ) -> Result<(), InvalidSetting> {
        let protocol = connection.check()?;
        let clash = |name: &str| {
            Err(InvalidSetting(format!(
                "there is already a connection named {name:?}"
            )))
        };
        match previous {
            Some(previous) => {
                let Some(index) = self
                    .connections
                    .iter()
                    .position(|existing| existing.name == previous)
                else {
                    return Err(InvalidSetting(format!(
                        "there is no connection named {previous:?}"
                    )));
                };
                if connection.name != previous && self.connection(&connection.name).is_some() {
                    return clash(&connection.name);
                }
                connection.extra = std::mem::take(&mut self.connections[index].extra);
                let old = format!("{CONNECTION_PREFIX}{previous}");
                let new = connection.selection();
                self.connections[index] = connection;
                for setting in [Setting::Image, Setting::LanguageModel] {
                    let slot = self.slot_mut(setting);
                    if slot.as_deref() == Some(old.as_str()) {
                        *slot =
                            (setting.modality() == Some(protocol.modality())).then(|| new.clone());
                    }
                }
            }
            None => {
                if self.connection(&connection.name).is_some() {
                    return clash(&connection.name);
                }
                if self.connections.len() >= MAX_CONNECTIONS {
                    return Err(InvalidSetting(format!(
                        "the settings hold at most {MAX_CONNECTIONS} connections"
                    )));
                }
                self.connections.push(connection);
            }
        }
        Ok(())
    }

    /// Remove the connection named `name`, and any choice of it. Its key
    /// stays where it is kept.
    pub fn remove_connection(&mut self, name: &str) -> Result<(), InvalidSetting> {
        let before = self.connections.len();
        self.connections
            .retain(|connection| connection.name != name);
        if self.connections.len() == before {
            return Err(InvalidSetting(format!(
                "there is no connection named {name:?}"
            )));
        }
        let chosen = format!("{CONNECTION_PREFIX}{name}");
        for setting in [Setting::Image, Setting::LanguageModel] {
            let slot = self.slot_mut(setting);
            if slot.as_deref() == Some(chosen.as_str()) {
                *slot = None;
            }
        }
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

    fn connection(name: &str, protocol: Protocol, url: &str, model: &str) -> Connection {
        Connection::new(name, protocol, url, model)
    }

    #[test]
    fn choices_and_connections_are_read_and_written_with_what_a_newer_build_added() {
        let settings = ProviderSettings::parse(
            r#"
image = "connection:seedream"
language_model = "connection:deepseek"
future = "kept"

[[connection]]
name = "deepseek"
protocol = "openai-chat"
url = "https://api.deepseek.example/v1"
model = "deepseek-chat"
key = "deepseek"
price_per_million_input = 0.28
tier = "kept too"

[[connection]]
name = "seedream"
protocol = "openai-images"
url = "https://ark.example/api/v3"
model = "seedream-4-0"
price_per_image = 0.03
references = 4

[[connection]]
name = "later"
protocol = "a-protocol-from-a-newer-build"
url = "https://later.example"
model = "m"

[agents]
claude-code = "/opt/claude/bin/claude"
"#,
        )
        .unwrap();
        assert_eq!(settings.get(Setting::Image), Some("connection:seedream"));
        assert_eq!(settings.get(Setting::Video), None);
        let deepseek = settings.connection("deepseek").unwrap();
        assert_eq!(deepseek.check(), Ok(Protocol::OpenAiChat));
        assert_eq!(deepseek.key_name(), Some("deepseek"));
        assert_eq!(deepseek.price_per_million_input, Some(0.28));
        let seedream = settings.connection("seedream").unwrap();
        assert_eq!(seedream.check(), Ok(Protocol::OpenAiImages));
        assert_eq!((seedream.references, seedream.key_name()), (4, None));
        assert_eq!(settings.connection("later").unwrap().protocol(), None);
        assert_eq!(
            settings.agent_executable("claude-code"),
            Some(Path::new("/opt/claude/bin/claude"))
        );
        let written = toml::to_string(&settings).unwrap();
        assert!(written.contains("future = \"kept\""), "{written}");
        assert!(written.contains("tier = \"kept too\""), "{written}");
        assert!(
            written.contains("a-protocol-from-a-newer-build"),
            "{written}"
        );
        assert_eq!(ProviderSettings::parse(&written).unwrap(), settings);
    }

    #[test]
    fn the_server_an_earlier_build_kept_becomes_a_connection() {
        let settings = ProviderSettings::parse(
            r#"
language_model = "openai-compatible"

[openai_compatible]
url = "https://api.deepseek.example/v1"
model = "deepseek-chat"
key = "deepseek"
tier = "kept"
"#,
        )
        .unwrap();
        assert_eq!(
            settings.get(Setting::LanguageModel),
            Some("connection:deepseek")
        );
        let adopted = settings.connection("deepseek").unwrap();
        assert_eq!(adopted.check(), Ok(Protocol::OpenAiChat));
        assert_eq!(adopted.url, "https://api.deepseek.example/v1");
        assert_eq!(adopted.key_name(), Some("deepseek"));
        let written = toml::to_string(&settings).unwrap();
        assert!(!written.contains("openai_compatible"), "{written}");
        assert!(written.contains("tier = \"kept\""), "{written}");

        // Keyless, under a name already taken, and chosen as `local`.
        let settings = ProviderSettings::parse(
            r#"
language_model = "local"

[openai_compatible]
url = "http://localhost:11434/v1"
model = "llama"

[[connection]]
name = "server"
protocol = "openai-chat"
url = "https://other.example/v1"
model = "other"
"#,
        )
        .unwrap();
        assert_eq!(
            settings.get(Setting::LanguageModel),
            Some("connection:server_2")
        );
        assert_eq!(settings.connection("server_2").unwrap().key_name(), None);

        // Never usable without a model: kept as it was, not adopted.
        let settings =
            ProviderSettings::parse("[openai_compatible]\nurl = \"https://x.example/v1\"\n")
                .unwrap();
        assert!(settings.connections().is_empty());
        assert!(
            toml::to_string(&settings)
                .unwrap()
                .contains("openai_compatible")
        );
    }

    #[test]
    fn a_connection_is_checked_before_it_is_kept() {
        let good = connection(
            "deepseek",
            Protocol::OpenAiChat,
            "https://api.example.test/v1",
            "deepseek-chat",
        );
        assert_eq!(good.check(), Ok(Protocol::OpenAiChat));
        for url in [
            "https://api.example.test/v1",
            "http://localhost:11434/v1",
            "http://127.0.0.1:8080",
            "http://127.0.0.2:8080",
            "http://[::1]:8080/v1",
        ] {
            let mut local = good.clone();
            local.url = url.to_owned();
            assert!(local.check().is_ok(), "{url}");
        }
        let mut refused = Vec::new();
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
            let mut bad = good.clone();
            bad.url = url.to_owned();
            refused.push(bad);
        }
        for name in ["sk-live-123", "deep seek", "1key", "OPENROUTER", ""] {
            let mut bad = good.clone();
            bad.name = name.to_owned();
            refused.push(bad.clone());
            bad.name = "fine".to_owned();
            bad.key = Some(name.to_owned());
            refused.push(bad);
        }
        for model in ["", "two words", "line\nbreak"] {
            let mut bad = good.clone();
            bad.model = model.to_owned();
            refused.push(bad);
        }
        let mut bad = good.clone();
        bad.protocol = "telepathy".to_owned();
        refused.push(bad);
        for price in [-1.0, f64::NAN, f64::INFINITY, 20_000.0] {
            let mut bad = good.clone();
            bad.price_per_million_output = Some(price);
            refused.push(bad);
        }
        let mut bad = good.clone();
        bad.price_per_image = Some(0.04);
        refused.push(bad);
        let mut bad = good.clone();
        bad.references = 2;
        refused.push(bad);

        let image = connection(
            "gemini",
            Protocol::GeminiImages,
            "https://generativelanguage.example/v1beta",
            "gemini-image",
        );
        assert_eq!(image.check(), Ok(Protocol::GeminiImages));
        let mut bad = image.clone();
        bad.url = "http://localhost:8080".to_owned();
        refused.push(bad);
        let mut bad = image.clone();
        bad.references = MAX_REFERENCES + 1;
        refused.push(bad);
        let mut bad = image;
        bad.price_per_million_input = Some(1.0);
        refused.push(bad);

        for bad in refused {
            assert!(bad.check().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn connections_are_added_renamed_and_removed_with_their_choices() {
        let mut settings = ProviderSettings::default();
        let text = connection(
            "glm",
            Protocol::OpenAiChat,
            "https://glm.example/v4",
            "glm-5",
        );
        settings.put_connection(None, text.clone()).unwrap();
        assert!(
            settings.put_connection(None, text.clone()).is_err(),
            "two connections never share a name"
        );
        assert!(
            settings.set(Setting::Image, "connection:glm").is_err(),
            "it makes no images"
        );
        assert!(settings.set(Setting::Video, "connection:glm").is_err());
        assert!(
            settings
                .set(Setting::LanguageModel, "connection:none")
                .is_err()
        );
        settings
            .set(Setting::LanguageModel, "connection:glm")
            .unwrap();

        let mut renamed = text.clone();
        renamed.name = "zhipu".to_owned();
        settings.put_connection(Some("glm"), renamed).unwrap();
        assert_eq!(
            settings.get(Setting::LanguageModel),
            Some("connection:zhipu")
        );
        assert!(settings.connection("glm").is_none());
        assert!(settings.put_connection(Some("glm"), text.clone()).is_err());

        let images = connection(
            "grok",
            Protocol::OpenAiImages,
            "https://api.x.example/v1",
            "grok-imagine-image",
        );
        settings.put_connection(None, images).unwrap();
        settings.set(Setting::Image, "connection:grok").unwrap();
        let mut clash = text.clone();
        clash.name = "grok".to_owned();
        assert!(settings.put_connection(Some("zhipu"), clash).is_err());

        // Made into an image connection, it is no longer the language model.
        let mut changed = connection(
            "zhipu",
            Protocol::OpenAiImages,
            "https://glm.example/v4",
            "cogview-4",
        );
        changed.references = 1;
        settings.put_connection(Some("zhipu"), changed).unwrap();
        assert_eq!(settings.get(Setting::LanguageModel), None);

        settings.remove_connection("grok").unwrap();
        assert_eq!(settings.get(Setting::Image), None);
        assert!(settings.remove_connection("grok").is_err());
        assert_eq!(settings.connections().len(), 1);
    }

    #[test]
    fn a_choice_is_changed_cleared_or_refused() {
        let mut settings = ProviderSettings::default();
        settings.set(Setting::LanguageModel, " codex ").unwrap();
        assert_eq!(settings.get(Setting::LanguageModel), Some("codex"));
        settings.set(Setting::LanguageModel, "").unwrap();
        assert_eq!(settings.get(Setting::LanguageModel), None);
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
        let mut local = connection(
            "ollama",
            Protocol::OpenAiChat,
            "http://localhost:11434/v1",
            "llama",
        );
        local.price_per_million_output = Some(0.0);
        settings.put_connection(None, local).unwrap();
        settings.save(&path).unwrap();
        assert_eq!(ProviderSettings::load(&path).unwrap(), settings);
        assert!(!path.with_extension("toml.kineto-tmp").exists());
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn prices_are_kept_in_micros() {
        assert_eq!(micros(0.04), 40_000);
        assert_eq!(micros(0.0035), 3_500);
        assert_eq!(micros(2.5), 2_500_000);
    }
}
