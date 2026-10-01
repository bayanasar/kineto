//! The language model a session writes documents with.
//!
//! Chosen when a session opens, from `KINETO_LLM_PROVIDER`, the way the image
//! provider is: no model is asked anything unless it was named.
//!
//! - unset, empty or `none`: no model; documents cannot be generated
//! - `kimi-code`: Kimi Code, driven over ACP under kineto's own data root
//!   (`KINETO_KIMI_CODE_HOME`, by default `agents/kimi-code` in kineto's
//!   state directory); the CLI is `KINETO_KIMI_CODE_BIN`, otherwise `kimi`
//!   on `PATH`
//! - `kimi`: Kimi's API with the key in `KINETO_KIMI_API_KEY`, and the model
//!   in `KINETO_KIMI_MODEL` when set
//! - `local`: an OpenAI-compatible server at `KINETO_LOCAL_LLM_URL` serving
//!   `KINETO_LOCAL_LLM_MODEL`

use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};

use kineto_jobs::ErrorClass;
use kineto_llm::{
    Billing, LlmBackend, LlmBackendInfo, LlmRequest, LlmResponse,
    acp::{AcpAgent, KIMI_CODE_PROVIDER},
    openai::{KIMI_PROVIDER, LOCAL_PROVIDER, OpenAiCompatible},
};
use kineto_providers::{
    LlmCapabilities,
    secret::{SecretError, SecretSource},
};

pub const LLM_PROVIDER_VARIABLE: &str = "KINETO_LLM_PROVIDER";
pub const KIMI_CODE_BIN_VARIABLE: &str = "KINETO_KIMI_CODE_BIN";
pub const KIMI_CODE_HOME_VARIABLE: &str = "KINETO_KIMI_CODE_HOME";
pub const KIMI_MODEL_VARIABLE: &str = "KINETO_KIMI_MODEL";
pub const LOCAL_URL_VARIABLE: &str = "KINETO_LOCAL_LLM_URL";
pub const LOCAL_MODEL_VARIABLE: &str = "KINETO_LOCAL_LLM_MODEL";

/// Which kind of model a session has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentModelKind {
    None,
    KimiCode,
    Kimi,
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
    /// Kimi Code; `None` when its CLI was not found.
    KimiCode(Option<AcpAgent>),
    /// Kimi's API; `None` without a key.
    Kimi(Option<OpenAiCompatible>),
    /// A local server; `None` when its address or model is missing.
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
            Some(KIMI_CODE_PROVIDER) => {
                let executable = setting(KIMI_CODE_BIN_VARIABLE)
                    .map(PathBuf::from)
                    .or_else(|| find_on_path("kimi", env("PATH").as_deref()));
                let home = setting(KIMI_CODE_HOME_VARIABLE)
                    .map(PathBuf::from)
                    .or_else(|| state_directory(&env).map(|state| state.join("agents/kimi-code")));
                Self::KimiCode(
                    executable
                        .filter(|executable| executable.is_file())
                        .zip(home)
                        .map(|(executable, home)| AcpAgent::kimi_code(executable, home)),
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
            Some("local") => Self::Local(
                setting(LOCAL_URL_VARIABLE)
                    .zip(setting(LOCAL_MODEL_VARIABLE))
                    .map(|(url, model)| OpenAiCompatible::local(&url, &model)),
            ),
            Some(other) => return Err(ModelSelectionError::Unknown(other.to_owned())),
        })
    }

    /// Stop an agent call in progress once `cancel` is set, so closing a
    /// project does not wait for a long answer.
    #[must_use]
    pub fn cancelled_by(self, cancel: Arc<AtomicBool>) -> Self {
        match self {
            Self::KimiCode(Some(agent)) => Self::KimiCode(Some(agent.with_cancel(cancel))),
            other => other,
        }
    }

    #[must_use]
    pub fn describe(&self) -> DocumentModelInfo {
        let (kind, ready) = match self {
            Self::None => (DocumentModelKind::None, false),
            Self::KimiCode(agent) => (DocumentModelKind::KimiCode, agent.is_some()),
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
            Self::KimiCode(_) => KIMI_CODE_PROVIDER,
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
        message: error.to_string(),
    })
}

impl LlmBackend for DocumentModel {
    type Error = ModelError;

    fn info(&self) -> LlmBackendInfo {
        match self {
            Self::KimiCode(Some(agent)) => agent.info(),
            Self::Kimi(Some(api)) | Self::Local(Some(api)) => api.info(),
            #[cfg(any(test, feature = "testing"))]
            Self::Fake(fake) => fake.info(),
            _ => self.unavailable(),
        }
    }

    fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, ModelError> {
        match self {
            Self::KimiCode(Some(agent)) => forward(agent, request),
            Self::Kimi(Some(api)) | Self::Local(Some(api)) => forward(api, request),
            #[cfg(any(test, feature = "testing"))]
            Self::Fake(fake) => forward(fake, request),
            _ => Err(ModelError {
                message: "no language model is configured".to_owned(),
                class: ErrorClass::Terminal,
            }),
        }
    }

    fn classify(&self, error: &ModelError) -> ErrorClass {
        error.class
    }
}

fn find_on_path(name: &str, path: Option<&str>) -> Option<PathBuf> {
    let names: &[String] = &if cfg!(windows) {
        vec![format!("{name}.exe"), format!("{name}.cmd")]
    } else {
        vec![name.to_owned()]
    };
    std::env::split_paths(path?)
        .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
        .find(|candidate| candidate.is_file())
}

/// Kineto's per-user state directory, as the desktop app places it.
fn state_directory(env: &impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let set = |name: &str| env(name).filter(|value| !value.is_empty());
    if cfg!(windows) {
        set("LOCALAPPDATA").map(|base| Path::new(&base).join("Kineto"))
    } else if cfg!(target_os = "macos") {
        set("HOME").map(|home| Path::new(&home).join("Library/Application Support/Kineto"))
    } else {
        set("XDG_STATE_HOME")
            .map(|state| Path::new(&state).join("kineto"))
            .or_else(|| set("HOME").map(|home| Path::new(&home).join(".local/state/kineto")))
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
    fn kimi_code_needs_its_cli_and_a_data_root() {
        let directory =
            std::env::temp_dir().join(format!("kineto-llm-select-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let cli = directory.join("kimi");
        std::fs::write(&cli, "").unwrap();
        let keys = Keys(BTreeMap::new());

        let path = directory.to_string_lossy().into_owned();
        let found = DocumentModel::select(
            Some("kimi-code"),
            &keys,
            env(&[("PATH", &path), ("HOME", "/home/director")]),
        )
        .unwrap();
        let info = found.describe();
        assert_eq!((info.kind, info.ready), (DocumentModelKind::KimiCode, true));
        assert_eq!(
            info.billing,
            Billing::Subscription,
            "an agent's cost is not known"
        );

        let missing =
            DocumentModel::select(Some("kimi-code"), &keys, env(&[("HOME", "/home/director")]))
                .unwrap();
        assert!(!missing.describe().ready);
        let local = DocumentModel::select(
            Some("local"),
            &keys,
            env(&[(LOCAL_URL_VARIABLE, "http://127.0.0.1:8080/v1")]),
        )
        .unwrap();
        assert!(!local.describe().ready, "a local model needs a model name");
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn kineto_keeps_its_agent_data_in_its_own_state_directory() {
        if cfg!(windows) || cfg!(target_os = "macos") {
            return;
        }
        assert_eq!(
            state_directory(&env(&[("HOME", "/home/director")])),
            Some(PathBuf::from("/home/director/.local/state/kineto"))
        );
        assert_eq!(
            state_directory(&env(&[
                ("HOME", "/home/director"),
                ("XDG_STATE_HOME", "/state")
            ])),
            Some(PathBuf::from("/state/kineto"))
        );
    }
}
