//! Reads galaxy.txt. One line per planet: `<planet_id> <neighbor_id> ...`.
//! Connections go both ways, so if 1 lists 2, 2 is also connected to 1.
//! Empty lines and lines starting with `#` are skipped.

use crate::error::OrchestratorError;
use crate::galaxy::topology::Topology;
use common_game::utils::ID;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// # Errors
/// `GalaxyFileError` if the file can't be read, a token isn't a number, or a
/// planet lists itself as a neighbor.
pub fn parse(path: impl AsRef<Path>) -> Result<Topology, OrchestratorError> {
    let content = std::fs::read_to_string(path.as_ref()).map_err(|e| {
        OrchestratorError::GalaxyFileError(format!("Cannot read {}: {e}", path.as_ref().display()))
    })?;

    parse_str(&content)
}

/// Same as [`parse`] but from a string (used by the GUI and the tests).
///
/// # Errors
/// See [`parse`].
pub fn parse_str(content: &str) -> Result<Topology, OrchestratorError> {
    let mut adjacency: HashMap<ID, HashSet<ID>> = HashMap::new();

    for (line_no, raw_line) in content.lines().enumerate() {
        let line = raw_line.trim();

        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let mut tokens = line.split_whitespace();

        let planet_id: ID = tokens
            .next()
            .ok_or_else(|| {
                OrchestratorError::GalaxyFileError(format!(
                    "Line {}: expected planet id",
                    line_no + 1
                ))
            })?
            .parse()
            .map_err(|_| {
                OrchestratorError::GalaxyFileError(format!(
                    "Line {}: planet id is not a valid u32",
                    line_no + 1
                ))
            })?;

        // so planets with no neighbors still exist
        adjacency.entry(planet_id).or_default();

        // remaining tokens are neighbor ids
        for token in tokens {
            let neighbor_id: ID = token.parse().map_err(|_| {
                OrchestratorError::GalaxyFileError(format!(
                    "Line {}: '{}' is not a valid neighbor id",
                    line_no + 1,
                    token
                ))
            })?;

            if neighbor_id == planet_id {
                return Err(OrchestratorError::GalaxyFileError(format!(
                    "Line {}: planet {planet_id} lists itself as its own neighbor",
                    line_no + 1
                )));
            }

            // bidirectional
            adjacency.entry(planet_id).or_default().insert(neighbor_id);
            adjacency.entry(neighbor_id).or_default().insert(planet_id);
        }
    }

    Ok(Topology::from_adjacency(adjacency))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_ring() {
        let input = "
# Ring of 3
1 2 3
2 1 3
3 2 1
";
        let topology = parse_str(input).unwrap();
        assert!(topology.are_neighbors(1, 2));
        assert!(topology.are_neighbors(2, 3));
        assert!(topology.are_neighbors(3, 1));
        assert!(!topology.are_neighbors(1, 1));
    }

    #[test]
    fn bidirectional_implied() {
        let input = "1 2";
        let topology = parse_str(input).unwrap();
        assert!(topology.are_neighbors(1, 2));
        assert!(topology.are_neighbors(2, 1));
    }

    #[test]
    fn self_neighbor_is_rejected() {
        let input = "1 1";
        assert!(parse_str(input).is_err());
    }

    #[test]
    fn blank_and_comment_lines_ignored() {
        let input = "
# comment

1 2
";
        assert!(parse_str(input).is_ok());
    }

    #[test]
    fn invalid_id_returns_error() {
        let input = "abc 2";
        assert!(parse_str(input).is_err());
    }
}
