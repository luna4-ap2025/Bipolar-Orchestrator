//! Reads planets.toml, which says which group's planet goes on each id:
//! ```toml
//! [planets]
//! 1 = "orbitron"
//! 2 = "rustrelli"
//! ```
//! It's not real TOML parsing, just this simple format. The `[planets]` line
//! is required, `#` lines are skipped.

use crate::error::OrchestratorError;
use common_game::utils::ID;
use std::collections::HashMap;
use std::path::Path;

/// planet id -> factory name
pub type PlanetConfigMap = HashMap<ID, String>;

/// # Errors
/// `GalaxyFileError` if the file can't be read or parsed (see [`parse_str`]).
pub fn parse(path: impl AsRef<Path>) -> Result<PlanetConfigMap, OrchestratorError> {
    let content = std::fs::read_to_string(path.as_ref()).map_err(|e| {
        OrchestratorError::GalaxyFileError(format!(
            "cannot read planet config {}: {e}",
            path.as_ref().display()
        ))
    })?;
    parse_str(&content)
}

/// # Errors
/// `GalaxyFileError` if `[planets]` is missing or a line isn't
/// `<id> = "<name>"`.
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

        if line.starts_with('[') {
            in_planets_section = false;
            continue;
        }

        if !in_planets_section {
            continue;
        }

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
        .map_err(|_| err(line_no, &format!("'{id_str}' is not a valid planet id")))?;

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
        let input = "[planets]\n1 = \"orbitron\"\n2 = \"rustrelli\"\n";
        let map = parse_str(input).unwrap();
        assert_eq!(map.get(&1).unwrap(), "orbitron");
        assert_eq!(map.get(&2).unwrap(), "rustrelli");
    }

    #[test]
    fn missing_section_header_is_error() {
        assert!(parse_str("1 = \"orbitron\"").is_err());
    }

    #[test]
    fn unquoted_name_is_error() {
        assert!(parse_str("[planets]\n1 = orbitron").is_err());
    }

    #[test]
    fn invalid_id_is_error() {
        assert!(parse_str("[planets]\nabc = \"orbitron\"").is_err());
    }

    #[test]
    fn comments_and_blank_lines_ignored() {
        let input = "# header\n\n[planets]\n# comment\n3 = \"crabtorio\"\n";
        let map = parse_str(input).unwrap();
        assert_eq!(map.get(&3).unwrap(), "crabtorio");
    }

    #[test]
    fn empty_planets_section_is_ok() {
        let map = parse_str("[planets]\n").unwrap();
        assert!(map.is_empty());
    }
}
