//! # Planet factory configuration parser
//!
//! Reads `planets.toml` and returns a map of `planet_id → factory_name`.
//! This tells the orchestrator which group's planet implementation to use
//! for each ID listed in the galaxy topology file.
//!
//! ## File format
//! A minimal TOML-like format with a single `[planets]` section:
//!
//! ```toml
//! [planets]
//! 1 = "orbitron"
//! 2 = "rustrelli"
//! ```
//!
//! Lines beginning with `#` are comments and are ignored.
//! The `[planets]` header is required.
//!
//! ## Owner: Vale

use crate::error::OrchestratorError;
use common_game::utils::ID;
use std::collections::HashMap;
use std::path::Path;

/// Maps planet IDs to factory names as read from `planets.toml`.
pub type PlanetConfigMap = HashMap<ID, String>;

/// Parses the planet factory configuration file at `path`.
///
/// # Errors
/// Returns [`OrchestratorError::GalaxyFileError`] if the file cannot be read,
/// if the `[planets]` section is missing, or if any line is malformed.
pub fn parse(path: impl AsRef<Path>) -> Result<PlanetConfigMap, OrchestratorError> {
    let content = std::fs::read_to_string(path.as_ref()).map_err(|e| {
        OrchestratorError::GalaxyFileError(format!(
            "Cannot read planet config {:?}: {e}",
            path.as_ref()
        ))
    })?;
    parse_str(&content)
}

/// Parses planet factory configuration from a string (useful for testing).
///
/// # Errors
/// See [`parse`].
pub fn parse_str(content: &str) -> Result<PlanetConfigMap, OrchestratorError> {
    let mut map = PlanetConfigMap::new();
    let mut in_planets_section = false;

    for (line_no, raw_line) in content.lines().enumerate() {
        let line = raw_line.trim();

        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if line == "[planets]" {
            in_planets_section = true;
            continue;
        }

        // any other section header ends the planets block
        if line.starts_with('[') {
            in_planets_section = false;
            continue;
        }

        if !in_planets_section {
            continue;
        }

        // parse:  <id> = "<factory_name>"
        let (id, name) = parse_assignment(line, line_no + 1)?;
        map.insert(id, name);
    }

    if map.is_empty() && !content.contains("[planets]") {
        return Err(OrchestratorError::GalaxyFileError(
            "planets.toml is missing the [planets] section".to_string(),
        ));
    }

    Ok(map)
}

fn parse_assignment(line: &str, line_no: usize) -> Result<(ID, String), OrchestratorError> {
    let mut parts = line.splitn(2, '=');

    let id_str = parts
        .next()
        .ok_or_else(|| err(line_no, "expected '<id> = \"<name>\"'"))?
        .trim();

    let name_str = parts
        .next()
        .ok_or_else(|| err(line_no, "missing '=' in assignment"))?
        .trim();

    let id: ID = id_str
        .parse()
        .map_err(|_| err(line_no, &format!("'{id_str}' is not a valid planet id (u32)")))?;

    // strip surrounding quotes
    let name = name_str
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .ok_or_else(|| err(line_no, "factory name must be quoted, e.g. \"orbitron\""))?
        .to_string();

    if name.is_empty() {
        return Err(err(line_no, "factory name must not be empty"));
    }

    Ok((id, name))
}

fn err(line_no: usize, msg: &str) -> OrchestratorError {
    OrchestratorError::GalaxyFileError(format!("planets.toml line {line_no}: {msg}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_config() {
        let input = r#"
# example
[planets]
1 = "orbitron"
2 = "rustrelli"
"#;
        let map = parse_str(input).unwrap();
        assert_eq!(map.get(&1).unwrap(), "orbitron");
        assert_eq!(map.get(&2).unwrap(), "rustrelli");
    }

    #[test]
    fn missing_section_header_is_error() {
        let input = r#"1 = "orbitron""#;
        assert!(parse_str(input).is_err());
    }

    #[test]
    fn unquoted_name_is_error() {
        let input = "[planets]\n1 = orbitron";
        assert!(parse_str(input).is_err());
    }

    #[test]
    fn invalid_id_is_error() {
        let input = "[planets]\nabc = \"orbitron\"";
        assert!(parse_str(input).is_err());
    }

    #[test]
    fn comments_and_blank_lines_ignored() {
        let input = "# header\n\n[planets]\n# comment\n3 = \"crabtorio\"\n";
        let map = parse_str(input).unwrap();
        assert_eq!(map.get(&3).unwrap(), "crabtorio");
    }

    #[test]
    fn empty_planets_section_is_ok() {
        let input = "[planets]\n";
        let map = parse_str(input).unwrap();
        assert!(map.is_empty());
    }
}
