use thiserror::Error;

/// Error returned while executing a model-visible tool invocation.
#[derive(Debug, Error, PartialEq)]
pub enum FunctionCallError {
    #[error("{0}")]
    RespondToModel(String),
    #[error("{0}")]
    MalformedToolCallTruncated(MalformedToolCallDiagnostic),
    #[error("Fatal error: {0}")]
    Fatal(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MalformedToolCallDiagnostic {
    pub tool: String,
    pub byte_len: usize,
    pub category: String,
    pub excerpt: String,
    pub finish_reason: Option<String>,
}

impl std::fmt::Display for MalformedToolCallDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "malformed tool call arguments for `{}`: category={} byte_len={}",
            self.tool, self.category, self.byte_len
        )?;
        if let Some(finish_reason) = &self.finish_reason {
            write!(f, " finish_reason={finish_reason}")?;
        }
        if !self.excerpt.is_empty() {
            write!(f, " excerpt={:?}", self.excerpt)?;
        }
        Ok(())
    }
}
