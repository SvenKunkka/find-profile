use crate::db::RootKind;

use super::error::{Result, RootError};

/// Parse a kind string supplied by the user into a strongly-typed `RootKind`.
///
/// Returns a structured `RootError::InvalidKind` for any string that isn't
/// exactly `"local"` or `"nas"`.
pub fn parse_kind(input: &str) -> Result<RootKind> {
    RootKind::parse(input).ok_or_else(|| RootError::InvalidKind(input.to_string()))
}

/// Resolve the final `RootKind` for a root path.
///
/// Rules:
/// 1. If the user explicitly provided a kind string, use it (after validation).
/// 2. Otherwise, auto-detect from the path:
///    - paths starting with `/Volumes/` are treated as `nas`
///    - all other paths are treated as `local`
pub fn resolve_kind(path: &str, user_provided: Option<&str>) -> Result<RootKind> {
    if let Some(s) = user_provided {
        return parse_kind(s);
    }
    Ok(RootKind::detect_from_path(path))
}
