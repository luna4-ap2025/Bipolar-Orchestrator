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
    // TODO(both): if args.auto_start, call api.start_logic() here

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
/// Available commands (to be extended):
/// - `start`              → start game logic
/// - `stop`               → stop game logic
/// - `sunray <planet_id>` → manually send a sunray to a planet
/// - `asteroid <planet_id>` → manually send an asteroid to a planet
/// - `move <explorer_id> <planet_id>` → move an explorer to a planet
/// - `bag <explorer_id>`  → print explorer bag contents
/// - `state <planet_id>`  → print planet internal state
/// - `quit`               → gracefully shut down
fn interactive_loop() {
    use std::io::{self, BufRead};

    println!("Bipolar Orchestrator ready. Type 'help' for commands.");

    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim().to_string();

        match line.as_str() {
            "help" => print_help(),
            "quit" | "exit" => {
                println!("Shutting down...");
                // TODO(Vivi): call api.shutdown() here before breaking
                break;
            }
            // TODO(both): parse the line with api::commands::parse() and call the right api method
            other => println!("Unknown command: '{other}'. Type 'help'."),
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
