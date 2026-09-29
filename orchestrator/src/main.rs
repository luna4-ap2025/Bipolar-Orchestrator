//! Command line version of the orchestrator (the GUI is in the `gui` crate).
//!
//! ```text
//! bipolar-orchestrator --galaxy galaxy.txt --planets planets.toml [--auto]
//! ```
//! `--auto` starts the logic right away instead of waiting for `start`.

use bipolar_orchestrator::api::OrchestratorApi;
use bipolar_orchestrator::explorer::{ExplorerHandle, ExplorerRegistry};
use bipolar_orchestrator::galaxy;
use bipolar_orchestrator::planet::{self, PlanetRegistry};
use bipolar_orchestrator::probability::ProbabilityRegistry;

use common_game::components::forge::Forge;
use common_game::protocols::orchestrator_explorer::OrchestratorToExplorer;
use common_game::protocols::orchestrator_planet::PlanetToOrchestrator;
use common_game::protocols::planet_explorer::PlanetToExplorer;

use crossbeam_channel::unbounded;
use std::collections::HashMap;
use std::path::PathBuf;

// Mostly the same as builder::build_api, but with nicer errors and logs for
// the CLI. Could be merged some day.
#[allow(clippy::too_many_lines)]
fn main() {
    env_logger::init();

    let args = parse_args();

    log::info!(
        "Starting Bipolar Orchestrator - galaxy: {}, planets: {}",
        args.galaxy_path.display(),
        args.planets_path.display()
    );

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

    for id in topology.planet_ids() {
        if !planet_config.contains_key(&id) {
            eprintln!("Planet {id} is in galaxy.txt but has no entry in planets.toml");
            std::process::exit(1);
        }
    }

    // planets
    let (planet_tx, planet_rx) = unbounded::<PlanetToOrchestrator>();

    let factories: HashMap<String, Box<dyn planet::factories::PlanetFactory>> =
        planet::factories::all_factories()
            .into_iter()
            .map(|factory| (factory.name().to_lowercase(), factory))
            .collect();

    let mut planet_registry = PlanetRegistry::new();

    let planet_ids: Vec<_> = topology.planet_ids().collect();

    for &planet_id in &planet_ids {
        let factory_name = planet_config
            .get(&planet_id)
            .ok_or_else(|| format!("Planet {planet_id} has no factory config"))
            .unwrap();

        let factory = factories
            .get(&factory_name.to_lowercase())
            .ok_or_else(|| format!("Unknown planet factory '{factory_name}'"))
            .unwrap();

        let handle = planet::spawn_planet(
            planet_id,
            factory.name(),
            factory.as_ref(),
            planet_tx.clone(),
        )
        .unwrap();

        planet_registry.insert(handle);
    }

    log::info!("Spawned {} planets", planet_registry.count());

    // explorers (same ids and starting planets as builder.rs)
    let (explorer_tx, explorer_rx) =
        unbounded::<bipolar_orchestrator::explorer::handle::ExplorerToOrchestratorMsg>();

    let mut explorer_registry = ExplorerRegistry::new();

    // Viviana
    {
        let (tx_to_viv, rx_from_orch) = unbounded::<OrchestratorToExplorer>();
        let (planet_reply_tx, rx_from_planet) = unbounded::<PlanetToExplorer>();
        let tx_planet = planet_registry
            .get(4)
            .expect("planet 4 not spawned")
            .explorer_sender();

        let mut viv = explorer_astronaut::create_explorer(
            1,
            rx_from_orch,
            explorer_tx.clone(),
            rx_from_planet,
            tx_planet,
            4,
        )
        .expect("failed to create Viviana");

        let thread = std::thread::Builder::new()
            .name("explorer-viviana".into())
            .spawn(move || viv.run())
            .expect("failed to spawn Viviana thread");

        explorer_registry.insert(ExplorerHandle::new(
            1,
            tx_to_viv,
            planet_reply_tx,
            4,
            thread,
        ));
        log::info!("Spawned explorer 1 (Viviana) on planet 4");
    }

    // Jeb
    {
        let (tx_to_jeb, rx_from_orch) = unbounded::<OrchestratorToExplorer>();
        let (planet_reply_tx, rx_from_planet) = unbounded::<PlanetToExplorer>();
        let tx_planet = planet_registry
            .get(1)
            .expect("planet 1 not spawned")
            .explorer_sender();

        let jeb = explorer_jebediah::create_explorer(
            2,
            rx_from_orch,
            explorer_tx.clone(),
            rx_from_planet,
            tx_planet,
            1,
        )
        .expect("failed to create Jeb");

        let thread = std::thread::Builder::new()
            .name("explorer-jeb".into())
            .spawn(move || {
                if let Err(e) = jeb.run() {
                    log::error!("Jeb explorer thread exited with error: {e}");
                }
            })
            .expect("failed to spawn Jeb thread");

        explorer_registry.insert(ExplorerHandle::new(
            2,
            tx_to_jeb,
            planet_reply_tx,
            1,
            thread,
        ));
        log::info!("Spawned explorer 2 (Jeb) on planet 1");
    }

    let mut rng = rand::rng();

    let prob_registry = ProbabilityRegistry::new(planet_ids.iter().copied(), &mut rng);

    let forge = Forge::new().expect("failed to create Forge");

    let mut api = OrchestratorApi::new(
        topology,
        planet_registry,
        explorer_registry,
        prob_registry,
        planet_rx,
        explorer_rx,
        forge,
    );

    if args.auto_start {
        api.start_logic().unwrap();
    }

    interactive_loop(&mut api);
}

struct Args {
    galaxy_path: PathBuf,
    planets_path: PathBuf,
    auto_start: bool,
}

// Panics if --galaxy or --planets is missing.
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
                let path = args
                    .next()
                    .expect("--planets requires a file path argument");
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

// Reads commands from stdin until `quit` (list in api/commands.rs, or type
// `help`).
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

            Command::MoveExplorer {
                explorer_id,
                planet_id,
            } => match api.move_explorer(explorer_id, planet_id) {
                Ok(()) => println!("Explorer {explorer_id} moved to planet {planet_id}."),
                Err(e) => println!("Error: {e}"),
            },

            Command::GenerateResource {
                explorer_id,
                resource,
            } => match api.generate_resource(explorer_id, resource) {
                Ok(()) => println!("Resource generated by explorer {explorer_id}."),
                Err(e) => println!("Error: {e}"),
            },

            Command::CombineResource {
                explorer_id,
                resource,
            } => match api.combine_resource(explorer_id, resource) {
                Ok(()) => println!("Resource combined by explorer {explorer_id}."),
                Err(e) => println!("Error: {e}"),
            },

            Command::Bag { explorer_id } => match api.bag_content(explorer_id) {
                Ok(content) => println!("Explorer {explorer_id} bag: {content:?}"),
                Err(e) => println!("Error: {e}"),
            },

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
