use std::{error::Error, fmt};

use sha2::{Digest, Sha256};

const PROMPT_HASH_DOMAIN: &[u8] = b"kineto.prompt_hash.v1";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PromptHash(String);

impl PromptHash {
    /// Derive the canonical provenance hash for one compiled provider input.
    ///
    /// `canonical_intent` must be a deterministic canonical encoding of the
    /// structured intent. Callers must not pass `Debug` output or an
    /// order-unstable map serialization here. The hash is domain-separated and
    /// length-framed so field boundaries cannot alias each other.
    #[must_use]
    pub fn derive(canonical_intent: &[u8], compiled: &CompiledPrompt) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(PROMPT_HASH_DOMAIN);
        update_framed(&mut hasher, canonical_intent);
        update_framed(&mut hasher, compiled.compiler_version.as_bytes());
        update_framed(&mut hasher, compiled.prompt.as_bytes());
        Self(format!("sha256:{:x}", hasher.finalize()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn update_framed(hasher: &mut Sha256, bytes: &[u8]) {
    let length = u64::try_from(bytes.len()).expect("prompt-hash field length must fit in u64");
    hasher.update(length.to_be_bytes());
    hasher.update(bytes);
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConstraintId(String);

impl ConstraintId {
    pub fn new(value: impl Into<String>) -> Result<Self, ConstraintIdError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            return Err(ConstraintIdError);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConstraintIdError;

impl fmt::Display for ConstraintIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(
            "constraint id must be 1..=128 bytes of portable ASCII letters, digits, '_', '-', or '.'",
        )
    }
}

impl Error for ConstraintIdError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DegradationLevel {
    None,
    Cosmetic,
    IdentityAffecting,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DroppedConstraint {
    pub constraint: ConstraintId,
    pub reason: String,
    pub degradation: DegradationLevel,
}

/// A prompt compilation result whose degradation cannot disagree with its
/// dropped constraints.
///
/// The fields are intentionally private. `degradation()` is derived on demand
/// from `dropped_constraints`, so there is no stored value that can lie about
/// the severity of a compilation loss.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledPrompt {
    prompt: String,
    applied_constraints: Vec<ConstraintId>,
    dropped_constraints: Vec<DroppedConstraint>,
    compiler_version: String,
}

impl CompiledPrompt {
    pub fn new(
        prompt: impl Into<String>,
        applied_constraints: Vec<ConstraintId>,
        dropped_constraints: Vec<DroppedConstraint>,
        compiler_version: impl Into<String>,
    ) -> Result<Self, CompiledPromptError> {
        let compiler_version = compiler_version.into();
        if compiler_version.is_empty() {
            return Err(CompiledPromptError::MissingCompilerVersion);
        }
        if dropped_constraints
            .iter()
            .any(|constraint| constraint.reason.is_empty())
        {
            return Err(CompiledPromptError::MissingDropReason);
        }

        Ok(Self {
            prompt: prompt.into(),
            applied_constraints,
            dropped_constraints,
            compiler_version,
        })
    }

    #[must_use]
    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    #[must_use]
    pub fn applied_constraints(&self) -> &[ConstraintId] {
        &self.applied_constraints
    }

    #[must_use]
    pub fn dropped_constraints(&self) -> &[DroppedConstraint] {
        &self.dropped_constraints
    }

    #[must_use]
    pub fn compiler_version(&self) -> &str {
        &self.compiler_version
    }

    #[must_use]
    pub fn degradation(&self) -> DegradationLevel {
        self.dropped_constraints
            .iter()
            .map(|constraint| constraint.degradation)
            .max()
            .unwrap_or(DegradationLevel::None)
    }

    #[must_use]
    pub fn is_degraded(&self) -> bool {
        self.degradation() != DegradationLevel::None
    }

    /// Hash the structured intent, compiler version, and verbatim compiled
    /// provider input required by the canonical artifact provenance contract.
    ///
    /// `canonical_intent` has the same canonical-encoding requirement as
    /// [`PromptHash::derive`].
    #[must_use]
    pub fn prompt_hash(&self, canonical_intent: &[u8]) -> PromptHash {
        PromptHash::derive(canonical_intent, self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompiledPromptError {
    MissingCompilerVersion,
    MissingDropReason,
}

impl fmt::Display for CompiledPromptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MissingCompilerVersion => "compiler version must not be empty",
            Self::MissingDropReason => "every dropped constraint must explain why it was dropped",
        })
    }
}

impl Error for CompiledPromptError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerationPolicy {
    pub block_identity_affecting_degradation: bool,
}

impl GenerationPolicy {
    #[must_use]
    pub fn allows(self, prompt: &CompiledPrompt) -> bool {
        !(self.block_identity_affecting_degradation
            && matches!(prompt.degradation(), DegradationLevel::IdentityAffecting))
    }
}

pub trait PromptCompiler<Intent> {
    type Error;

    fn compile(&self, intent: &Intent) -> Result<CompiledPrompt, Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn constraint(value: &str) -> ConstraintId {
        ConstraintId::new(value).unwrap()
    }

    #[test]
    fn degradation_is_derived_from_dropped_constraints() {
        let compiled = CompiledPrompt::new(
            "portrait of Alice",
            vec![constraint("framing.medium_close_up")],
            vec![
                DroppedConstraint {
                    constraint: constraint("negative_prompt.no_text"),
                    reason: "provider has no negative-prompt channel".to_owned(),
                    degradation: DegradationLevel::Cosmetic,
                },
                DroppedConstraint {
                    constraint: constraint("identity.alice.reference_pack"),
                    reason: "provider does not accept identity references".to_owned(),
                    degradation: DegradationLevel::IdentityAffecting,
                },
            ],
            "image-prompt/1",
        )
        .unwrap();

        assert_eq!(compiled.degradation(), DegradationLevel::IdentityAffecting);
        assert_eq!(compiled.prompt(), "portrait of Alice");
        assert_eq!(compiled.applied_constraints().len(), 1);
        assert_eq!(compiled.dropped_constraints().len(), 2);
        assert_eq!(compiled.compiler_version(), "image-prompt/1");
        assert!(compiled.is_degraded());
    }

    #[test]
    fn policy_can_block_identity_affecting_degradation() {
        let policy = GenerationPolicy {
            block_identity_affecting_degradation: true,
        };
        let cosmetic = CompiledPrompt::new(
            "prompt",
            Vec::new(),
            vec![DroppedConstraint {
                constraint: constraint("camera.slow_dolly"),
                reason: "provider lacks parametric camera control".to_owned(),
                degradation: DegradationLevel::Cosmetic,
            }],
            "video-prompt/1",
        )
        .unwrap();
        let identity_affecting = CompiledPrompt::new(
            "prompt",
            Vec::new(),
            vec![DroppedConstraint {
                constraint: constraint("identity.alice.reference_pack"),
                reason: "provider lacks identity references".to_owned(),
                degradation: DegradationLevel::IdentityAffecting,
            }],
            "video-prompt/1",
        )
        .unwrap();

        assert!(policy.allows(&cosmetic));
        assert!(!policy.allows(&identity_affecting));
    }

    #[test]
    fn every_dropped_constraint_requires_a_reason() {
        let result = CompiledPrompt::new(
            "prompt",
            Vec::new(),
            vec![DroppedConstraint {
                constraint: constraint("camera.slow_dolly"),
                reason: String::new(),
                degradation: DegradationLevel::Cosmetic,
            }],
            "video-prompt/1",
        );

        assert_eq!(result.unwrap_err(), CompiledPromptError::MissingDropReason);
    }

    #[test]
    fn constraint_ids_are_portable_and_bounded() {
        assert!(ConstraintId::new("identity.alice_reference-1").is_ok());
        assert!(ConstraintId::new("contains space").is_err());
        assert!(ConstraintId::new("x".repeat(129)).is_err());
    }

    #[test]
    fn prompt_hash_covers_intent_compiler_version_and_provider_input() {
        let compiled = CompiledPrompt::new(
            "portrait of Alice",
            vec![constraint("identity.alice")],
            Vec::new(),
            "image-prompt/1",
        )
        .unwrap();
        let intent = br#"{"camera":"static","subject":"alice"}"#;

        let first = compiled.prompt_hash(intent);
        let second = compiled.prompt_hash(intent);
        assert_eq!(first, second);
        assert_eq!(
            first.as_str(),
            "sha256:633a444286a909f2bb6f7caf3ad8dbedb4688e4997f0f009fb1d505302d8b4bd"
        );

        assert_ne!(
            first,
            compiled.prompt_hash(br#"{"camera":"dolly","subject":"alice"}"#)
        );

        let newer_compiler = CompiledPrompt::new(
            "portrait of Alice",
            vec![constraint("identity.alice")],
            Vec::new(),
            "image-prompt/2",
        )
        .unwrap();
        assert_ne!(first, newer_compiler.prompt_hash(intent));

        let changed_provider_input = CompiledPrompt::new(
            "cinematic portrait of Alice",
            vec![constraint("identity.alice")],
            Vec::new(),
            "image-prompt/1",
        )
        .unwrap();
        assert_ne!(first, changed_provider_input.prompt_hash(intent));
    }

    #[test]
    fn prompt_hash_length_framing_prevents_boundary_aliases() {
        let left = CompiledPrompt::new("c", Vec::new(), Vec::new(), "b").unwrap();
        let right = CompiledPrompt::new("bc", Vec::new(), Vec::new(), "a").unwrap();

        assert_ne!(left.prompt_hash(b"a"), right.prompt_hash(b""));
    }
}
