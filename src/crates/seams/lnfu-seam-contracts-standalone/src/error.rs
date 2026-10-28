//! Unified error body for the seam layer.
//!
//! Contract reference: CONTRACT.md section 6.1 defines the single error body
//! used across the kernel; section 6.3 defines the fail-loud principle: every
//! failure carries `detail` and `locus`, and no path returns success silently.

use std::fmt;

/// Unified error body.
///
/// Field semantics follow CONTRACT.md section 6.1:
/// - `code`: stable error code, one code carries exactly one meaning.
/// - `domain`: error domain prefix without the trailing dash.
/// - `detail`: human readable detail, must state what failed.
/// - `stderr_tail`: optional tail of an external command stderr.
/// - `locus`: `file:line` of the producing site, or command plus arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractError {
    pub code: &'static str,
    pub domain: &'static str,
    pub detail: String,
    pub stderr_tail: Option<String>,
    pub locus: String,
}

impl ContractError {
    /// Construct an error at the producing site.
    ///
    /// `locus` is passed explicitly so the producing site stays auditable
    /// instead of relying on a captured backtrace.
    pub fn new(
        code: &'static str,
        domain: &'static str,
        detail: impl Into<String>,
        locus: impl Into<String>,
    ) -> Self {
        Self {
            code,
            domain,
            detail: detail.into(),
            stderr_tail: None,
            locus: locus.into(),
        }
    }

    /// Attach the tail of an external command stderr.
    pub fn with_stderr_tail(mut self, tail: impl Into<String>) -> Self {
        self.stderr_tail = Some(tail.into());
        self
    }

    /// Build the locus string from the current source location.
    pub fn here(code: &'static str, domain: &'static str, detail: impl Into<String>) -> Self {
        Self::new(code, domain, detail, format!("{}:{}", file!(), line!()))
    }
}

impl From<std::io::Error> for ContractError {
    fn from(error: std::io::Error) -> Self {
        Self::here(
            crate::resource_codes::E_RES_001,
            crate::resource_codes::DOMAIN_RESOURCE,
            format!("io read failed: {error}"),
        )
    }
}

impl From<serde_json::Error> for ContractError {
    fn from(error: serde_json::Error) -> Self {
        Self::here(
            crate::resource_codes::E_RES_001,
            crate::resource_codes::DOMAIN_RESOURCE,
            format!("json parse failed: {error}"),
        )
    }
}

impl fmt::Display for ContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {} ({})", self.code, self.detail, self.locus)?;
        if let Some(tail) = &self.stderr_tail {
            write!(f, " stderr_tail={tail}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ContractError {}
