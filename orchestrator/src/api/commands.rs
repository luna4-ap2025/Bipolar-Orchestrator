//! Turns what the user types in the CLI into a [`Command`].

use common_game::components::resource::{BasicResourceType, ComplexResourceType};
use common_game::utils::ID;

#[derive(Debug, PartialEq)]
pub enum Command {
    Start,
    Stop,
    Sunray {
        planet_id: ID,
    },
    Asteroid {
        planet_id: ID,
    },
    MoveExplorer {
        explorer_id: ID,
        planet_id: ID,
    },
    Bag {
        explorer_id: ID,
    },
    PlanetState {
        planet_id: ID,
    },
    GenerateResource {
        explorer_id: ID,
        resource: BasicResourceType,
    },
    CombineResource {
        explorer_id: ID,
        resource: ComplexResourceType,
    },
    Neighbors {
        planet_id: ID,
    },
    AlivePlanets,
    Help,
    Quit,
}

/// # Errors
/// A message for the user if the command doesn't exist or its arguments are
/// missing or wrong.
pub fn parse(input: &str) -> Result<Command, String> {
    let mut parts = input.split_whitespace();
    let verb = parts.next().unwrap_or("").to_lowercase();

    match verb.as_str() {
        "start" => Ok(Command::Start),
        "stop" => Ok(Command::Stop),
        "help" => Ok(Command::Help),
        "quit" | "exit" => Ok(Command::Quit),
        "planets" => Ok(Command::AlivePlanets),

        "sunray" => {
            let id = parse_id(&mut parts, "planet_id")?;
            Ok(Command::Sunray { planet_id: id })
        }

        "asteroid" => {
            let id = parse_id(&mut parts, "planet_id")?;
            Ok(Command::Asteroid { planet_id: id })
        }

        "state" => {
            let id = parse_id(&mut parts, "planet_id")?;
            Ok(Command::PlanetState { planet_id: id })
        }

        "neighbors" => {
            let id = parse_id(&mut parts, "planet_id")?;
            Ok(Command::Neighbors { planet_id: id })
        }

        "bag" => {
            let id = parse_id(&mut parts, "explorer_id")?;
            Ok(Command::Bag { explorer_id: id })
        }

        "move" => {
            let explorer_id = parse_id(&mut parts, "explorer_id")?;
            let planet_id = parse_id(&mut parts, "planet_id")?;
            Ok(Command::MoveExplorer {
                explorer_id,
                planet_id,
            })
        }

        "generate" => {
            let explorer_id = parse_id(&mut parts, "explorer_id")?;
            let resource_name = parts.next().ok_or("generate <explorer_id> <resource>")?;
            let resource = parse_basic_resource(resource_name)?;
            Ok(Command::GenerateResource {
                explorer_id,
                resource,
            })
        }

        "combine" => {
            let explorer_id = parse_id(&mut parts, "explorer_id")?;
            let resource_name = parts.next().ok_or("combine <explorer_id> <resource>")?;
            let resource = parse_complex_resource(resource_name)?;
            Ok(Command::CombineResource {
                explorer_id,
                resource,
            })
        }

        other => Err(format!("Unknown command '{other}'. Type 'help'.")),
    }
}

fn parse_id<'a>(parts: &mut impl Iterator<Item = &'a str>, name: &str) -> Result<ID, String> {
    parts
        .next()
        .ok_or_else(|| format!("Missing argument: {name}"))?
        .parse::<ID>()
        .map_err(|_| format!("'{name}' must be an integer"))
}

fn parse_basic_resource(s: &str) -> Result<BasicResourceType, String> {
    match s.to_lowercase().as_str() {
        "oxygen" => Ok(BasicResourceType::Oxygen),
        "hydrogen" => Ok(BasicResourceType::Hydrogen),
        "carbon" => Ok(BasicResourceType::Carbon),
        "silicon" => Ok(BasicResourceType::Silicon),
        other => Err(format!(
            "Unknown basic resource '{other}'. Options: oxygen, hydrogen, carbon, silicon"
        )),
    }
}

fn parse_complex_resource(s: &str) -> Result<ComplexResourceType, String> {
    match s.to_lowercase().as_str() {
        "water" => Ok(ComplexResourceType::Water),
        "diamond" => Ok(ComplexResourceType::Diamond),
        "life" => Ok(ComplexResourceType::Life),
        "robot" => Ok(ComplexResourceType::Robot),
        "dolphin" => Ok(ComplexResourceType::Dolphin),
        "aipartner" => Ok(ComplexResourceType::AIPartner),
        other => Err(format!(
            "Unknown complex resource '{other}'. Options: water, diamond, life, robot, dolphin, aipartner"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_start() {
        assert_eq!(parse("start").unwrap(), Command::Start);
    }

    #[test]
    fn parses_sunray() {
        assert_eq!(parse("sunray 3").unwrap(), Command::Sunray { planet_id: 3 });
    }

    #[test]
    fn parses_move() {
        assert_eq!(
            parse("move 1 5").unwrap(),
            Command::MoveExplorer {
                explorer_id: 1,
                planet_id: 5
            }
        );
    }

    #[test]
    fn unknown_command_errors() {
        assert!(parse("foobar").is_err());
    }

    #[test]
    fn missing_arg_errors() {
        assert!(parse("sunray").is_err());
        assert!(parse("move 1").is_err());
    }
}
