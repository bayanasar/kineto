use std::{error::Error, fmt};

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
}
