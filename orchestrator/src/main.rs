//! # Bipolar Orchestrator - entry point
//!
//! Parses CLI arguments, reads the galaxy and planet-config files, constructs
//! the orchestrator, and hands control to the interactive API loop.
//!
//! ## Usage
//! ```text
//! bipolar-orchestrator --galaxy galaxy.txt --planets planets.toml [--auto]
//! ```
//! - `--galaxy <path>`  : path to the galaxy topology file (required)
//! - `--planets <path>` : path to the planet factory config (required)
//! - `--auto`           : immediately start the game logic without waiting for user input

use bipolar_orchestrator::api::OrchestratorApi;
use bipolar_orchestrator::galaxy;
use std::path::PathBuf;

fn main() {
    env_logger::init();

    let args = parse_args();

    log::info!(
        "Starting Bipolar Orchestrator - galaxy: {:?}, planets: {:?}",
        args.galaxy_path,
        args.planets_path
    );

    // ── Load galaxy topology ──────────────────────────────────────────────────
    let topology = match galaxy::parser::parse(&args.galaxy_path) {
        Ok(t) => {
            log::info!("Galaxy loaded: {} planets", t.planet_count());
            t
        }
        Err(e) => {
            eprintln!("Error loading galaxy file: {e}");
            std::process::exit(1);
        }
    };

    // ── Load planet factory mapping ───────────────────────────────────────────
    let planet_config = match galaxy::planet_config::parse(&args.planets_path) {
        Ok(c) => {
            log::info!("Planet config loaded: {} entries", c.len());
            c
        }
        Err(e) => {
            eprintln!("Error loading planets config: {e}");
            std::process::exit(1);
        }
    };

    // ── Validate that every planet in the topology has a factory entry ────────
    for id in topology.planet_ids() {
        if !planet_config.contains_key(&id) {
            eprintln!("Planet {id} is in galaxy.txt but has no entry in planets.toml");
            std::process::exit(1);
        }
    }

    // TODO(Vivi): construct OrchestratorApi::new(topology, planet_config, ...)
    //             spawn planet threads, spawn explorer threads
    // TODO(Vivi): assign `api` here, then pass it to interactive_loop
    // if args.auto_start { api.start_logic().unwrap(); }

    interactive_loop();
}

/// Holds parsed CLI arguments.
struct Args {
    galaxy_path: PathBuf,
    planets_path: PathBuf,
    /// When true the game logic starts immediately without waiting for user input.
    auto_start: bool,
}

/// Minimal hand-rolled CLI parser.
///
/// # Panics
/// Panics if `--galaxy` or `--planets` is not provided.
fn parse_args() -> Args {
    let mut args = std::env::args().skip(1);
    let mut galaxy_path: Option<PathBuf> = None;
    let mut planets_path: Option<PathBuf> = None;
    let mut auto_start = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--galaxy" => {
                let path = args.next().expect("--galaxy requires a file path argument");
                galaxy_path = Some(PathBuf::from(path));
            }
            "--planets" => {
                let path = args.next().expect("--planets requires a file path argument");
                planets_path = Some(PathBuf::from(path));
            }
            "--auto" => auto_start = true,
            other => log::warn!("Unknown argument ignored: {other}"),
        }
    }

    Args {
        galaxy_path: galaxy_path.expect("--galaxy <path> is required"),
        planets_path: planets_path.expect("--planets <path> is required"),
        auto_start,
    }
}

/// Blocking interactive loop that reads user commands from stdin.
///
/// Accepts an `OrchestratorApi` and dispatches every typed command to it.
/// Blocks until the user types `quit` or stdin closes.
///
/// Available commands:
/// - `start`                        start the game logic loop
/// - `stop`                         stop the game logic loop
/// - `sunray <planet_id>`           send a sunray manually
/// - `asteroid <planet_id>`         send an asteroid manually
/// - `move <explorer_id> <pid>`     move an explorer to a planet
/// - `generate <explorer_id> <res>` generate a basic resource
/// - `combine <explorer_id> <res>`  combine resources into a complex one
/// - `bag <explorer_id>`            print the explorer bag contents
/// - `state <planet_id>`            print planet internal state
/// - `neighbors <planet_id>`        list neighboring planets
/// - `planets`                      list all alive planets
/// - `quit`                         gracefully shut down
fn interactive_loop(api: &mut bipolar_orchestrator::api::OrchestratorApi) {
    use bipolar_orchestrator::api::commands::{self, Command};
    use std::io::{self, BufRead};

    println!("Bipolar Orchestrator ready. Type 'help' for commands.");

    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim().to_string();

        if line.is_empty() {
            continue;
        }

        // parse the raw string into a typed Command
        let cmd = match commands::parse(&line) {
            Ok(c) => c,
            Err(e) => {
                println!("Error: {e}");
                continue;
            }
        };

        match cmd {
            Command::Help => print_help(),

            Command::Quit => {
                println!("Shutting down...");
                api.shutdown();
                break;
            }

            Command::Start => match api.start_logic() {
                Ok(()) => println!("Game logic started."),
                Err(e) => println!("Error: {e}"),
            },

            Command::Stop => match api.stop_logic() {
                Ok(()) => println!("Game logic stopped."),
                Err(e) => println!("Error: {e}"),
            },

            Command::Sunray { planet_id } => match api.send_sunray(planet_id) {
                Ok(()) => println!("Sunray sent to planet {planet_id}."),
                Err(e) => println!("Error: {e}"),
            },

            Command::Asteroid { planet_id } => match api.send_asteroid(planet_id) {
                Ok(true) => println!("Planet {planet_id} deflected the asteroid."),
                Ok(false) => println!("Planet {planet_id} was destroyed."),
                Err(e) => println!("Error: {e}"),
            },

            Command::MoveExplorer { explorer_id, planet_id } => {
                match api.move_explorer(explorer_id, planet_id) {
                    Ok(()) => println!("Explorer {explorer_id} moved to planet {planet_id}."),
                    Err(e) => println!("Error: {e}"),
                }
            }

            Command::GenerateResource { explorer_id, resource } => {
                match api.generate_resource(explorer_id, resource) {
                    Ok(()) => println!("Resource generated by explorer {explorer_id}."),
                    Err(e) => println!("Error: {e}"),
                }
            }

            Command::CombineResource { explorer_id, resource } => {
                match api.combine_resource(explorer_id, resource) {
                    Ok(()) => println!("Resource combined by explorer {explorer_id}."),
                    Err(e) => println!("Error: {e}"),
                }
            }

            Command::Bag { explorer_id } => {
                // bag content display is handled once BagContentRequest is wired
                println!("Bag command for explorer {explorer_id} not yet wired (Vivi).");
            }

            Command::PlanetState { planet_id } => match api.planet_state(planet_id) {
                Ok(state) => println!(
                    "Planet {planet_id}: {} cells, {} charged, rocket={}",
                    state.energy_cells.len(),
                    state.charged_cells_count,
                    state.has_rocket
                ),
                Err(e) => println!("Error: {e}"),
            },

            Command::Neighbors { planet_id } => match api.neighbors(planet_id) {
                Ok(ids) => println!("Neighbors of {planet_id}: {ids:?}"),
                Err(e) => println!("Error: {e}"),
            },

            Command::AlivePlanets => {
                let ids = api.alive_planets();
                println!("Alive planets: {ids:?}");
            }
        }
    }
}

fn print_help() {
    println!(
        "\
Commands:
  start                      Start game logic (probability loop + explorer AIs)
  stop                       Stop game logic
  sunray <planet_id>         Send a sunray manually
  asteroid <planet_id>       Send an asteroid manually
  move <explorer_id> <pid>   Move explorer to planet
  bag <explorer_id>          Show explorer bag
  state <planet_id>          Show planet internal state
  quit                       Exit"
    );
}
