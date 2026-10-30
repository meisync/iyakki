//! One error type. Messages name the directive and the reason, never the value (a value may be a key).

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VpnError {
    /// The file is too big or has too many or too long lines.
    TooLarge(String),
    /// Not UTF-8, NUL or another control character.
    BadText(String),
    /// A directive that is refused on purpose (script, plugin, file path, control socket ...).
    Rejected { line: usize, directive: String, reason: String },
    /// A value that does not parse or is out of range.
    Invalid { line: usize, directive: String, reason: String },
    /// Something required is missing.
    Missing(String),
    /// A route, DNS server or endpoint that the checks refuse.
    Unsafe(String),
    /// The netlink conversation failed (kernel error number, or a reply that does not parse).
    Netlink(String),
}

impl std::fmt::Display for VpnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VpnError::TooLarge(s) | VpnError::BadText(s) | VpnError::Missing(s) | VpnError::Unsafe(s) | VpnError::Netlink(s) => f.write_str(s),
            VpnError::Rejected { line, directive, reason } => write!(f, "line {line}: \"{directive}\" is not accepted: {reason}"),
            VpnError::Invalid { line, directive, reason } => write!(f, "line {line}: \"{directive}\" is not valid: {reason}"),
        }
    }
}

impl std::error::Error for VpnError {}

pub(crate) fn rejected(line: usize, directive: &str, reason: &str) -> VpnError {
    VpnError::Rejected { line, directive: directive.to_string(), reason: reason.to_string() }
}

pub(crate) fn invalid(line: usize, directive: &str, reason: &str) -> VpnError {
    VpnError::Invalid { line, directive: directive.to_string(), reason: reason.to_string() }
}
