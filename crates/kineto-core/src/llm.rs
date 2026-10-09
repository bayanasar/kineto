//! The language model a session writes documents with.
//!
//! Chosen when a session opens, from `KINETO_LLM_PROVIDER`, the way the image
//! provider is: no model is asked anything unless it was named.
//!
//! - unset, empty or `none`: no model; documents cannot be generated
//! - `claude-code`, `codex`, `kimi-code`, `agy`, `mini-agent`: the agent CLI
//!   the director already installed and signed in to
//!   ([`kineto_llm::agents`]), found on the search path or where it is
//!   usually installed, or at `KINETO_<AGENT>_BIN` (`KINETO_CLAUDE_CODE_BIN`,
//!   `KINETO_MINI_AGENT_BIN` for the Python that runs Mini-Agent)
//! - `kimi`: Kimi's API with the key in `KINETO_KIMI_API_KEY`, and the model
//!   in `KINETO_KIMI_MODEL` when set
//! - `local` (or `openai-compatible`): any server that speaks the OpenAI
//!   chat-completions protocol, at `KINETO_LOCAL_LLM_URL` serving
//!   `KINETO_LOCAL_LLM_MODEL`; with `KINETO_LOCAL_LLM_KEY` naming the key it
//!   takes (`deepseek` reads the key stored as `deepseek`, or
//!   `KINETO_DEEPSEEK_API_KEY`). HTTPS, or HTTP to a loopback address only.
//!
//! Each value is read from the director's provider settings first and the
//! environment second; the desktop shell decides which.

use std::{
    error::Error,
    fmt,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

use kineto_jobs::ErrorClass;
use kineto_llm::{
    Billing, LlmBackend, LlmBackendInfo, LlmRequest, LlmResponse,
    agents::{AgentCli, AgentKind, locate},
    openai::{KIMI_PROVIDER, LOCAL_PROVIDER, OpenAiCompatible},
};
use kineto_providers::{
    LlmCapabilities,
    secret::{SecretError, SecretSource},
};

pub const LLM_PROVIDER_VARIABLE: &str = "KINETO_LLM_PROVIDER";
pub const KIMI_MODEL_VARIABLE: &str = "KINETO_KIMI_MODEL";
pub const LOCAL_URL_VARIABLE: &str = "KINETO_LOCAL_LLM_URL";
pub const LOCAL_MODEL_VARIABLE: &str = "KINETO_LOCAL_LLM_MODEL";
/// Names the key an OpenAI-compatible server takes, as it is stored.
pub const LOCAL_KEY_VARIABLE: &str = "KINETO_LOCAL_LLM_KEY";

/// The variable that names where `agent` is installed, overriding where
/// kineto would look: `KINETO_CLAUDE_CODE_BIN` for `claude-code`.
#[must_use]
pub fn agent_executable_variable(agent: AgentKind) -> String {
    format!(
        "KINETO_{}_BIN",
        agent.name().to_ascii_uppercase().replace('-', "_")
    )
}

/// Which kind of model a session has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentModelKind {
    None,
    /// A signed-in agent CLI.
    Agent(AgentKind),
    Kimi,
    /// An OpenAI-compatible server.
    Local,
}

/// What the director is told about the session's model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentModelInfo {
    pub kind: DocumentModelKind,
    /// Whether a document can be asked for at all.
    pub ready: bool,
    /// The model, as far as kineto knows it; an agent uses its own default.
    pub model: String,
    pub billing: Billing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelSelectionError {
    /// The selection names no model this build has.
    Unknown(String),
    Secret(SecretError),
}

impl fmt::Display for ModelSelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(name) => write!(formatter, "unknown language model provider {name:?}"),
            Self::Secret(error) => error.fmt(formatter),
        }
    }
}

impl Error for ModelSelectionError {}

/// The model a session writes documents with.
#[derive(Debug)]
pub enum DocumentModel {
    None,
    /// A signed-in agent CLI; `None` when it is not installed.
    Agent(AgentKind, Option<AgentCli>),
    /// Kimi's API; `None` without a key.
    Kimi(Option<OpenAiCompatible>),
    /// An OpenAI-compatible server; `None` when its address or model is
    /// missing, or the key it names is not stored.
    Local(Option<OpenAiCompatible>),
    #[cfg(any(test, feature = "testing"))]
    Fake(kineto_llm::fake::FakeLlm),
}

impl DocumentModel {
    /// The model `selection` names, configured from `env` and `secrets`.
    pub fn select(
        selection: Option<&str>,
        secrets: &impl SecretSource,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ModelSelectionError> {
        let setting = |name: &str| env(name).filter(|value| !value.trim().is_empty());
        Ok(match selection.map(str::trim) {
            None | Some("" | "none") => Self::None,
            Some(name) if AgentKind::from_name(name).is_some() => {
                let agent = AgentKind::from_name(name).expect("checked above");
                let executable = setting(&agent_executable_variable(agent))
                    .map(PathBuf::from)
                    .or_else(|| locate(agent, &env));
                Self::Agent(
                    agent,
                    executable
                        .filter(|executable| executable.is_file())
                        .map(|executable| AgentCli::new(agent, executable)),
                )
            }
            Some(KIMI_PROVIDER) => Self::Kimi(
                secrets
                    .api_key(KIMI_PROVIDER)
                    .map_err(ModelSelectionError::Secret)?
                    .map(|key| {
                        OpenAiCompatible::kimi(key, setting(KIMI_MODEL_VARIABLE).as_deref(), None)
                    }),
            ),
            Some("local" | "openai-compatible") => {
                // A key that is named but not stored leaves the server not
                // ready, rather than asked without the key it needs.
                let key = match setting(LOCAL_KEY_VARIABLE) {
                    Some(name) => secrets
                        .api_key(name.trim())
                        .map_err(ModelSelectionError::Secret)?
                        .map(Some),
                    None => Some(None),
                };
                Self::Local(
                    setting(LOCAL_URL_VARIABLE)
                        .zip(setting(LOCAL_MODEL_VARIABLE))
                        .zip(key)
                        .map(|((url, model), key)| match key {
                            Some(key) => OpenAiCompatible::endpoint(&url, &model, key),
                            None => OpenAiCompatible::local(&url, &model),
                        }),
                )
            }
            Some(other) => return Err(ModelSelectionError::Unknown(other.to_owned())),
        })
    }

    /// Stop an agent call in progress once `cancel` is set, so closing a
    /// project does not wait for a long answer.
    #[must_use]
    pub fn cancelled_by(self, cancel: Arc<AtomicBool>) -> Self {
        match self {
            Self::Agent(kind, Some(agent)) => Self::Agent(kind, Some(agent.with_cancel(cancel))),
            other => other,
        }
    }

    #[must_use]
    pub fn describe(&self) -> DocumentModelInfo {
        let (kind, ready) = match self {
            Self::None => (DocumentModelKind::None, false),
            Self::Agent(kind, agent) => (DocumentModelKind::Agent(*kind), agent.is_some()),
            Self::Kimi(api) => (DocumentModelKind::Kimi, api.is_some()),
            Self::Local(api) => (DocumentModelKind::Local, api.is_some()),
            #[cfg(any(test, feature = "testing"))]
            Self::Fake(_) => (DocumentModelKind::Local, true),
        };
        let info = self.info();
        DocumentModelInfo {
            kind,
            ready,
            model: info.model,
            billing: info.billing,
        }
    }

    fn unavailable(&self) -> LlmBackendInfo {
        let provider = match self {
            Self::Agent(kind, _) => kind.name(),
            Self::Kimi(_) => KIMI_PROVIDER,
            Self::Local(_) => LOCAL_PROVIDER,
            _ => "none",
        };
        LlmBackendInfo {
            provider: provider.to_owned(),
            model: String::new(),
            capabilities: LlmCapabilities {
                context_window_tokens: None,
                tools: false,
                structured_output: false,
                vision_input: false,
            },
            billing: Billing::Unpriced,
        }
    }
}

/// Why a model call failed, with its class kept from the backend.
#[derive(Debug)]
pub struct ModelError {
    message: String,
    class: ErrorClass,
    lost_answer: bool,
}

impl fmt::Display for ModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ModelError {}

fn forward<B: LlmBackend>(backend: &B, request: &LlmRequest) -> Result<LlmResponse, ModelError>
where
    B::Error: fmt::Display,
{
    backend.complete(request).map_err(|error| ModelError {
        class: backend.classify(&error),
        lost_answer: backend.may_have_lost_an_answer(&error),
        message: error.to_string(),
    })
}

impl LlmBackend for DocumentModel {
    type Error = ModelError;

    fn info(&self) -> LlmBackendInfo {
        match self {
            Self::Agent(_, Some(agent)) => agent.info(),
            Self::Kimi(Some(api)) | Self::Local(Some(api)) => api.info(),
            #[cfg(any(test, feature = "testing"))]
            Self::Fake(fake) => fake.info(),
            _ => self.unavailable(),
        }
    }

    fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, ModelError> {
        match self {
            Self::Agent(_, Some(agent)) => forward(agent, request),
            Self::Kimi(Some(api)) | Self::Local(Some(api)) => forward(api, request),
            #[cfg(any(test, feature = "testing"))]
            Self::Fake(fake) => forward(fake, request),
            _ => Err(ModelError {
                message: "no language model is configured".to_owned(),
                class: ErrorClass::Terminal,
                lost_answer: false,
            }),
        }
    }

    fn classify(&self, error: &ModelError) -> ErrorClass {
        error.class
    }

    fn may_have_lost_an_answer(&self, error: &ModelError) -> bool {
        error.lost_answer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kineto_providers::secret::SecretString;
    use std::collections::BTreeMap;

    struct Keys(BTreeMap<&'static str, &'static str>);

    impl SecretSource for Keys {
        fn api_key(&self, provider: &str) -> Result<Option<SecretString>, SecretError> {
            Ok(self
                .0
                .get(provider)
                .and_then(|key| SecretString::new((*key).to_owned())))
        }
    }

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let pairs: BTreeMap<String, String> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        move |name| pairs.get(name).cloned()
    }

    #[test]
    fn a_model_is_used_only_when_named() {
        let keys = Keys(BTreeMap::from([("kimi", "sk-test")]));
        for selection in [None, Some(""), Some("none")] {
            let model = DocumentModel::select(selection, &keys, env(&[])).unwrap();
            assert_eq!(model.describe().kind, DocumentModelKind::None);
            assert!(!model.describe().ready);
        }
        let kimi = DocumentModel::select(Some("kimi"), &keys, env(&[])).unwrap();
        let info = kimi.describe();
        assert_eq!((info.kind, info.ready), (DocumentModelKind::Kimi, true));
        assert_eq!(info.model, "kimi-k3");
        assert!(matches!(info.billing, Billing::PerToken { .. }));

        let keyless =
            DocumentModel::select(Some("kimi"), &Keys(BTreeMap::new()), env(&[])).unwrap();
        assert!(!keyless.describe().ready);
        assert!(matches!(
            DocumentModel::select(Some("gpt"), &keys, env(&[])),
            Err(ModelSelectionError::Unknown(_))
        ));
    }

    #[test]
    fn an_agent_is_the_cli_the_director_installed() {
        let directory =
            std::env::temp_dir().join(format!("kineto-llm-select-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let cli = directory.join(if cfg!(windows) {
            "claude.exe"
        } else {
            "claude"
        });
        std::fs::write(&cli, "").unwrap();
        let elsewhere = directory.join("my-codex");
        std::fs::write(&elsewhere, "").unwrap();
        let keys = Keys(BTreeMap::new());

        let path = directory.to_string_lossy().into_owned();
        let found = DocumentModel::select(
            Some("claude-code"),
            &keys,
            env(&[("PATH", &path), ("HOME", "/nonexistent")]),
        )
        .unwrap();
        let info = found.describe();
        assert_eq!(
            (info.kind, info.ready),
            (DocumentModelKind::Agent(AgentKind::ClaudeCode), true)
        );
        assert_eq!(
            info.billing,
            Billing::Subscription,
            "an agent's cost is not known"
        );

        let named = elsewhere.to_string_lossy().into_owned();
        let codex = DocumentModel::select(
            Some("codex"),
            &keys,
            env(&[("KINETO_CODEX_BIN", &named), ("HOME", "/nonexistent")]),
        )
        .unwrap();
        assert!(
            codex.describe().ready,
            "found where the director said it is"
        );

        for agent in ["kimi-code", "agy", "mini-agent"] {
            let missing =
                DocumentModel::select(Some(agent), &keys, env(&[("HOME", "/nonexistent")]))
                    .unwrap();
            assert!(!missing.describe().ready, "{agent} is not installed");
            assert_eq!(missing.info().provider, agent);
        }
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn an_openai_compatible_server_takes_the_key_it_names() {
        let keys = Keys(BTreeMap::from([("deepseek", "sk-test")]));
        let server = [
            (LOCAL_URL_VARIABLE, "https://api.deepseek.example/v1"),
            (LOCAL_MODEL_VARIABLE, "deepseek-chat"),
            (LOCAL_KEY_VARIABLE, "deepseek"),
        ];
        let keyed = DocumentModel::select(Some("openai-compatible"), &keys, env(&server)).unwrap();
        let info = keyed.describe();
        assert_eq!((info.kind, info.ready), (DocumentModelKind::Local, true));
        assert_eq!(info.model, "deepseek-chat");
        assert_eq!(info.billing, Billing::Unpriced, "a keyed server is metered");

        let unstored =
            DocumentModel::select(Some("local"), &Keys(BTreeMap::new()), env(&server)).unwrap();
        assert!(
            !unstored.describe().ready,
            "a key that is named but not stored is never left out"
        );

        let keyless = DocumentModel::select(
            Some("local"),
            &keys,
            env(&[
                (LOCAL_URL_VARIABLE, "http://127.0.0.1:11434/v1"),
                (LOCAL_MODEL_VARIABLE, "llama"),
            ]),
        )
        .unwrap();
        assert_eq!(keyless.describe().billing, Billing::Free);
        let local = DocumentModel::select(
            Some("local"),
            &keys,
            env(&[(LOCAL_URL_VARIABLE, "http://127.0.0.1:8080/v1")]),
        )
        .unwrap();
        assert!(!local.describe().ready, "a local model needs a model name");
    }
}
