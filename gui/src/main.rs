// Pedantic clippy lints that don't fit Bevy code:
// - systems have to take Res/Query/Commands by value and often need many of them
// - this file is too long and should be split into modules, but moving systems
//   around this close to the deadline risks messing up the system ordering
// - the numeric casts are all pixel coords and frame indices (small values)
#![allow(
    clippy::needless_pass_by_value,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::type_complexity,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::window::{CursorOptions, PrimaryWindow, WindowResolution};
use bipolar_shared::{GalaxyEvent, GalaxySnapshot, Personality, ResourceKind};
use crossbeam_channel::{Receiver as CmdReceiver, Sender as CmdSender};
use rand::RngExt;
use std::collections::{HashMap, HashSet, VecDeque};
use std::f32::consts::{FRAC_PI_2, PI};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// --- Commands from the UI to the orchestrator ---
//
// OrchestratorApi methods block while waiting for acks (with timeouts of a
// few seconds), so calling them from a Bevy system would freeze the window.
// Input systems send a UiCommand instead and the bridge thread runs it.
enum UiCommand {
    Sunray(u32),
    Asteroid(u32),
    MoveExplorer {
        explorer_id: u32,
        dst: u32,
    },
    ToggleLogic,
    // bag/planet info needs a round trip, so it's only fetched on right-click
    InspectExplorer(u32),
    InspectPlanet(u32),
    // debug keys E/S, so I can test the Solace/Eclipse flip without waiting
    DebugNudgeHostility(f64),
}

#[derive(Resource, Clone)]
struct UiCommands(CmdSender<UiCommand>);

#[derive(Resource, Default)]
struct Selection {
    explorer: Option<u32>,
    // Sunray/Asteroid buttons fire at this planet
    planet: Option<u32>,
}

// Only used for the Start/Pause/Resume button. NotStarted and Paused send the
// same command, they just show a different button.
#[derive(Resource, Clone, Copy, PartialEq)]
enum LogicRunState {
    NotStarted,
    Running,
    Paused,
}

impl LogicRunState {
    fn toggled(self) -> Self {
        match self {
            LogicRunState::NotStarted | LogicRunState::Paused => LogicRunState::Running,
            LogicRunState::Running => LogicRunState::Paused,
        }
    }
}

// The bridge thread waits for this before building the galaxy. Otherwise it
// starts while Bevy is still opening the window (takes a few seconds) and
// planets can get hit before anything is on screen.
#[derive(Resource)]
struct ReadySignal(Option<CmdSender<()>>);

// --- Cockpit UI ---
//
// All the cockpit art is in cockpit.png, exported from Aseprite with "Split
// Layers" into a sheet 4 slots wide, 1600x900 per slot. The rectangles below
// are measured from the layers in that file, so if I change the art they
// have to be measured again.

const SOLACE_GOLD: Color = Color::srgb(1.0, 0.85, 0.35);
const ECLIPSE_PURPLE: Color = Color::srgb(0.65, 0.45, 0.95);

const FONT_REGULAR: &str = "fonts/Pixeloid_Font_1_0/PixeloidSans.ttf";

// Everything is laid out for 1600x900 and scaled up by 1.2 to fill my
// laptop's 1920x1080 screen (same 16:9 ratio). On a different resolution this
// will need changing.
// The scale is only for on-screen positions/sizes. Crops into cockpit.png stay
// in the file's own 1600x900 pixel coordinates.
const DISPLAY_SCALE: f32 = 1.2;
const WINDOW_WIDTH: f32 = 1600.0 * DISPLAY_SCALE;
const WINDOW_HEIGHT: f32 = 900.0 * DISPLAY_SCALE;

// Bounding box of the see-through window in cockpit.png. The real shape has
// angled top corners, but the box is good enough to ignore clicks on the frame.
const VIEWPORT_HOLE_MIN: Vec2 = Vec2::new(110.0 * DISPLAY_SCALE, 97.0 * DISPLAY_SCALE);
const VIEWPORT_HOLE_MAX: Vec2 = Vec2::new(1494.0 * DISPLAY_SCALE, 646.0 * DISPLAY_SCALE);

fn cockpit_slot_rect(col: u32, row: u32, local: Rect) -> Rect {
    let origin = Vec2::new(col as f32 * 1600.0, row as f32 * 900.0);
    Rect::new(
        origin.x + local.min.x,
        origin.y + local.min.y,
        origin.x + local.max.x,
        origin.y + local.max.y,
    )
}

fn cockpit_full_slot(col: u32, row: u32) -> Rect {
    cockpit_slot_rect(col, row, Rect::new(0.0, 0.0, 1600.0, 900.0))
}

// Each slot is one full-size layer (transparent except its own art), so
// stacking them at (0,0) rebuilds the whole cockpit.
fn cockpit_shell_layer(asset_server: &AssetServer, col: u32, row: u32) -> impl Bundle {
    (
        ImageNode {
            image: asset_server.load("cockpit.png"),
            rect: Some(cockpit_full_slot(col, row)),
            ..default()
        },
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            top: Val::Px(0.0),
            width: Val::Px(WINDOW_WIDTH),
            height: Val::Px(WINDOW_HEIGHT),
            ..default()
        },
    )
}

// Buttons are cropped to just their own art. Since every layer is canvas
// sized, the crop rect is also where the button goes on screen.
struct CockpitButtonSpec {
    col: u32,
    row: u32,
    local: Rect,
}

const BTN_START: CockpitButtonSpec = CockpitButtonSpec {
    col: 1,
    row: 1,
    local: Rect {
        min: Vec2::new(491.0, 650.0),
        max: Vec2::new(590.0, 733.0),
    },
};
const BTN_PAUSE: CockpitButtonSpec = CockpitButtonSpec {
    col: 2,
    row: 1,
    local: Rect {
        min: Vec2::new(586.0, 650.0),
        max: Vec2::new(697.0, 733.0),
    },
};
const BTN_RESUME: CockpitButtonSpec = CockpitButtonSpec {
    col: 3,
    row: 1,
    local: Rect {
        min: Vec2::new(688.0, 650.0),
        max: Vec2::new(805.0, 733.0),
    },
};
const BTN_SUNRAY: CockpitButtonSpec = CockpitButtonSpec {
    col: 0,
    row: 2,
    local: Rect {
        min: Vec2::new(792.0, 650.0),
        max: Vec2::new(904.0, 733.0),
    },
};
const BTN_ASTEROID: CockpitButtonSpec = CockpitButtonSpec {
    col: 1,
    row: 2,
    local: Rect {
        min: Vec2::new(896.0, 650.0),
        max: Vec2::new(1006.0, 733.0),
    },
};
const BTN_MOVE: CockpitButtonSpec = CockpitButtonSpec {
    col: 2,
    row: 2,
    local: Rect {
        min: Vec2::new(1001.0, 650.0),
        max: Vec2::new(1102.0, 733.0),
    },
};
const HOLOGRAM_LEFT: CockpitButtonSpec = CockpitButtonSpec {
    col: 3,
    row: 2,
    local: Rect {
        min: Vec2::new(98.0, 342.0),
        max: Vec2::new(353.0, 739.0),
    },
};
const HOLOGRAM_RIGHT: CockpitButtonSpec = CockpitButtonSpec {
    col: 0,
    row: 3,
    local: Rect {
        min: Vec2::new(1248.0, 368.0),
        max: Vec2::new(1495.0, 743.0),
    },
};

fn cockpit_button_bundle(asset_server: &AssetServer, spec: &CockpitButtonSpec) -> impl Bundle {
    let w = (spec.local.max.x - spec.local.min.x) * DISPLAY_SCALE;
    let h = (spec.local.max.y - spec.local.min.y) * DISPLAY_SCALE;
    (
        ImageNode {
            image: asset_server.load("cockpit.png"),
            rect: Some(cockpit_slot_rect(spec.col, spec.row, spec.local)),
            ..default()
        },
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(spec.local.min.x * DISPLAY_SCALE),
            top: Val::Px(spec.local.min.y * DISPLAY_SCALE),
            width: Val::Px(w),
            height: Val::Px(h),
            ..default()
        },
    )
}

// Solace's drawing fills more of its frame than Eclipse's, so it gets a
// smaller size to look the same.
const PORTRAIT_BUBBLE_SIZE: f32 = 128.0 * DISPLAY_SCALE;
const SOLACE_INNER_SIZE: f32 = 84.0 * DISPLAY_SCALE;
const ECLIPSE_INNER_SIZE: f32 = 96.0 * DISPLAY_SCALE;

// Full-body figure of whoever is in control, standing next to the galaxy.
// Around a third of the viewport height, any bigger felt like too much.
const STANCE_SIZE: f32 = 240.0 * DISPLAY_SCALE;

#[derive(Component)]
struct TopBarPersonalityText;

#[derive(Component)]
struct TopBarCycleText;

// Arrow showing whether hostility is going up or down. Each planet has its own
// probability curve so there's no single "odds" number to show, but hostility
// pushes all of them the same way.
#[derive(Component)]
struct TopBarTrendText;

#[derive(Component)]
struct GameOverOverlay;

#[derive(Component, Debug)]
enum TransportButton {
    Start,
    Pause,
    Resume,
}

#[derive(Component)]
struct SunrayButtonTag;

#[derive(Component)]
struct AsteroidButtonTag;

// TODO: this should become the Cosmic Chronicle event log. For now it just
// shows some overview stats.
#[derive(Component)]
struct ChronicleScreenText;

// --- Holograms (right-click inspection) ---
//
// Inspecting = which hologram is open. Inspection = the data the bridge
// thread fetched for it (filled in a bit later, since it's a round trip).

#[derive(Clone, Copy, PartialEq, Eq)]
enum InspectTarget {
    Explorer(u32),
    Planet(u32),
}

#[derive(Resource, Default)]
struct Inspecting(Option<InspectTarget>);

#[derive(Clone, Default)]
enum InspectionData {
    #[default]
    None,
    ExplorerBag {
        id: u32,
        bag_lines: Vec<String>,
    },
    PlanetInfo {
        id: u32,
        energy_cells: usize,
        charged_cells: usize,
        has_rocket: bool,
    },
}

#[derive(Resource, Clone)]
struct Inspection(Arc<Mutex<InspectionData>>);

// left panel = explorer bag, right panel = planet
#[derive(Component)]
struct ExplorerHologram;

#[derive(Component)]
struct PlanetHologram;

#[derive(Component)]
struct ExplorerHologramText;

#[derive(Component)]
struct PlanetHologramText;

fn format_mmss(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    format!("{:02}:{:02}", total / 60, total % 60)
}

// --- Map hologram ---
//
// Compass button toggles a small map of the galaxy: alive planets, the
// connections between them, and the current phase.

#[derive(Resource, Default)]
struct MapHologramOpen(bool);

#[derive(Component)]
struct CompassButtonTag;

#[derive(Component)]
struct MapHologramPanel;

#[derive(Component)]
struct MapNodeTag(u32);

// index into CONNECTIONS
#[derive(Component)]
struct MapEdgeTag(usize);

#[derive(Component)]
struct MapClockText;

const MAP_PANEL_W: f32 = 480.0 * DISPLAY_SCALE;
const MAP_PANEL_H: f32 = 384.0 * DISPLAY_SCALE;
const MAP_RING_RADIUS_X: f32 = 150.0 * DISPLAY_SCALE;
const MAP_RING_RADIUS_Y: f32 = 100.0 * DISPLAY_SCALE;
// pushed down a bit to leave room for the clock text at the top
const MAP_RING_CENTER: Vec2 =
    Vec2::new(MAP_PANEL_W / 2.0, MAP_PANEL_H / 2.0 + 30.0 * DISPLAY_SCALE);
const MAP_NODE_SIZE: f32 = 28.0 * DISPLAY_SCALE;
const MAP_EDGE_THICKNESS: f32 = 6.0 * DISPLAY_SCALE;

// same ring as planet_position, but in UI coords (y goes down)
fn map_node_local_pos(index: usize) -> Vec2 {
    let angle = FRAC_PI_2 - index as f32 * 2.0 * PI / 7.0;
    MAP_RING_CENTER
        + Vec2::new(
            MAP_RING_RADIUS_X * angle.cos(),
            -MAP_RING_RADIUS_Y * angle.sin(),
        )
}

// --- Galaxy config ---

const GALAXY_SRC: &str = include_str!("../../galaxy.txt");
const PLANETS_SRC: &str = include_str!("../../planets.toml");

// Read planets.toml here too so names don't need a round trip.
#[derive(Resource)]
struct PlanetNames(HashMap<u32, &'static str>);

impl PlanetNames {
    fn load() -> Self {
        let factories = bipolar_orchestrator::galaxy::planet_config::parse_str(PLANETS_SRC)
            .expect("planets.toml should already be valid — build_api parses the same file");
        Self(
            factories
                .into_iter()
                .map(|(id, factory)| (id, display_name(&factory)))
                .collect(),
        )
    }

    fn get(&self, planet_id: u32) -> &'static str {
        self.0.get(&planet_id).copied().unwrap_or("Unknown")
    }
}

// ids are fixed in builder.rs / main.rs
fn explorer_display_name(id: u32) -> &'static str {
    match id {
        1 => "Viviana",
        2 => "Jebediah",
        _ => "Unknown Explorer",
    }
}

// names as written in the professor's planet repo list
fn display_name(factory: &str) -> &'static str {
    match factory {
        "orbitron" => "Orbitron",
        "skycartel" => "SkyCartel",
        "rustrelli" => "Rustrelli",
        "thecompilerstrikesback" => "The Compiler Strikes Back",
        "crabtorio" => "Crabtorio",
        "houstonwehaveaborrow" => "Houston We Have A Borrow",
        "enterprise" => "Enterprise",
        _ => "Unknown",
    }
}

// --- Resources ---

// None until the first snapshot arrives. An empty default snapshot would look
// like every planet is dead and they'd all play their death animation.
#[derive(Resource, Clone)]
struct LiveSnapshot(Arc<Mutex<Option<GalaxySnapshot>>>);

#[derive(Resource, Default)]
struct GalaxyState {
    alive: HashSet<u32>,
    explorer_planet: HashMap<u32, u32>,
    explorer_bag: HashMap<u32, Vec<(ResourceKind, usize)>>,
    // current neighbors, shrinks when planets are destroyed
    neighbors: HashMap<u32, Vec<u32>>,
    personality: Personality,
    // 0.0 - 1.0, drives the Solace/Eclipse crossfade
    hostility: f64,
    phase_elapsed: f64,
    // same problem as LiveSnapshot: before the first poll `alive` is empty,
    // so sync_planets waits for this
    ready: bool,
}

#[derive(Clone, Copy, PartialEq, Default)]
enum HostilityDirection {
    Rising,
    Falling,
    #[default]
    Steady,
}

// Sampled every couple of seconds, otherwise small wobbles make the arrow
// flicker.
#[derive(Resource, Default)]
struct HostilityTrend {
    last_value: f64,
    last_sample_secs: f32,
    direction: HostilityDirection,
}

const HOSTILITY_TREND_SAMPLE_SECS: f32 = 2.0;
// the normal ramp moves ~0.008 in 2s, so anything under this is noise
const HOSTILITY_TREND_EPSILON: f64 = 0.0015;

// Portraits that should flash the glitch effect (after a manual override)
#[derive(Resource, Default)]
struct GlitchQueue(Vec<Personality>);

// remaining == 0.0 means hidden
#[derive(Component, Default)]
struct GlitchOverlay {
    remaining: f32,
}

const GLITCH_DURATION_SECS: f32 = 1.2;

// Suspicion: 0 = calm, 1 = confused, 2 = suspicious.
// Solace likes sunrays and Eclipse likes asteroids. If the player forces the
// one she likes, she only gets confused. If they force the other one, she gets
// more suspicious each time, and the third time she refuses and goes back to 0.
#[derive(Resource, Default)]
struct SuspicionState {
    solace_tier: u8,
    eclipse_tier: u8,
}

impl SuspicionState {
    fn tier(&self, p: &Personality) -> u8 {
        match p {
            Personality::Solace => self.solace_tier,
            Personality::Eclipse => self.eclipse_tier,
        }
    }

    fn tier_mut(&mut self, p: &Personality) -> &mut u8 {
        match p {
            Personality::Solace => &mut self.solace_tier,
            Personality::Eclipse => &mut self.eclipse_tier,
        }
    }
}

// the refusal isn't a tier of its own, it resets to 0 right away
const SUSPICION_MAX_RESTING_TIER: u8 = 2;

enum SuspicionOutcome {
    // reaction = portrait animation to play, only when a new tier is reached
    Allowed { reaction: Option<&'static str> },
    Refused,
}

fn apply_manual_override(
    state: &mut SuspicionState,
    personality: &Personality,
    sunray: bool,
) -> SuspicionOutcome {
    let preferred = match personality {
        Personality::Solace => sunray,
        Personality::Eclipse => !sunray,
    };
    let tier = state.tier_mut(personality);
    if preferred {
        let was_calm = *tier == 0;
        *tier = (*tier).max(1);
        SuspicionOutcome::Allowed {
            reaction: if was_calm { Some("confused") } else { None },
        }
    } else if *tier >= SUSPICION_MAX_RESTING_TIER {
        *tier = 0;
        SuspicionOutcome::Refused
    } else {
        *tier += 1;
        let reaction = if *tier == 1 { "confused" } else { "suspicious" };
        SuspicionOutcome::Allowed {
            reaction: Some(reaction),
        }
    }
}

#[derive(Component)]
struct SuspicionMeterFill(Personality);

const SUSPICION_METER_W: f32 = 30.0 * DISPLAY_SCALE;
const SUSPICION_METER_H: f32 = 88.0 * DISPLAY_SCALE;
const SUSPICION_FILL_INSET: f32 = 3.0 * DISPLAY_SCALE;

// --- Components ---

#[derive(Component)]
struct AnimationConfig {
    first: usize,
    last: usize,
    timer: Timer,
    looping: bool,
    // range to play after the current one-shot, before hiding
    next_range: Option<(usize, usize)>,
    pending_hide: bool,
}

impl AnimationConfig {
    fn new(first: usize, last: usize, fps: f32, looping: bool) -> Self {
        Self {
            first,
            last,
            timer: Timer::from_seconds(1.0 / fps, TimerMode::Repeating),
            looping,
            next_range: None,
            pending_hide: false,
        }
    }
}

#[derive(Component)]
struct PlanetTag(u32);

#[derive(Component, PartialEq)]
enum PlanetState {
    Alive,
    Dying,
    Dead,
}

#[derive(Component)]
struct ExplorerTag(u32);

#[derive(Resource)]
struct ResourceIcons(HashMap<ResourceKind, Handle<Image>>);

impl ResourceIcons {
    fn load(asset_server: &AssetServer) -> Self {
        use ResourceKind::{
            Carbon, Diamond, Dolphin, Hydrogen, Life, Oxygen, Robot, Silicon, Water,
        };
        let pairs = [
            (Oxygen, "resource_oxygen.png"),
            (Hydrogen, "resource_hydrogen.png"),
            (Carbon, "resource_carbon.png"),
            (Silicon, "resource_silicon.png"),
            (Diamond, "resource_diamond.png"),
            (Water, "resource_water.png"),
            (Life, "resource_life.png"),
            (Robot, "resource_robot.png"),
            (Dolphin, "resource_dolphin.png"),
        ];
        Self(
            pairs
                .into_iter()
                .map(|(k, f)| (k, asset_server.load(f)))
                .collect(),
        )
    }
}

// resource icons shown above each explorer, extra slots are hidden
const RESOURCE_BADGE_SLOTS: usize = 3;

#[derive(Component)]
struct ResourceBadge {
    explorer_id: u32,
    slot: usize,
}

#[derive(PartialEq)]
enum ExplorerPhase {
    Settled,
    Departing,
    Moving,
    Arrived,
    // the "oh, it's you" animation when both explorers end up on the same
    // planet (just visual, explorers can't talk to each other)
    ReactingOther,
}

const CO_REACT_HOLD_SECS: f32 = 1.0;

// Explorers don't use AnimationConfig, everything for them is in here.
#[derive(Component)]
struct ExplorerAnim {
    // frame ranges come from jeb.json / viviana.json
    moving_range: (usize, usize),
    arrived_range: (usize, usize),
    departing_range: (usize, usize),
    // resting on a planet: Viviana collects, Jeb idles
    settled_range: (usize, usize),
    react_other_range: (usize, usize),
    fps: f32,
    anim_timer: Timer,
    cur_planet: u32,
    target_planet: u32,
    from_pos: Vec2,
    to_pos: Vec2,
    move_t: f32,
    phase: ExplorerPhase,
    phase_timer: Timer,
    // so react_other plays once each time they meet, not every frame
    was_co_located: bool,
    // One entry per ExplorerMoved event. An explorer can move several times
    // between two snapshots (250ms), so we queue the hops and animate each one
    // instead of jumping straight to the last planet.
    pending_hops: VecDeque<u32>,
}

impl ExplorerAnim {
    fn new_viviana(start_planet: u32, tags: &AnimTags) -> Self {
        let pos = explorer_offset_pos(start_planet, 1);
        Self {
            moving_range: tags.viviana("moving"),
            arrived_range: tags.viviana("arrived"),
            departing_range: tags.viviana("departing"),
            settled_range: tags.viviana("collecting"),
            react_other_range: tags.viviana("react_other"),
            fps: 5.0,
            anim_timer: Timer::from_seconds(1.0 / 5.0, TimerMode::Repeating),
            cur_planet: start_planet,
            target_planet: start_planet,
            from_pos: pos,
            to_pos: pos,
            move_t: 0.0,
            phase: ExplorerPhase::Settled,
            phase_timer: Timer::from_seconds(0.8, TimerMode::Once),
            was_co_located: false,
            pending_hops: VecDeque::new(),
        }
    }

    fn new_jeb(start_planet: u32, tags: &AnimTags) -> Self {
        let pos = explorer_offset_pos(start_planet, 2);
        Self {
            moving_range: tags.jeb("moving"),
            arrived_range: tags.jeb("arrived"),
            departing_range: tags.jeb("departing"),
            settled_range: tags.jeb("idle"),
            react_other_range: tags.jeb("react_other"),
            fps: 5.0,
            anim_timer: Timer::from_seconds(1.0 / 5.0, TimerMode::Repeating),
            cur_planet: start_planet,
            target_planet: start_planet,
            from_pos: pos,
            to_pos: pos,
            move_t: 0.0,
            phase: ExplorerPhase::Settled,
            phase_timer: Timer::from_seconds(0.8, TimerMode::Once),
            was_co_located: false,
            pending_hops: VecDeque::new(),
        }
    }
}

#[derive(Component)]
struct PortraitTag(Personality);

#[derive(Component)]
struct VignetteTag;

#[derive(Component)]
struct PortraitConfig {
    idle_first: usize,
    idle_last: usize,
    idle_fps: f32,
    react_fps: f32,
    timer: Timer,
    cur_first: usize,
    cur_last: usize,
    looping: bool,
    hold_secs: f32,
    holding: bool,
    hold_timer: Timer,
    pending: VecDeque<(usize, usize)>,
}

impl PortraitConfig {
    fn new(idle_first: usize, idle_last: usize, idle_fps: f32, react_fps: f32) -> Self {
        Self {
            idle_first,
            idle_last,
            idle_fps,
            react_fps,
            timer: Timer::from_seconds(1.0 / idle_fps, TimerMode::Repeating),
            cur_first: idle_first,
            cur_last: idle_last,
            looping: true,
            hold_secs: 0.8,
            holding: false,
            hold_timer: Timer::from_seconds(0.8, TimerMode::Once),
            pending: VecDeque::new(),
        }
    }

    fn react(&mut self, first: usize, last: usize, atlas_index: &mut usize) {
        self.cur_first = first;
        self.cur_last = last;
        self.looping = false;
        self.holding = false;
        self.timer = Timer::from_seconds(1.0 / self.react_fps, TimerMode::Repeating);
        *atlas_index = first;
    }

    fn return_to_idle(&mut self, atlas_index: &mut usize) {
        self.cur_first = self.idle_first;
        self.cur_last = self.idle_last;
        self.looping = true;
        self.holding = false;
        self.timer = Timer::from_seconds(1.0 / self.idle_fps, TimerMode::Repeating);
        *atlas_index = self.idle_first;
    }
}

struct PortraitReaction {
    target: Personality,
    first: usize,
    last: usize,
    // true = interrupt the current one and clear the queue
    priority: bool,
}

// --- Animation tags ---
//
// Frame ranges are read by tag name from the Aseprite JSON exports, so
// re-exporting a sheet with a different number of frames doesn't break the
// animations.
#[derive(serde::Deserialize)]
struct AsepriteFrameTag {
    name: String,
    from: usize,
    to: usize,
}

#[derive(serde::Deserialize)]
struct AsepriteMeta {
    #[serde(rename = "frameTags")]
    frame_tags: Vec<AsepriteFrameTag>,
}

#[derive(serde::Deserialize)]
struct AsepriteExport {
    meta: AsepriteMeta,
}

#[derive(Resource)]
struct AnimTags {
    solace: HashMap<String, (usize, usize)>,
    eclipse: HashMap<String, (usize, usize)>,
    solace_stance: HashMap<String, (usize, usize)>,
    eclipse_stance: HashMap<String, (usize, usize)>,
    jeb: HashMap<String, (usize, usize)>,
    viviana: HashMap<String, (usize, usize)>,
    // all 7 planet sheets have the same tags, so one map is enough
    planet: HashMap<String, (usize, usize)>,
    sunray: HashMap<String, (usize, usize)>,
    asteroid: HashMap<String, (usize, usize)>,
}

impl AnimTags {
    fn load() -> Self {
        Self {
            solace: Self::load_one("solace.json"),
            eclipse: Self::load_one("eclipse.json"),
            solace_stance: Self::load_one("solace_stance.json"),
            eclipse_stance: Self::load_one("eclipse_stance.json"),
            jeb: Self::load_one("jeb.json"),
            viviana: Self::load_one("viviana.json"),
            planet: Self::load_one("planet_orbitron.json"),
            sunray: Self::load_one("sunray.json"),
            asteroid: Self::load_one("asteroid.json"),
        }
    }

    fn load_one(file: &str) -> HashMap<String, (usize, usize)> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join(file);
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!("failed to read animation tags from {}: {e}", path.display())
        });
        let export: AsepriteExport = serde_json::from_str(&text).unwrap_or_else(|e| {
            panic!(
                "failed to parse animation tags from {}: {e}",
                path.display()
            )
        });
        export
            .meta
            .frame_tags
            .into_iter()
            .map(|t| (t.name, (t.from, t.to)))
            .collect()
    }

    // Panics on unknown tags on purpose: a typo should crash at startup, not
    // silently play the wrong frames.
    fn get(&self, personality: &Personality, tag: &str) -> (usize, usize) {
        let map = match personality {
            Personality::Solace => &self.solace,
            Personality::Eclipse => &self.eclipse,
        };
        Self::lookup(map, tag, "solace.json/eclipse.json")
    }

    fn stance(&self, personality: &Personality, tag: &str) -> (usize, usize) {
        let map = match personality {
            Personality::Solace => &self.solace_stance,
            Personality::Eclipse => &self.eclipse_stance,
        };
        Self::lookup(map, tag, "solace_stance.json/eclipse_stance.json")
    }

    fn jeb(&self, tag: &str) -> (usize, usize) {
        Self::lookup(&self.jeb, tag, "jeb.json")
    }

    fn viviana(&self, tag: &str) -> (usize, usize) {
        Self::lookup(&self.viviana, tag, "viviana.json")
    }

    fn planet(&self, tag: &str) -> (usize, usize) {
        Self::lookup(&self.planet, tag, "planet_*.json")
    }

    fn sunray(&self, tag: &str) -> (usize, usize) {
        Self::lookup(&self.sunray, tag, "sunray.json")
    }

    fn asteroid(&self, tag: &str) -> (usize, usize) {
        Self::lookup(&self.asteroid, tag, "asteroid.json")
    }

    fn lookup(map: &HashMap<String, (usize, usize)>, tag: &str, source: &str) -> (usize, usize) {
        *map.get(tag)
            .unwrap_or_else(|| panic!("unknown animation tag {tag:?} — check {source}'s frameTags"))
    }
}

struct PlanetReaction {
    planet_id: u32,
    first: usize,
    last: usize,
}

#[derive(Resource, Default)]
struct PlanetReactionQueue(VecDeque<PlanetReaction>);

#[derive(Resource, Default)]
struct PortraitEventQueue(VecDeque<PortraitReaction>);

// --- Background movement ---

// Nebulas just bob around a fixed point with a sine wave, so there's no
// wrap-around to deal with.
#[derive(Component)]
struct DriftBob {
    base: Vec2,
    amp: Vec2,
    speed: f32,
    phase: f32,
    rot_speed: f32,
}

// only the active personality's nebula is faded in
#[derive(Component)]
struct NebulaTag(Personality);

// Both portraits are always on screen, but only one stance is: Solace and
// Eclipse share one body, so she can only be standing there once.
#[derive(Component)]
struct StanceTag(Personality);

// Stances only ever idle, so they don't need PortraitConfig.
#[derive(Component)]
struct StanceAnim {
    first: usize,
    last: usize,
    timer: Timer,
}

impl StanceAnim {
    fn new(range: (usize, usize)) -> Self {
        Self {
            first: range.0,
            last: range.1,
            timer: Timer::from_seconds(1.0 / 6.0, TimerMode::Repeating),
        }
    }
}

#[derive(Component)]
struct Twinkle {
    speed: f32,
    phase: f32,
    base_alpha: f32,
    pulse_alpha: f32,
}

// DriftBob for stars inside UI panels (moves Node.left/top, not Transform)
#[derive(Component)]
struct PanelStarBob {
    base: Vec2,
    amp: Vec2,
    speed: f32,
    phase: f32,
}

#[derive(Component)]
struct ShootingStar {
    velocity: Vec2,
    life: Timer,
}

#[derive(Resource)]
struct ShootingStarTimer(Timer);

impl Default for ShootingStarTimer {
    fn default() -> Self {
        Self(Timer::from_seconds(20.0, TimerMode::Once))
    }
}

// star cursor (the OS cursor is hidden)
#[derive(Component)]
struct CursorTag;

// --- Galaxy layout ---

// Same as galaxy.txt: each planet connects to the next one and the one after.
const CONNECTIONS: &[(usize, usize)] = &[
    (0, 1),
    (1, 2),
    (2, 3),
    (3, 4),
    (4, 5),
    (5, 6),
    (6, 0),
    (0, 2),
    (1, 3),
    (2, 4),
    (3, 5),
    (4, 6),
    (5, 0),
    (6, 1),
];

// An ellipse because the viewport is much wider than it is tall. With a
// circle the top planet went under the top screen. Leaves ~140px below the
// top screen and ~50px above the console.
const RING_RADIUS_X: f32 = 230.0 * DISPLAY_SCALE;
const RING_RADIUS_Y: f32 = 175.0 * DISPLAY_SCALE;

// The viewport is a bit above screen center (the bottom console is way
// taller than the top screen), so the ring moves up with it.
const RING_CENTER: Vec2 = Vec2::new(0.0, 78.0 * DISPLAY_SCALE);

fn planet_position(index: usize) -> Vec2 {
    let angle = FRAC_PI_2 - index as f32 * 2.0 * PI / 7.0;
    RING_CENTER + Vec2::new(RING_RADIUS_X * angle.cos(), RING_RADIUS_Y * angle.sin())
}

// Top-left of the stance in UI coords. Also used by the projectiles, since
// sunrays/asteroids are thrown from the stance.
fn stance_ui_top_left(personality: &Personality) -> Vec2 {
    let stance_ring_half_width = RING_RADIUS_X + 48.0 * DISPLAY_SCALE;
    let stance_gap = 48.0 * DISPLAY_SCALE;
    let stance_ring_center_ui = Vec2::new(
        WINDOW_WIDTH / 2.0 + RING_CENTER.x,
        WINDOW_HEIGHT / 2.0 - RING_CENTER.y,
    );
    let stance_top_y = stance_ring_center_ui.y - STANCE_SIZE / 2.0;
    match personality {
        Personality::Solace => Vec2::new(
            stance_ring_center_ui.x - stance_ring_half_width - stance_gap - STANCE_SIZE,
            stance_top_y,
        ),
        Personality::Eclipse => Vec2::new(
            stance_ring_center_ui.x + stance_ring_half_width + stance_gap,
            stance_top_y,
        ),
    }
}

// Same point in world coords, because projectiles are sprites and the stance
// is a UI node.
fn stance_world_pos(personality: &Personality) -> Vec2 {
    let ui_center = stance_ui_top_left(personality) + Vec2::splat(STANCE_SIZE / 2.0);
    Vec2::new(
        ui_center.x - WINDOW_WIDTH / 2.0,
        WINDOW_HEIGHT / 2.0 - ui_center.y,
    )
}

fn explorer_offset_pos(planet_id: u32, explorer_id: u32) -> Vec2 {
    let ring = planet_id.saturating_sub(1) as usize;
    let base = planet_position(ring);
    let offset = if explorer_id == 2 {
        Vec2::new(36.0, 36.0) * DISPLAY_SCALE
    } else {
        Vec2::new(-36.0, 36.0) * DISPLAY_SCALE
    };
    base + offset
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn main() {
    let snapshot: Arc<Mutex<Option<GalaxySnapshot>>> = Arc::new(Mutex::new(None));
    let snap_write = Arc::clone(&snapshot);

    let inspection = Inspection(Arc::new(Mutex::new(InspectionData::None)));
    let inspection_write = inspection.clone();

    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<UiCommand>();
    let (ready_tx, ready_rx) = crossbeam_channel::bounded::<()>(1);

    std::thread::spawn(move || {
        // wait until setup() has the scene on screen
        let _ = ready_rx.recv();

        let mut api = bipolar_orchestrator::builder::build_api(GALAXY_SRC, PLANETS_SRC);

        // Logic waits for the Start button, so in a demo I can show
        // everything first before things start moving.
        *snap_write.lock().unwrap() = Some(bipolar_orchestrator::snapshot::build(&api));

        let mut logic_running = false;
        loop {
            drain_ui_commands(&cmd_rx, &mut api, &mut logic_running, &inspection_write);

            std::thread::sleep(Duration::from_millis(250));
            let s = bipolar_orchestrator::snapshot::build(&api);
            *snap_write.lock().unwrap() = Some(s);
        }
    });

    App::new()
        .add_plugins(
            DefaultPlugins
                .set(ImagePlugin::default_nearest())
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Bipolar Orchestrator".to_string(),
                        // Ignore Windows display scaling, otherwise the UI
                        // (Val::Px) gets scaled again and stops lining up
                        // with the cockpit art.
                        resolution: WindowResolution::new(
                            WINDOW_WIDTH as u32,
                            WINDOW_HEIGHT as u32,
                        )
                        .with_scale_factor_override(1.0),
                        // Fullscreen for the presentation. This uses the
                        // monitor's resolution, which is why DISPLAY_SCALE
                        // is set for 1920x1080.
                        mode: bevy::window::WindowMode::BorderlessFullscreen(
                            bevy::window::MonitorSelection::Primary,
                        ),
                        ..default()
                    }),
                    ..default()
                }),
        )
        .insert_resource(ClearColor(Color::srgb(0.04, 0.04, 0.10)))
        .insert_resource(LiveSnapshot(snapshot))
        .insert_resource(GalaxyState::default())
        .insert_resource(PortraitEventQueue::default())
        .insert_resource(PlanetReactionQueue::default())
        .insert_resource(UiCommands(cmd_tx))
        .insert_resource(Selection::default())
        .insert_resource(Inspecting::default())
        .insert_resource(inspection)
        .insert_resource(AnimTags::load())
        .insert_resource(PlanetNames::load())
        .insert_resource(LogicRunState::NotStarted)
        .insert_resource(ShootingStarTimer::default())
        .insert_resource(GlitchQueue::default())
        .insert_resource(MapHologramOpen::default())
        .insert_resource(SuspicionState::default())
        .insert_resource(HostilityTrend::default())
        .insert_resource(ReadySignal(Some(ready_tx)))
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                animate_sprites,
                draw_connections,
                poll_snapshot,
                drive_explorers,
                sync_planets,
                drive_planet_anim,
                drive_planet_reactions,
                sync_portrait_glow,
                drive_portraits,
                drive_glitch_overlays,
                sync_vignette,
                handle_input,
                draw_selection_highlight,
                update_top_bar_personality,
                update_top_bar_cycle,
                update_chronicle_screen_text,
                sync_transport_buttons,
                handle_sunray_button,
                handle_asteroid_button,
                cockpit_button_hover,
            ),
        )
        .add_systems(
            Update,
            (
                sync_hologram_visibility,
                sync_hologram_content,
                sync_explorers,
                sync_action_buttons_enabled,
                sync_selection_validity,
                sync_stance_visibility,
                drive_stance_anim,
                sync_resource_badges,
            ),
        )
        .add_systems(
            Update,
            (
                drive_drift_bob,
                sync_nebula_personality,
                drive_twinkle,
                drive_panel_stars,
                spawn_shooting_stars,
                drive_shooting_stars,
                follow_cursor,
                update_hostility_trend,
                update_top_bar_trend,
                sync_game_over_overlay,
                drive_projectiles,
            ),
        )
        .add_systems(
            Update,
            (
                toggle_map_hologram,
                sync_map_hologram_visibility,
                sync_map_nodes,
                sync_map_edges,
                update_map_clock_text,
                sync_suspicion_meter,
            ),
        )
        .run();
}

// Runs on the bridge thread. Same API calls the CLI makes.
fn drain_ui_commands(
    cmd_rx: &CmdReceiver<UiCommand>,
    api: &mut bipolar_orchestrator::api::OrchestratorApi,
    logic_running: &mut bool,
    inspection: &Inspection,
) {
    while let Ok(cmd) = cmd_rx.try_recv() {
        match cmd {
            UiCommand::Sunray(planet_id) => {
                if let Err(e) = api.send_sunray(planet_id) {
                    eprintln!("[bridge] manual sunray to {planet_id} failed: {e}");
                }
            }
            UiCommand::Asteroid(planet_id) => {
                if let Err(e) = api.send_asteroid(planet_id) {
                    eprintln!("[bridge] manual asteroid to {planet_id} failed: {e}");
                }
            }
            UiCommand::MoveExplorer { explorer_id, dst } => {
                if let Err(e) = api.move_explorer(explorer_id, dst) {
                    eprintln!("[bridge] move explorer {explorer_id} -> {dst} failed: {e}");
                }
            }
            UiCommand::ToggleLogic => {
                if *logic_running {
                    match api.stop_logic() {
                        Ok(()) => *logic_running = false,
                        Err(e) => eprintln!("[bridge] stop_logic failed: {e}"),
                    }
                } else {
                    match api.start_logic() {
                        Ok(()) => *logic_running = true,
                        Err(e) => eprintln!("[bridge] start_logic failed: {e}"),
                    }
                }
            }
            UiCommand::InspectExplorer(id) => {
                let bag_lines = match api.bag_content(id) {
                    Ok(bag) if bag.is_empty() => vec!["(bag is empty)".to_string()],
                    Ok(bag) => bag
                        .iter()
                        .map(|(res, count)| format!("{res:?} x{count}"))
                        .collect(),
                    Err(e) => vec![format!("(failed to fetch bag: {e})")],
                };
                *inspection.0.lock().unwrap() = InspectionData::ExplorerBag { id, bag_lines };
            }
            UiCommand::InspectPlanet(id) => {
                let data = match api.planet_state(id) {
                    Ok(s) => InspectionData::PlanetInfo {
                        id,
                        energy_cells: s.energy_cells.len(),
                        charged_cells: s.charged_cells_count,
                        has_rocket: s.has_rocket,
                    },
                    Err(e) => {
                        eprintln!("[bridge] planet_state({id}) failed: {e}");
                        InspectionData::PlanetInfo {
                            id,
                            energy_cells: 0,
                            charged_cells: 0,
                            has_rocket: false,
                        }
                    }
                };
                *inspection.0.lock().unwrap() = data;
            }
            UiCommand::DebugNudgeHostility(delta) => {
                api.debug_nudge_hostility(delta);
            }
        }
    }
}

// --- Reading snapshots ---

fn poll_snapshot(
    mut commands: Commands,
    live: Res<LiveSnapshot>,
    tags: Res<AnimTags>,
    projectile_assets: Res<ProjectileAssets>,
    mut state: ResMut<GalaxyState>,
    mut queue: ResMut<PortraitEventQueue>,
    mut planet_queue: ResMut<PlanetReactionQueue>,
    mut explorer_anims: Query<(&ExplorerTag, &mut ExplorerAnim)>,
) {
    let Ok(mut guard) = live.0.try_lock() else {
        return;
    };
    let Some(snap) = guard.as_mut() else { return };
    // Take the events out. This runs every frame but a snapshot only comes
    // every 250ms, so without take() the same events got handled over and over
    // and the reaction queue filled up.
    let events = std::mem::take(&mut snap.events);
    state.ready = true;

    let new_personality = snap.personality.clone();

    if new_personality != state.personality {
        // loser plays "breaking", winner plays "awakening"
        let (loser, winner) = match &new_personality {
            Personality::Eclipse => (Personality::Solace, Personality::Eclipse),
            Personality::Solace => (Personality::Eclipse, Personality::Solace),
        };
        let loser_break = tags.get(&loser, "breaking");
        let winner_awaken = tags.get(&winner, "awakening");
        queue.0.push_back(PortraitReaction {
            target: loser,
            first: loser_break.0,
            last: loser_break.1,
            priority: true,
        });
        queue.0.push_back(PortraitReaction {
            target: winner,
            first: winner_awaken.0,
            last: winner_awaken.1,
            priority: true,
        });
    }

    // state.personality is still the old one here, which is who these
    // events actually happened under
    for event in &events {
        match event {
            GalaxyEvent::SunraySent { planet_id } => {
                // Eclipse doesn't want sunrays
                let tag = match state.personality {
                    Personality::Solace => "sending",
                    Personality::Eclipse => "unwanted_event",
                };
                let (first, last) = tags.get(&state.personality, tag);
                queue.0.push_back(PortraitReaction {
                    target: state.personality.clone(),
                    first,
                    last,
                    priority: false,
                });
                spawn_projectile(
                    &mut commands,
                    &projectile_assets,
                    &tags,
                    ProjectileKind::Sunray,
                    *planet_id,
                );
            }
            GalaxyEvent::SunrayReceived { planet_id } => {
                if state.personality == Personality::Solace {
                    let (first, last) = tags.get(&Personality::Solace, "satisfied");
                    queue.0.push_back(PortraitReaction {
                        target: Personality::Solace,
                        first,
                        last,
                        priority: false,
                    });
                }
                let (first, last) = tags.planet("receive");
                planet_queue.0.push_back(PlanetReaction {
                    planet_id: *planet_id,
                    first,
                    last,
                });
            }
            GalaxyEvent::AsteroidSent { planet_id } => {
                // Solace feeling guilty is priority so it doesn't get lost
                // behind all the normal sunray reactions
                let (tag, priority) = match state.personality {
                    Personality::Eclipse => ("sending", false),
                    Personality::Solace => ("unwanted_event", true),
                };
                let (first, last) = tags.get(&state.personality, tag);
                queue.0.push_back(PortraitReaction {
                    target: state.personality.clone(),
                    first,
                    last,
                    priority,
                });
                spawn_projectile(
                    &mut commands,
                    &projectile_assets,
                    &tags,
                    ProjectileKind::Asteroid,
                    *planet_id,
                );
            }
            GalaxyEvent::AsteroidDeflected { planet_id } => {
                if state.personality == Personality::Eclipse {
                    let (first, last) = tags.get(&Personality::Eclipse, "satisfied");
                    queue.0.push_back(PortraitReaction {
                        target: Personality::Eclipse,
                        first,
                        last,
                        priority: false,
                    });
                }
                let (first, last) = tags.planet("hit");
                planet_queue.0.push_back(PlanetReaction {
                    planet_id: *planet_id,
                    first,
                    last,
                });
            }
            GalaxyEvent::PlanetDestroyed { planet_id } => {
                // portraits react through the personality flip instead
                let (first, last) = tags.planet("hit");
                planet_queue.0.push_back(PlanetReaction {
                    planet_id: *planet_id,
                    first,
                    last,
                });
            }
            GalaxyEvent::ExplorerKilled { explorer_id } => {
                // TODO: death animation. The sprite is hidden by sync_explorers.
                println!("[gui] Explorer {explorer_id} died with their planet");
            }
            GalaxyEvent::ExplorerMoved {
                explorer_id, to, ..
            } => {
                // see pending_hops
                for (tag, mut ea) in &mut explorer_anims {
                    if tag.0 == *explorer_id {
                        ea.pending_hops.push_back(*to);
                    }
                }
            }
        }
    }

    state.personality = new_personality;
    state.alive = snap.alive_planets.iter().copied().collect();
    state.explorer_planet = snap.explorers.iter().map(|e| (e.id, e.planet)).collect();
    state.explorer_bag = snap
        .explorers
        .iter()
        .map(|e| (e.id, e.bag.clone()))
        .collect();
    state.neighbors.clone_from(&snap.neighbors);
    state.hostility = snap.hostility;
    state.phase_elapsed = snap.phase_elapsed;
}

// Unselect planets/explorers that died, otherwise the buttons keep firing at a
// planet that doesn't exist and it looks like they're broken.
fn sync_selection_validity(state: Res<GalaxyState>, mut selection: ResMut<Selection>) {
    if !state.ready {
        return;
    }
    if let Some(p) = selection.planet
        && !state.alive.contains(&p)
    {
        selection.planet = None;
    }
    if let Some(e) = selection.explorer
        && !state.explorer_planet.contains_key(&e)
    {
        selection.explorer = None;
    }
}

// --- Explorers ---

// hide explorers that are gone from the snapshot (died with their planet)
fn sync_explorers(state: Res<GalaxyState>, mut query: Query<(&ExplorerTag, &mut Visibility)>) {
    if !state.ready {
        return;
    }
    for (tag, mut vis) in &mut query {
        *vis = if state.explorer_planet.contains_key(&tag.0) {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}

fn spawn_resource_badges(commands: &mut Commands, explorer_id: u32) {
    for slot in 0..RESOURCE_BADGE_SLOTS {
        commands.spawn((
            Sprite {
                custom_size: Some(Vec2::new(20.0, 20.0) * DISPLAY_SCALE),
                ..default()
            },
            Transform::from_xyz(0.0, 0.0, 3.0),
            Visibility::Hidden,
            ResourceBadge { explorer_id, slot },
        ));
    }
}

fn sync_resource_badges(
    state: Res<GalaxyState>,
    icons: Res<ResourceIcons>,
    explorer_q: Query<(&ExplorerTag, &Transform), Without<ResourceBadge>>,
    mut badge_q: Query<
        (&ResourceBadge, &mut Transform, &mut Sprite, &mut Visibility),
        Without<ExplorerTag>,
    >,
) {
    let positions: HashMap<u32, Vec2> = explorer_q
        .iter()
        .map(|(tag, tf)| (tag.0, Vec2::new(tf.translation.x, tf.translation.y)))
        .collect();

    for (badge, mut tf, mut sprite, mut vis) in &mut badge_q {
        let Some(&pos) = positions.get(&badge.explorer_id) else {
            *vis = Visibility::Hidden;
            continue;
        };
        let held = state
            .explorer_bag
            .get(&badge.explorer_id)
            .and_then(|bag| bag.get(badge.slot));

        match held {
            Some(&(kind, _count)) => {
                let x_offset = (badge.slot as f32 - (RESOURCE_BADGE_SLOTS as f32 - 1.0) / 2.0)
                    * 22.0
                    * DISPLAY_SCALE;
                tf.translation = Vec3::new(pos.x + x_offset, pos.y + 46.0 * DISPLAY_SCALE, 3.0);
                sprite.image = icons.0[&kind].clone();
                *vis = Visibility::Inherited;
            }
            None => *vis = Visibility::Hidden,
        }
    }
}

fn drive_explorers(
    state: Res<GalaxyState>,
    time: Res<Time>,
    mut query: Query<(&ExplorerTag, &mut Transform, &mut Sprite, &mut ExplorerAnim)>,
) {
    const MOVE_SECS: f32 = 2.0;
    const DEPART_HOLD: f32 = 0.8;
    const ARRIVE_HOLD: f32 = 0.8;

    let delta = time.delta();

    // copied out so the co-location check can see the other explorer
    // without a second query
    let backend_positions: Vec<(u32, u32)> = state
        .explorer_planet
        .iter()
        .map(|(&id, &p)| (id, p))
        .collect();

    for (tag, mut tf, mut sprite, mut ea) in &mut query {
        ea.anim_timer.tick(delta);
        let tick = ea.anim_timer.just_finished();

        // finish the current hop before starting the next one
        let can_start_new_move = matches!(
            ea.phase,
            ExplorerPhase::Settled | ExplorerPhase::ReactingOther
        );

        if can_start_new_move {
            // Use the queued hops first, and fall back to the snapshot
            // position in case a move event got missed.
            // Skip hops to planets that died in the meantime (the queue can be
            // a few seconds behind), otherwise the explorer walks to empty
            // space and stands there.
            let mut next = None;
            while let Some(bp) = ea.pending_hops.pop_front() {
                if state.alive.contains(&bp) {
                    next = Some(bp);
                    break;
                }
            }
            let next = next.or_else(|| {
                state
                    .explorer_planet
                    .get(&tag.0)
                    .copied()
                    .filter(|&bp| bp != ea.cur_planet)
            });

            if let Some(bp) = next {
                ea.target_planet = bp;
                ea.to_pos = explorer_offset_pos(bp, tag.0);
                ea.from_pos = Vec2::new(tf.translation.x, tf.translation.y);
                ea.anim_timer = Timer::from_seconds(1.0 / ea.fps, TimerMode::Repeating);
                ea.phase = ExplorerPhase::Departing;
                ea.phase_timer = Timer::from_seconds(DEPART_HOLD, TimerMode::Once);
                ea.was_co_located = false;
                if let Some(atlas) = &mut sprite.texture_atlas {
                    atlas.index = ea.departing_range.0;
                }
            }
        }

        match ea.phase {
            ExplorerPhase::Settled => {
                if tick && let Some(atlas) = &mut sprite.texture_atlas {
                    let (f, l) = ea.settled_range;
                    if atlas.index >= l {
                        atlas.index = f;
                    } else {
                        atlas.index += 1;
                    }
                }
                // Compare backend positions for both explorers. cur_planet can
                // be a few hops behind (see pending_hops), and using it made
                // this almost never trigger.
                let co_located = state.explorer_planet.get(&tag.0).is_some_and(|&my_p| {
                    backend_positions
                        .iter()
                        .any(|&(id, p)| id != tag.0 && p == my_p)
                });
                if co_located && !ea.was_co_located {
                    ea.phase = ExplorerPhase::ReactingOther;
                    ea.phase_timer = Timer::from_seconds(CO_REACT_HOLD_SECS, TimerMode::Once);
                    ea.anim_timer = Timer::from_seconds(1.0 / ea.fps, TimerMode::Repeating);
                    if let Some(atlas) = &mut sprite.texture_atlas {
                        atlas.index = ea.react_other_range.0;
                    }
                }
                ea.was_co_located = co_located;
            }

            ExplorerPhase::ReactingOther => {
                if tick && let Some(atlas) = &mut sprite.texture_atlas {
                    let (f, l) = ea.react_other_range;
                    if atlas.index >= l {
                        atlas.index = f;
                    } else {
                        atlas.index += 1;
                    }
                }
                ea.phase_timer.tick(delta);
                if ea.phase_timer.just_finished() {
                    ea.phase = ExplorerPhase::Settled;
                    ea.anim_timer = Timer::from_seconds(1.0 / ea.fps, TimerMode::Repeating);
                    if let Some(atlas) = &mut sprite.texture_atlas {
                        atlas.index = ea.settled_range.0;
                    }
                }
            }

            ExplorerPhase::Departing => {
                if tick && let Some(atlas) = &mut sprite.texture_atlas {
                    let (_, l) = ea.departing_range;
                    if atlas.index < l {
                        atlas.index += 1;
                    }
                }
                ea.phase_timer.tick(delta);
                if ea.phase_timer.just_finished() {
                    ea.phase = ExplorerPhase::Moving;
                    ea.move_t = 0.0;
                    ea.anim_timer = Timer::from_seconds(1.0 / ea.fps, TimerMode::Repeating);
                    if let Some(atlas) = &mut sprite.texture_atlas {
                        atlas.index = ea.moving_range.0;
                    }
                }
            }

            ExplorerPhase::Moving => {
                ea.move_t += delta.as_secs_f32() / MOVE_SECS;
                if tick && let Some(atlas) = &mut sprite.texture_atlas {
                    let (f, l) = ea.moving_range;
                    if atlas.index >= l {
                        atlas.index = f;
                    } else {
                        atlas.index += 1;
                    }
                }
                if ea.move_t >= 1.0 {
                    ea.cur_planet = ea.target_planet;
                    tf.translation.x = ea.to_pos.x;
                    tf.translation.y = ea.to_pos.y;
                    ea.phase = ExplorerPhase::Arrived;
                    ea.phase_timer = Timer::from_seconds(ARRIVE_HOLD, TimerMode::Once);
                    ea.anim_timer = Timer::from_seconds(1.0 / ea.fps, TimerMode::Repeating);
                    if let Some(atlas) = &mut sprite.texture_atlas {
                        atlas.index = ea.arrived_range.0;
                    }
                } else {
                    let pos = ea.from_pos.lerp(ea.to_pos, smoothstep(ea.move_t));
                    tf.translation.x = pos.x;
                    tf.translation.y = pos.y;
                }
            }

            ExplorerPhase::Arrived => {
                if tick && let Some(atlas) = &mut sprite.texture_atlas {
                    let (_, l) = ea.arrived_range;
                    if atlas.index < l {
                        atlas.index += 1;
                    }
                }
                ea.phase_timer.tick(delta);
                if ea.phase_timer.just_finished() {
                    ea.phase = ExplorerPhase::Settled;
                    ea.anim_timer = Timer::from_seconds(1.0 / ea.fps, TimerMode::Repeating);
                    if let Some(atlas) = &mut sprite.texture_atlas {
                        atlas.index = ea.settled_range.0;
                    }
                }
            }
        }
    }
}

// --- Sunray / asteroid projectiles ---
//
// Spawned on SunraySent/AsteroidSent and flown from the stance to the planet.
// They always play "impact" at the end, even if the asteroid gets deflected.
// The planet's own hit/receive animation is what shows what happened.

// fast enough that the farthest planet takes under 1.5s
const PROJECTILE_SPEED: f32 = 700.0 * DISPLAY_SCALE;
const PROJECTILE_MIN_TRAVEL_SECS: f32 = 0.35;
const PROJECTILE_SIZE: f32 = 40.0 * DISPLAY_SCALE;

#[derive(Clone, Copy, PartialEq)]
enum ProjectileKind {
    Sunray,
    Asteroid,
}

#[derive(Component)]
struct Projectile {
    from: Vec2,
    to: Vec2,
    travel_secs: f32,
    elapsed: f32,
    impacting: bool,
    anim_timer: Timer,
    moving_range: (usize, usize),
    impact_range: (usize, usize),
}

#[derive(Resource)]
struct ProjectileAssets {
    sunray_image: Handle<Image>,
    sunray_layout: Handle<TextureAtlasLayout>,
    asteroid_image: Handle<Image>,
    asteroid_layout: Handle<TextureAtlasLayout>,
}

impl ProjectileAssets {
    fn load(asset_server: &AssetServer, layouts: &mut Assets<TextureAtlasLayout>) -> Self {
        Self {
            sunray_image: asset_server.load("sunray.png"),
            sunray_layout: layouts.add(TextureAtlasLayout::from_grid(
                UVec2::new(16, 16),
                6,
                1,
                None,
                None,
            )),
            asteroid_image: asset_server.load("asteroid.png"),
            asteroid_layout: layouts.add(TextureAtlasLayout::from_grid(
                UVec2::new(16, 16),
                9,
                1,
                None,
                None,
            )),
        }
    }
}

// sunrays come from Solace, asteroids from Eclipse
fn spawn_projectile(
    commands: &mut Commands,
    assets: &ProjectileAssets,
    tags: &AnimTags,
    kind: ProjectileKind,
    planet_id: u32,
) {
    let (image, layout, moving, impact, origin) = match kind {
        ProjectileKind::Sunray => (
            assets.sunray_image.clone(),
            assets.sunray_layout.clone(),
            tags.sunray("moving"),
            tags.sunray("impact"),
            Personality::Solace,
        ),
        ProjectileKind::Asteroid => (
            assets.asteroid_image.clone(),
            assets.asteroid_layout.clone(),
            tags.asteroid("moving"),
            tags.asteroid("impact"),
            Personality::Eclipse,
        ),
    };
    let from = stance_world_pos(&origin);
    let to = planet_position(planet_id.saturating_sub(1) as usize);
    let travel_secs = ((to - from).length() / PROJECTILE_SPEED).max(PROJECTILE_MIN_TRAVEL_SECS);

    commands.spawn((
        Sprite {
            image,
            texture_atlas: Some(TextureAtlas {
                layout,
                index: moving.0,
            }),
            custom_size: Some(Vec2::splat(PROJECTILE_SIZE)),
            ..default()
        },
        Transform::from_xyz(from.x, from.y, 5.0),
        Projectile {
            from,
            to,
            travel_secs,
            elapsed: 0.0,
            impacting: false,
            anim_timer: Timer::from_seconds(1.0 / 12.0, TimerMode::Repeating),
            moving_range: moving,
            impact_range: impact,
        },
    ));
}

fn drive_projectiles(
    mut commands: Commands,
    time: Res<Time>,
    mut query: Query<(Entity, &mut Projectile, &mut Transform, &mut Sprite)>,
) {
    let delta = time.delta();
    for (entity, mut proj, mut tf, mut sprite) in &mut query {
        proj.anim_timer.tick(delta);

        if !proj.impacting {
            proj.elapsed += delta.as_secs_f32();
            let t = (proj.elapsed / proj.travel_secs).min(1.0);
            let pos = proj.from.lerp(proj.to, t);
            let dir = (proj.to - proj.from).normalize_or_zero();
            tf.translation = Vec3::new(pos.x, pos.y, 5.0);
            tf.rotation = Quat::from_rotation_z(dir.y.atan2(dir.x));

            if proj.anim_timer.just_finished()
                && let Some(atlas) = &mut sprite.texture_atlas
            {
                let (f, l) = proj.moving_range;
                if atlas.index >= l {
                    atlas.index = f;
                } else {
                    atlas.index += 1;
                }
            }

            if t >= 1.0 {
                proj.impacting = true;
                proj.anim_timer = Timer::from_seconds(1.0 / 12.0, TimerMode::Repeating);
                if let Some(atlas) = &mut sprite.texture_atlas {
                    atlas.index = proj.impact_range.0;
                }
            }
        } else if proj.anim_timer.just_finished()
            && let Some(atlas) = &mut sprite.texture_atlas
        {
            let (_, l) = proj.impact_range;
            if atlas.index >= l {
                commands.entity(entity).despawn();
            } else {
                atlas.index += 1;
            }
        }
    }
}

// --- Planets ---

fn sync_planets(
    state: Res<GalaxyState>,
    tags: Res<AnimTags>,
    mut query: Query<(
        &PlanetTag,
        &mut Visibility,
        &mut PlanetState,
        &mut AnimationConfig,
        &mut Sprite,
    )>,
) {
    if !state.ready {
        return;
    }

    for (tag, mut vis, mut pstate, mut anim, mut sprite) in &mut query {
        match *pstate {
            PlanetState::Alive => {
                if state.alive.contains(&tag.0) {
                    *vis = Visibility::Inherited;
                } else {
                    // just died: play dying, then destroyed, then hide
                    let dying = tags.planet("dying");
                    let destroyed = tags.planet("destroyed");
                    *pstate = PlanetState::Dying;
                    anim.first = dying.0;
                    anim.last = dying.1;
                    anim.looping = false;
                    anim.next_range = Some(destroyed);
                    anim.pending_hide = true;
                    anim.timer = Timer::from_seconds(1.0 / 10.0, TimerMode::Repeating);
                    if let Some(atlas) = &mut sprite.texture_atlas {
                        atlas.index = dying.0;
                    }
                }
            }
            // drive_planet_anim takes care of these
            PlanetState::Dying | PlanetState::Dead => {}
        }
    }
}

fn drive_planet_anim(
    time: Res<Time>,
    tags: Res<AnimTags>,
    mut query: Query<(
        &mut Sprite,
        &mut AnimationConfig,
        &mut PlanetState,
        &mut Visibility,
    )>,
) {
    for (mut sprite, mut anim, mut state, mut vis) in &mut query {
        anim.timer.tick(time.delta());
        if !anim.timer.just_finished() {
            continue;
        }
        if let Some(atlas) = &mut sprite.texture_atlas {
            if atlas.index >= anim.last {
                if anim.looping {
                    atlas.index = anim.first;
                } else if let Some((nf, nl)) = anim.next_range.take() {
                    anim.first = nf;
                    anim.last = nl;
                    atlas.index = nf;
                } else if anim.pending_hide {
                    *state = PlanetState::Dead;
                    *vis = Visibility::Hidden;
                } else if *state == PlanetState::Alive {
                    // hit/receive finished, back to idle
                    let idle = tags.planet("idle");
                    anim.first = idle.0;
                    anim.last = idle.1;
                    anim.looping = true;
                    anim.timer = Timer::from_seconds(1.0 / 10.0, TimerMode::Repeating);
                    atlas.index = idle.0;
                }
            } else {
                atlas.index += 1;
            }
        }
    }
}

fn drive_planet_reactions(
    mut queue: ResMut<PlanetReactionQueue>,
    mut query: Query<(&PlanetTag, &mut Sprite, &mut AnimationConfig, &PlanetState)>,
) {
    while let Some(reaction) = queue.0.pop_front() {
        for (tag, mut sprite, mut anim, state) in &mut query {
            if tag.0 == reaction.planet_id && *state == PlanetState::Alive {
                anim.first = reaction.first;
                anim.last = reaction.last;
                anim.looping = false;
                anim.next_range = None;
                anim.pending_hide = false;
                anim.timer = Timer::from_seconds(1.0 / 10.0, TimerMode::Repeating);
                if let Some(atlas) = &mut sprite.texture_atlas {
                    atlas.index = reaction.first;
                }
            }
        }
    }
}

// --- Portraits ---
//
// Both portraits are always shown, the one in control is brighter so you can
// tell who it is without reading anything.

const PORTRAIT_ACTIVE_TINT: Color = Color::srgba(1.0, 1.0, 1.0, 1.0);
const PORTRAIT_DIM_TINT: Color = Color::srgba(0.55, 0.55, 0.6, 0.55);

fn sync_portrait_glow(
    state: Res<GalaxyState>,
    mut query: Query<(&PortraitTag, &mut ImageNode), Without<GlitchOverlay>>,
) {
    for (tag, mut image) in &mut query {
        // fades with hostility instead of switching all at once
        let dominance = match tag.0 {
            Personality::Solace => 1.0 - state.hostility,
            Personality::Eclipse => state.hostility,
        } as f32;
        image.color = lerp_color(PORTRAIT_DIM_TINT, PORTRAIT_ACTIVE_TINT, dominance);
    }
}

fn drive_portraits(
    time: Res<Time>,
    mut queue: ResMut<PortraitEventQueue>,
    mut query: Query<(&PortraitTag, &mut ImageNode, &mut PortraitConfig)>,
) {
    while let Some(reaction) = queue.0.pop_front() {
        for (tag, mut sprite, mut cfg) in &mut query {
            if tag.0 == reaction.target {
                if reaction.priority || cfg.looping {
                    if let Some(atlas) = &mut sprite.texture_atlas {
                        cfg.pending.clear();
                        cfg.react(reaction.first, reaction.last, &mut atlas.index);
                    }
                } else {
                    cfg.pending.push_back((reaction.first, reaction.last));
                }
            }
        }
    }

    for (_, mut sprite, mut cfg) in &mut query {
        let delta = time.delta();

        if cfg.holding {
            cfg.hold_timer.tick(delta);
            if cfg.hold_timer.just_finished() {
                if let Some((nf, nl)) = cfg.pending.pop_front() {
                    if let Some(atlas) = &mut sprite.texture_atlas {
                        cfg.react(nf, nl, &mut atlas.index);
                    }
                } else if let Some(atlas) = &mut sprite.texture_atlas {
                    cfg.return_to_idle(&mut atlas.index);
                }
            }
            continue;
        }

        cfg.timer.tick(delta);
        if !cfg.timer.just_finished() {
            continue;
        }
        if let Some(atlas) = &mut sprite.texture_atlas {
            if atlas.index >= cfg.cur_last {
                if cfg.looping {
                    atlas.index = cfg.cur_first;
                } else {
                    cfg.holding = true;
                    cfg.hold_timer = Timer::from_seconds(cfg.hold_secs, TimerMode::Once);
                }
            } else {
                atlas.index += 1;
            }
        }
    }
}

fn sync_stance_visibility(
    state: Res<GalaxyState>,
    mut query: Query<(&StanceTag, &mut Visibility)>,
) {
    for (tag, mut vis) in &mut query {
        *vis = if tag.0 == state.personality {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

// runs on the hidden one too so it doesn't jump when it appears
fn drive_stance_anim(time: Res<Time>, mut query: Query<(&mut StanceAnim, &mut ImageNode)>) {
    for (mut anim, mut image) in &mut query {
        anim.timer.tick(time.delta());
        if anim.timer.just_finished()
            && let Some(atlas) = &mut image.texture_atlas
        {
            atlas.index = if atlas.index >= anim.last {
                anim.first
            } else {
                atlas.index + 1
            };
        }
    }
}

// One glitch frame, flickered with alpha. Looks like a hack without having to
// draw a whole animation.
fn drive_glitch_overlays(
    time: Res<Time>,
    mut glitch: ResMut<GlitchQueue>,
    mut query: Query<(&PortraitTag, &mut GlitchOverlay, &mut ImageNode)>,
) {
    for target in glitch.0.drain(..) {
        for (tag, mut overlay, _) in &mut query {
            if tag.0 == target {
                overlay.remaining = GLITCH_DURATION_SECS;
            }
        }
    }

    let dt = time.delta_secs();
    let elapsed = time.elapsed_secs();
    for (_, mut overlay, mut image) in &mut query {
        if overlay.remaining <= 0.0 {
            image.color.set_alpha(0.0);
            continue;
        }
        overlay.remaining = (overlay.remaining - dt).max(0.0);
        let fraction = overlay.remaining / GLITCH_DURATION_SECS;
        let flicker = (elapsed * 45.0).sin().abs() * 0.5 + 0.5;
        image.color.set_alpha(fraction * flicker);
    }
}

// the tier itself changes in apply_manual_override
fn sync_suspicion_meter(
    suspicion: Res<SuspicionState>,
    mut query: Query<(&SuspicionMeterFill, &mut Node)>,
) {
    let max_h = SUSPICION_METER_H - SUSPICION_FILL_INSET * 2.0;
    for (fill, mut node) in &mut query {
        let frac = f32::from(suspicion.tier(&fill.0)) / f32::from(SUSPICION_MAX_RESTING_TIER);
        node.height = Val::Px(max_h * frac);
    }
}

// --- Other animation ---

// planets have their own (drive_planet_anim)
fn animate_sprites(
    time: Res<Time>,
    mut query: Query<(&mut Sprite, &mut AnimationConfig), Without<PlanetState>>,
) {
    for (mut sprite, mut anim) in &mut query {
        anim.timer.tick(time.delta());
        if anim.timer.just_finished()
            && let Some(atlas) = &mut sprite.texture_atlas
        {
            if atlas.index >= anim.last {
                if anim.looping {
                    atlas.index = anim.first;
                }
            } else {
                atlas.index += 1;
            }
        }
    }
}

fn draw_connections(mut gizmos: Gizmos, state: Res<GalaxyState>) {
    for &(a, b) in CONNECTIONS {
        let pid_a = (a + 1) as u32;
        let pid_b = (b + 1) as u32;
        if !state.alive.is_empty()
            && (!state.alive.contains(&pid_a) || !state.alive.contains(&pid_b))
        {
            continue;
        }
        gizmos.line_2d(
            planet_position(a),
            planet_position(b),
            Color::srgba(0.4, 0.4, 0.7, 0.4),
        );
    }
}

fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let a = a.to_linear();
    let b = b.to_linear();
    Color::linear_rgba(
        a.red + (b.red - a.red) * t,
        a.green + (b.green - a.green) * t,
        a.blue + (b.blue - a.blue) * t,
        a.alpha + (b.alpha - a.alpha) * t,
    )
}

fn sync_vignette(
    state: Res<GalaxyState>,
    time: Res<Time>,
    mut query: Query<&mut Sprite, With<VignetteTag>>,
) {
    // Very low alpha on purpose, it's supposed to be subtle. Solace's orange
    // turns everything yellow really fast so it's even lower.
    let target = match state.personality {
        Personality::Solace => Color::srgba(1.0, 0.55, 0.05, 0.015),
        Personality::Eclipse => Color::srgba(0.25, 0.0, 0.45, 0.04),
    };
    let speed = time.delta_secs() * 3.0;
    for mut sprite in &mut query {
        sprite.color = lerp_color(sprite.color, target, speed.min(1.0));
    }
}

// --- Background ---

fn drive_drift_bob(time: Res<Time>, mut query: Query<(&mut Transform, &DriftBob)>) {
    let t = time.elapsed_secs();
    for (mut tf, bob) in &mut query {
        tf.translation.x = bob.base.x + (t * bob.speed + bob.phase).sin() * bob.amp.x;
        tf.translation.y = bob.base.y + (t * bob.speed * 0.7 + bob.phase).cos() * bob.amp.y;
        tf.rotation = Quat::from_rotation_z(t * bob.rot_speed);
    }
}

fn sync_nebula_personality(
    state: Res<GalaxyState>,
    time: Res<Time>,
    mut query: Query<(&NebulaTag, &mut Sprite)>,
) {
    let speed = (time.delta_secs() * 0.5).min(1.0);
    for (tag, mut sprite) in &mut query {
        let dominance = match tag.0 {
            Personality::Solace => 1.0 - state.hostility,
            Personality::Eclipse => state.hostility,
        } as f32;
        let target = dominance * 0.35;
        let current = sprite.color.alpha();
        sprite.color.set_alpha(current + (target - current) * speed);
    }
}

fn drive_twinkle(time: Res<Time>, mut query: Query<(&mut Sprite, &Twinkle)>) {
    let t = time.elapsed_secs();
    for (mut sprite, tw) in &mut query {
        let pulse = (t * tw.speed + tw.phase).sin().max(0.0);
        sprite
            .color
            .set_alpha(tw.base_alpha + pulse * tw.pulse_alpha);
    }
}

fn drive_panel_stars(time: Res<Time>, mut query: Query<(&mut Node, &PanelStarBob)>) {
    let t = time.elapsed_secs();
    for (mut node, bob) in &mut query {
        node.left = Val::Px(bob.base.x + (t * bob.speed + bob.phase).sin() * bob.amp.x);
        node.top = Val::Px(bob.base.y + (t * bob.speed * 0.8 + bob.phase).cos() * bob.amp.y);
    }
}

// one every 20-40 seconds
fn spawn_shooting_stars(
    time: Res<Time>,
    mut timer: ResMut<ShootingStarTimer>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
) {
    timer.0.tick(time.delta());
    if !timer.0.just_finished() {
        return;
    }

    let mut rng = rand::rng();
    // the sprite points down-left, flipped it goes down-right
    let flipped = rng.random_bool(0.5);
    let start = Vec2::new(
        rng.random_range(-300.0..500.0),
        rng.random_range(200.0..360.0),
    ) * DISPLAY_SCALE;
    let dir = if flipped {
        Vec2::new(1.0, -1.0)
    } else {
        Vec2::new(-1.0, -1.0)
    };
    let velocity = dir.normalize() * 500.0;

    commands.spawn((
        Sprite {
            image: asset_server.load("shooting_star.png"),
            custom_size: Some(Vec2::new(16.0, 16.0)),
            flip_x: flipped,
            ..default()
        },
        Transform::from_xyz(start.x, start.y, -7.0),
        ShootingStar {
            velocity,
            life: Timer::from_seconds(1.6, TimerMode::Once),
        },
    ));

    let next: f32 = rng.random_range(20.0..40.0);
    timer.0 = Timer::from_seconds(next, TimerMode::Once);
}

fn drive_shooting_stars(
    time: Res<Time>,
    mut commands: Commands,
    mut query: Query<(Entity, &mut Transform, &mut ShootingStar)>,
) {
    for (entity, mut tf, mut star) in &mut query {
        let delta = time.delta();
        tf.translation.x += star.velocity.x * delta.as_secs_f32();
        tf.translation.y += star.velocity.y * delta.as_secs_f32();
        star.life.tick(delta);
        if star.life.just_finished() {
            commands.entity(entity).despawn();
        }
    }
}

fn follow_cursor(
    windows: Query<&Window, With<PrimaryWindow>>,
    mut query: Query<&mut Node, With<CursorTag>>,
) {
    let Ok(window) = windows.single() else { return };
    let Ok(mut node) = query.single_mut() else {
        return;
    };
    if let Some(pos) = window.cursor_position() {
        node.left = Val::Px(pos.x - 8.0);
        node.top = Val::Px(pos.y - 8.0);
    }
}

// --- Input ---
//
// Left click explorer, then a green neighbor planet: move there.
// Left click planet (no explorer selected): target for Sunray/Asteroid.
// Right click explorer/planet: open hologram. Right click empty space or the
// same thing again: close it.
// Space: start/pause logic.

const EXPLORER_HIT_RADIUS: f32 = 28.0 * DISPLAY_SCALE;
const PLANET_HIT_RADIUS: f32 = 55.0 * DISPLAY_SCALE;

fn handle_input(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    planets: Query<(&PlanetTag, &Transform)>,
    explorers: Query<(&ExplorerTag, &Transform)>,
    state: Res<GalaxyState>,
    mut selection: ResMut<Selection>,
    mut inspecting: ResMut<Inspecting>,
    mut logic_state: ResMut<LogicRunState>,
    cmds: Res<UiCommands>,
) {
    if keys.just_pressed(KeyCode::Space) {
        *logic_state = logic_state.toggled();
        let _ = cmds.0.send(UiCommand::ToggleLogic);
    }

    // debug: E pushes to Eclipse, S to Solace
    if keys.just_pressed(KeyCode::KeyE) {
        let _ = cmds.0.send(UiCommand::DebugNudgeHostility(1.0));
    }
    if keys.just_pressed(KeyCode::KeyS) {
        let _ = cmds.0.send(UiCommand::DebugNudgeHostility(-1.0));
    }

    let left = mouse.just_pressed(MouseButton::Left);
    let right = mouse.just_pressed(MouseButton::Right);
    if !left && !right {
        return;
    }

    let Ok(window) = windows.single() else { return };
    let Some(cursor) = window.cursor_position() else {
        return;
    };

    // clicks on the cockpit/buttons are handled by their own systems
    if cursor.x < VIEWPORT_HOLE_MIN.x
        || cursor.x > VIEWPORT_HOLE_MAX.x
        || cursor.y < VIEWPORT_HOLE_MIN.y
        || cursor.y > VIEWPORT_HOLE_MAX.y
    {
        return;
    }

    let Ok((camera, cam_transform)) = cameras.single() else {
        return;
    };
    let Ok(world_pos) = camera.viewport_to_world_2d(cam_transform, cursor) else {
        return;
    };

    if left {
        for (tag, tf) in &explorers {
            let pos = Vec2::new(tf.translation.x, tf.translation.y);
            if pos.distance(world_pos) < EXPLORER_HIT_RADIUS {
                selection.explorer = if selection.explorer == Some(tag.0) {
                    None
                } else {
                    Some(tag.0)
                };
                return;
            }
        }
    }

    if right {
        for (tag, tf) in &explorers {
            let pos = Vec2::new(tf.translation.x, tf.translation.y);
            if pos.distance(world_pos) < EXPLORER_HIT_RADIUS {
                let target = InspectTarget::Explorer(tag.0);
                if inspecting.0 == Some(target) {
                    inspecting.0 = None;
                } else {
                    inspecting.0 = Some(target);
                    let _ = cmds.0.send(UiCommand::InspectExplorer(tag.0));
                }
                return;
            }
        }
    }

    for (tag, tf) in &planets {
        if !state.alive.contains(&tag.0) {
            continue;
        }
        let pos = Vec2::new(tf.translation.x, tf.translation.y);
        if pos.distance(world_pos) >= PLANET_HIT_RADIUS {
            continue;
        }

        if right {
            let target = InspectTarget::Planet(tag.0);
            if inspecting.0 == Some(target) {
                inspecting.0 = None;
            } else {
                inspecting.0 = Some(target);
                let _ = cmds.0.send(UiCommand::InspectPlanet(tag.0));
            }
            return;
        }

        if let Some(explorer_id) = selection.explorer {
            let current = state.explorer_planet.get(&explorer_id).copied();
            if let Some(current) = current
                && state
                    .neighbors
                    .get(&current)
                    .is_some_and(|ns| ns.contains(&tag.0))
            {
                let _ = cmds.0.send(UiCommand::MoveExplorer {
                    explorer_id,
                    dst: tag.0,
                });
                selection.explorer = None;
            }
            return;
        }

        // no explorer selected: just target the planet, the console buttons
        // do the actual firing
        selection.planet = if selection.planet == Some(tag.0) {
            None
        } else {
            Some(tag.0)
        };
        return;
    }

    // right click on nothing closes the hologram
    if right {
        inspecting.0 = None;
    }
}

// The glitch goes on whoever is in control right now, not on "the sunray one"
// or "the asteroid one", because the player is overriding *her*.
fn fire_on_selected_planet(
    selection: &Selection,
    cmds: &UiCommands,
    glitch: &mut GlitchQueue,
    suspicion: &mut SuspicionState,
    tags: &AnimTags,
    queue: &mut PortraitEventQueue,
    active: Personality,
    sunray: bool,
) {
    let Some(planet_id) = selection.planet else {
        return;
    };
    glitch.0.push(active.clone());

    match apply_manual_override(suspicion, &active, sunray) {
        SuspicionOutcome::Allowed { reaction } => {
            if sunray {
                let _ = cmds.0.send(UiCommand::Sunray(planet_id));
            } else {
                let _ = cmds.0.send(UiCommand::Asteroid(planet_id));
            }
            if let Some(tag) = reaction {
                let (first, last) = tags.get(&active, tag);
                queue.0.push_back(PortraitReaction {
                    target: active,
                    first,
                    last,
                    priority: true,
                });
            }
        }
        // nothing is sent to the orchestrator
        SuspicionOutcome::Refused => {
            let (first, last) = tags.get(&active, "refusal");
            queue.0.push_back(PortraitReaction {
                target: active,
                first,
                last,
                priority: true,
            });
        }
    }
}

fn handle_sunray_button(
    interaction_query: Query<&Interaction, (Changed<Interaction>, With<SunrayButtonTag>)>,
    selection: Res<Selection>,
    state: Res<GalaxyState>,
    cmds: Res<UiCommands>,
    mut glitch: ResMut<GlitchQueue>,
    mut suspicion: ResMut<SuspicionState>,
    tags: Res<AnimTags>,
    mut queue: ResMut<PortraitEventQueue>,
) {
    for interaction in &interaction_query {
        if *interaction == Interaction::Pressed {
            fire_on_selected_planet(
                &selection,
                &cmds,
                &mut glitch,
                &mut suspicion,
                &tags,
                &mut queue,
                state.personality.clone(),
                true,
            );
        }
    }
}

fn handle_asteroid_button(
    interaction_query: Query<&Interaction, (Changed<Interaction>, With<AsteroidButtonTag>)>,
    selection: Res<Selection>,
    state: Res<GalaxyState>,
    cmds: Res<UiCommands>,
    mut glitch: ResMut<GlitchQueue>,
    mut suspicion: ResMut<SuspicionState>,
    tags: Res<AnimTags>,
    mut queue: ResMut<PortraitEventQueue>,
) {
    for interaction in &interaction_query {
        if *interaction == Interaction::Pressed {
            fire_on_selected_planet(
                &selection,
                &cmds,
                &mut glitch,
                &mut suspicion,
                &tags,
                &mut queue,
                state.personality.clone(),
                false,
            );
        }
    }
}

fn is_active_transport(button: &TransportButton, state: LogicRunState) -> bool {
    matches!(
        (button, state),
        (TransportButton::Start, LogicRunState::NotStarted)
            | (TransportButton::Pause, LogicRunState::Running)
            | (TransportButton::Resume, LogicRunState::Paused)
    )
}

// Inactive buttons are dimmed instead of hidden, an empty gap in the console
// looked broken.
const TRANSPORT_INACTIVE_TINT: Color = Color::srgba(0.65, 0.65, 0.65, 1.0);

fn sync_transport_buttons(
    mut logic_state: ResMut<LogicRunState>,
    cmds: Res<UiCommands>,
    mut clicked: Query<(&TransportButton, &Interaction), Changed<Interaction>>,
    mut all_query: Query<(&TransportButton, &mut ImageNode)>,
) {
    for (button, interaction) in &mut clicked {
        if is_active_transport(button, *logic_state) && *interaction == Interaction::Pressed {
            *logic_state = logic_state.toggled();
            let _ = cmds.0.send(UiCommand::ToggleLogic);
        }
    }

    for (button, mut image) in &mut all_query {
        image.color = if is_active_transport(button, *logic_state) {
            Color::WHITE
        } else {
            TRANSPORT_INACTIVE_TINT
        };
    }
}

// only one hologram open at a time
fn sync_hologram_visibility(
    inspecting: Res<Inspecting>,
    mut expl_holo: Query<&mut Visibility, (With<ExplorerHologram>, Without<PlanetHologram>)>,
    mut planet_holo: Query<&mut Visibility, (With<PlanetHologram>, Without<ExplorerHologram>)>,
) {
    if !inspecting.is_changed() {
        return;
    }
    let (show_explorer, show_planet) = match inspecting.0 {
        Some(InspectTarget::Explorer(_)) => (true, false),
        Some(InspectTarget::Planet(_)) => (false, true),
        None => (false, false),
    };
    for mut v in &mut expl_holo {
        *v = if show_explorer {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
    for mut v in &mut planet_holo {
        *v = if show_planet {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

fn sync_hologram_content(
    inspection: Res<Inspection>,
    names: Res<PlanetNames>,
    mut expl_text: Query<&mut Text, (With<ExplorerHologramText>, Without<PlanetHologramText>)>,
    mut planet_text: Query<&mut Text, (With<PlanetHologramText>, Without<ExplorerHologramText>)>,
) {
    let Ok(data) = inspection.0.try_lock() else {
        return;
    };
    match &*data {
        InspectionData::None => {}
        InspectionData::ExplorerBag { id, bag_lines } => {
            let mut s = format!("{}\n\nBAG:\n", explorer_display_name(*id));
            for line in bag_lines {
                s.push_str("- ");
                s.push_str(line);
                s.push('\n');
            }
            for mut t in &mut expl_text {
                *t = Text::new(s.clone());
            }
        }
        InspectionData::PlanetInfo {
            id,
            energy_cells,
            charged_cells,
            has_rocket,
        } => {
            let s = format!(
                "{}\n\nEnergy cells: {charged_cells}/{energy_cells} charged\nRocket: {}",
                names.get(*id),
                if *has_rocket { "armed" } else { "none" }
            );
            for mut t in &mut planet_text {
                *t = Text::new(s.clone());
            }
        }
    }
}

fn toggle_map_hologram(
    mut open: ResMut<MapHologramOpen>,
    query: Query<&Interaction, (Changed<Interaction>, With<CompassButtonTag>)>,
) {
    for interaction in &query {
        if *interaction == Interaction::Pressed {
            open.0 = !open.0;
        }
    }
}

fn sync_map_hologram_visibility(
    open: Res<MapHologramOpen>,
    mut panel_q: Query<&mut Visibility, With<MapHologramPanel>>,
) {
    if !open.is_changed() {
        return;
    }
    for mut vis in &mut panel_q {
        *vis = if open.0 {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

fn sync_map_nodes(state: Res<GalaxyState>, mut query: Query<(&MapNodeTag, &mut Visibility)>) {
    if !state.ready {
        return;
    }
    for (tag, mut vis) in &mut query {
        *vis = if state.alive.contains(&tag.0) {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}

// same rule as draw_connections
fn sync_map_edges(state: Res<GalaxyState>, mut query: Query<(&MapEdgeTag, &mut Visibility)>) {
    for (tag, mut vis) in &mut query {
        let (a, b) = CONNECTIONS[tag.0];
        let pid_a = (a + 1) as u32;
        let pid_b = (b + 1) as u32;
        let alive = state.alive.is_empty()
            || (state.alive.contains(&pid_a) && state.alive.contains(&pid_b));
        *vis = if alive {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}

fn update_map_clock_text(
    state: Res<GalaxyState>,
    mut query: Query<(&mut Text, &mut TextColor), With<MapClockText>>,
) {
    let Ok((mut text, mut color)) = query.single_mut() else {
        return;
    };
    let pct = (state.hostility * 100.0).round() as i32;
    let (label, tint) = match state.personality {
        Personality::Solace => ("SOLACE", SOLACE_GOLD),
        Personality::Eclipse => ("ECLIPSE", ECLIPSE_PURPLE),
    };
    **text = format!("{label} ({pct}%) — {}", format_mmss(state.phase_elapsed));
    color.0 = tint;
}

// Only changes color. Moving them on press would need a saved base position
// since they're placed with absolute coords.
fn cockpit_button_hover(
    mut query: Query<
        (&Interaction, &mut ImageNode),
        (
            Changed<Interaction>,
            With<Button>,
            Without<SunrayButtonTag>,
            Without<AsteroidButtonTag>,
        ),
    >,
) {
    for (interaction, mut image) in &mut query {
        image.color = match *interaction {
            Interaction::Hovered => Color::srgb(1.15, 1.15, 1.15),
            Interaction::Pressed => Color::srgb(0.85, 0.85, 0.85),
            Interaction::None => Color::WHITE,
        };
    }
}

// dim Sunray/Asteroid when there's no planet to fire at
fn sync_action_buttons_enabled(
    selection: Res<Selection>,
    mut query: Query<&mut ImageNode, Or<(With<SunrayButtonTag>, With<AsteroidButtonTag>)>>,
) {
    let color = if selection.planet.is_some() {
        Color::WHITE
    } else {
        TRANSPORT_INACTIVE_TINT
    };
    for mut image in &mut query {
        image.color = color;
    }
}

// yellow = where the explorer is, green = where it can go
fn draw_selection_highlight(
    selection: Res<Selection>,
    state: Res<GalaxyState>,
    mut gizmos: Gizmos,
) {
    let Some(explorer_id) = selection.explorer else {
        return;
    };
    let Some(&current) = state.explorer_planet.get(&explorer_id) else {
        return;
    };

    let current_idx = current.saturating_sub(1) as usize;
    gizmos.circle_2d(
        Isometry2d::from_translation(planet_position(current_idx)),
        58.0 * DISPLAY_SCALE,
        Color::srgba(1.0, 1.0, 0.2, 0.9),
    );

    for &neighbor in state.neighbors.get(&current).into_iter().flatten() {
        let idx = neighbor.saturating_sub(1) as usize;
        gizmos.circle_2d(
            Isometry2d::from_translation(planet_position(idx)),
            58.0 * DISPLAY_SCALE,
            Color::srgba(0.2, 1.0, 0.3, 0.9),
        );
    }
}

// --- Screens / text ---

fn update_top_bar_personality(
    state: Res<GalaxyState>,
    mut query: Query<(&mut Text, &mut TextColor), With<TopBarPersonalityText>>,
) {
    let Ok((mut text, mut color)) = query.single_mut() else {
        return;
    };
    let pct = (state.hostility * 100.0).round() as i32;
    match state.personality {
        Personality::Solace => {
            **text = format!("SOLACE ({pct}%)");
            color.0 = SOLACE_GOLD;
        }
        Personality::Eclipse => {
            **text = format!("ECLIPSE ({pct}%)");
            color.0 = ECLIPSE_PURPLE;
        }
    }
}

fn update_top_bar_cycle(
    state: Res<GalaxyState>,
    mut query: Query<&mut Text, With<TopBarCycleText>>,
) {
    let Ok(mut text) = query.single_mut() else {
        return;
    };
    **text = format!("Cycle {}", format_mmss(state.phase_elapsed));
}

fn update_hostility_trend(
    time: Res<Time>,
    state: Res<GalaxyState>,
    mut trend: ResMut<HostilityTrend>,
) {
    if !state.ready {
        return;
    }
    let now = time.elapsed_secs();
    if now - trend.last_sample_secs < HOSTILITY_TREND_SAMPLE_SECS {
        return;
    }
    let delta = state.hostility - trend.last_value;
    trend.direction = if delta > HOSTILITY_TREND_EPSILON {
        HostilityDirection::Rising
    } else if delta < -HOSTILITY_TREND_EPSILON {
        HostilityDirection::Falling
    } else {
        HostilityDirection::Steady
    };
    trend.last_value = state.hostility;
    trend.last_sample_secs = now;
}

fn update_top_bar_trend(
    trend: Res<HostilityTrend>,
    mut query: Query<&mut Text, With<TopBarTrendText>>,
) {
    let Ok(mut text) = query.single_mut() else {
        return;
    };
    **text = match trend.direction {
        HostilityDirection::Rising => "Asteroid odds rising".to_string(),
        HostilityDirection::Falling => "Sunray odds rising".to_string(),
        HostilityDirection::Steady => "Odds holding steady".to_string(),
    };
}

// game ends when all explorers are dead (same check as the logic loop)
fn sync_game_over_overlay(
    state: Res<GalaxyState>,
    mut query: Query<&mut Visibility, With<GameOverOverlay>>,
) {
    let Ok(mut vis) = query.single_mut() else {
        return;
    };
    *vis = if state.ready && state.explorer_planet.is_empty() {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
}

// placeholder until the Chronicle log exists (see ChronicleScreenText)
fn update_chronicle_screen_text(
    state: Res<GalaxyState>,
    selection: Res<Selection>,
    names: Res<PlanetNames>,
    mut query: Query<&mut Text, With<ChronicleScreenText>>,
) {
    let Ok(mut text) = query.single_mut() else {
        return;
    };
    let personality = match state.personality {
        Personality::Solace => "Solace",
        Personality::Eclipse => "Eclipse",
    };
    let planet_sel = selection
        .planet
        .map_or("none".to_string(), |p| names.get(p).to_string());
    let explorer_sel = selection
        .explorer
        .map_or("none".to_string(), |e| explorer_display_name(e).to_string());
    **text = format!(
        "Planets alive: {}/7 | Phase: {personality} ({:.0}%) | Cycle: {}\n\
         Selected planet: {planet_sel} | Selected explorer: {explorer_sel}\n\
         (Cosmic Chronicle coming online...)",
        state.alive.len(),
        state.hostility * 100.0,
        format_mmss(state.phase_elapsed),
    );
}

fn spawn_cockpit_ui(
    commands: &mut Commands,
    asset_server: &AssetServer,
    layouts: &mut Assets<TextureAtlasLayout>,
    tags: &AnimTags,
) {
    let solace_layout = layouts.add(TextureAtlasLayout::from_grid(
        UVec2::new(128, 128),
        37,
        1,
        None,
        None,
    ));
    let eclipse_layout = layouts.add(TextureAtlasLayout::from_grid(
        UVec2::new(128, 128),
        36,
        1,
        None,
        None,
    ));
    // both stance sheets are 4 frames of 128x128
    let stance_layout = layouts.add(TextureAtlasLayout::from_grid(
        UVec2::new(128, 128),
        4,
        1,
        None,
        None,
    ));

    commands
        .spawn(Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            position_type: PositionType::Relative,
            ..default()
        })
        .with_children(|root| {
            root.spawn(cockpit_shell_layer(asset_server, 0, 0)); // frame + walls
            root.spawn(cockpit_shell_layer(asset_server, 1, 0)); // top screen
            root.spawn(cockpit_shell_layer(asset_server, 2, 0)); // bottom console
            root.spawn(cockpit_shell_layer(asset_server, 3, 0)); // chronicle screen
            // Slot (0,1) is skipped on purpose: it has all 6 buttons drawn in
            // it, which shows Start/Pause/Resume all at once.

            // buttons
            root.spawn((
                Button,
                cockpit_button_bundle(asset_server, &BTN_START),
                TransportButton::Start,
            ));
            root.spawn((
                Button,
                cockpit_button_bundle(asset_server, &BTN_PAUSE),
                TransportButton::Pause,
            ));
            root.spawn((
                Button,
                cockpit_button_bundle(asset_server, &BTN_RESUME),
                TransportButton::Resume,
            ));
            root.spawn((
                Button,
                cockpit_button_bundle(asset_server, &BTN_SUNRAY),
                SunrayButtonTag,
            ));
            root.spawn((
                Button,
                cockpit_button_bundle(asset_server, &BTN_ASTEROID),
                AsteroidButtonTag,
            ));
            // TODO: Move button doesn't do anything yet, moving is done by
            // clicking the explorer and then a planet.
            root.spawn((Button, cockpit_button_bundle(asset_server, &BTN_MOVE)));

            // top screen
            // (font sizes aren't multiplied by DISPLAY_SCALE on purpose)
            root.spawn(Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(16.0 * DISPLAY_SCALE),
                width: Val::Px(WINDOW_WIDTH),
                height: Val::Px(40.0 * DISPLAY_SCALE),
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                column_gap: Val::Px(40.0 * DISPLAY_SCALE),
                ..default()
            })
            .with_children(|bar| {
                bar.spawn((
                    Text::new("SOLACE"),
                    TextFont {
                        font: FontSource::Handle(asset_server.load(FONT_REGULAR)),
                        font_size: FontSize::Px(16.0),
                        ..default()
                    },
                    TextColor(SOLACE_GOLD),
                    TopBarPersonalityText,
                ));
                bar.spawn((
                    Text::new("Cycle 00:00"),
                    TextFont {
                        font: FontSource::Handle(asset_server.load(FONT_REGULAR)),
                        font_size: FontSize::Px(15.0),
                        ..default()
                    },
                    TextColor(Color::srgba(0.85, 0.95, 1.0, 0.9)),
                    TopBarCycleText,
                ));
                bar.spawn((
                    Text::new("Odds holding steady"),
                    TextFont {
                        font: FontSource::Handle(asset_server.load(FONT_REGULAR)),
                        font_size: FontSize::Px(14.0),
                        ..default()
                    },
                    TextColor(Color::srgba(0.75, 0.8, 0.9, 0.85)),
                    TopBarTrendText,
                ));
            });

            // chronicle screen: it has angled corners, so the text box is
            // kept inside the part that's fully drawn
            root.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(430.0 * DISPLAY_SCALE),
                    top: Val::Px(760.0 * DISPLAY_SCALE),
                    width: Val::Px(770.0 * DISPLAY_SCALE),
                    height: Val::Px(100.0 * DISPLAY_SCALE),
                    padding: UiRect::all(Val::Px(8.0)),
                    overflow: Overflow::clip(),
                    ..default()
                },
                Text::new(""),
                TextFont {
                    font: FontSource::Handle(asset_server.load(FONT_REGULAR)),
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
                TextColor(Color::srgba(0.75, 0.9, 1.0, 0.85)),
                ChronicleScreenText,
            ));

            // portraits, upper corners
            let solace_idle = tags.get(&Personality::Solace, "idle");
            spawn_portrait_bubble(
                root,
                asset_server,
                Vec2::new(180.0, 150.0) * DISPLAY_SCALE,
                SOLACE_GOLD,
                "solace.png",
                solace_layout,
                SOLACE_INNER_SIZE,
                PortraitConfig::new(solace_idle.0, solace_idle.1, 10.0, 10.0 / 3.0),
                Personality::Solace,
                "SOLACE",
            );
            let eclipse_idle = tags.get(&Personality::Eclipse, "idle");
            spawn_portrait_bubble(
                root,
                asset_server,
                Vec2::new(1292.0, 150.0) * DISPLAY_SCALE,
                ECLIPSE_PURPLE,
                "eclipse.png",
                eclipse_layout,
                ECLIPSE_INNER_SIZE,
                PortraitConfig::new(eclipse_idle.0, eclipse_idle.1, 4.0, 4.0),
                Personality::Eclipse,
                "ECLIPSE",
            );

            // Stances stand right next to the ring, watching over the galaxy.
            // Position math is in stance_ui_top_left.
            for (image, personality) in [
                ("solace_stance.png", Personality::Solace),
                ("eclipse_stance.png", Personality::Eclipse),
            ] {
                let top_left = stance_ui_top_left(&personality);
                let stance_range = tags.stance(&personality, "stance");
                root.spawn((
                    ImageNode {
                        image: asset_server.load(image),
                        texture_atlas: Some(TextureAtlas {
                            layout: stance_layout.clone(),
                            index: stance_range.0,
                        }),
                        ..default()
                    },
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(top_left.x),
                        top: Val::Px(top_left.y),
                        width: Val::Px(STANCE_SIZE),
                        height: Val::Px(STANCE_SIZE),
                        ..default()
                    },
                    StanceTag(personality),
                    StanceAnim::new(stance_range),
                ));
            }

            // holograms (hidden until right-click)
            let holo_left_w =
                (HOLOGRAM_LEFT.local.max.x - HOLOGRAM_LEFT.local.min.x) * DISPLAY_SCALE;
            root.spawn((
                cockpit_button_bundle(asset_server, &HOLOGRAM_LEFT),
                Visibility::Hidden,
                ExplorerHologram,
            ))
            .with_children(|h| {
                h.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(30.0),
                        top: Val::Px(34.0),
                        width: Val::Px(holo_left_w - 60.0),
                        ..default()
                    },
                    Text::new(""),
                    TextFont {
                        font: FontSource::Handle(asset_server.load(FONT_REGULAR)),
                        font_size: FontSize::Px(13.0),
                        ..default()
                    },
                    TextColor(Color::srgba(0.6, 0.9, 1.0, 0.95)),
                    ExplorerHologramText,
                ));
            });

            let holo_right_w =
                (HOLOGRAM_RIGHT.local.max.x - HOLOGRAM_RIGHT.local.min.x) * DISPLAY_SCALE;
            root.spawn((
                cockpit_button_bundle(asset_server, &HOLOGRAM_RIGHT),
                Visibility::Hidden,
                PlanetHologram,
            ))
            .with_children(|h| {
                h.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(30.0),
                        top: Val::Px(34.0),
                        width: Val::Px(holo_right_w - 60.0),
                        ..default()
                    },
                    Text::new(""),
                    TextFont {
                        font: FontSource::Handle(asset_server.load(FONT_REGULAR)),
                        font_size: FontSize::Px(13.0),
                        ..default()
                    },
                    TextColor(Color::srgba(1.0, 0.85, 0.6, 0.95)),
                    PlanetHologramText,
                ));
            });

            spawn_map_hologram(root, asset_server);

            // suspicion meters, next to the portraits on the side facing the
            // galaxy
            let suspicion_meter_top = 150.0 * DISPLAY_SCALE;
            spawn_suspicion_meter(
                root,
                asset_server,
                Vec2::new(
                    180.0 * DISPLAY_SCALE + PORTRAIT_BUBBLE_SIZE + 30.0 * DISPLAY_SCALE,
                    suspicion_meter_top,
                ),
                SOLACE_GOLD,
                Personality::Solace,
            );
            spawn_suspicion_meter(
                root,
                asset_server,
                Vec2::new(
                    1292.0 * DISPLAY_SCALE - SUSPICION_METER_W - 30.0 * DISPLAY_SCALE,
                    suspicion_meter_top,
                ),
                ECLIPSE_PURPLE,
                Personality::Eclipse,
            );

            // game over, spawned last so it's on top of everything
            root.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    flex_direction: FlexDirection::Column,
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    row_gap: Val::Px(12.0),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
                Visibility::Hidden,
                GameOverOverlay,
            ))
            .with_children(|overlay| {
                overlay.spawn((
                    Text::new("ALL EXPLORERS HAVE DIED"),
                    TextFont {
                        font: FontSource::Handle(asset_server.load(FONT_REGULAR)),
                        font_size: FontSize::Px(42.0),
                        ..default()
                    },
                    TextColor(Color::srgb(1.0, 0.35, 0.3)),
                ));
                overlay.spawn((
                    Text::new("GAME OVER"),
                    TextFont {
                        font: FontSource::Handle(asset_server.load(FONT_REGULAR)),
                        font_size: FontSize::Px(24.0),
                        ..default()
                    },
                    TextColor(Color::srgba(0.9, 0.9, 0.9, 0.9)),
                ));
            });
        });
}

// frame + portrait + glitch overlay + name
#[allow(clippy::too_many_arguments)]
fn spawn_portrait_bubble(
    root: &mut ChildSpawnerCommands,
    asset_server: &AssetServer,
    top_left: Vec2,
    tint: Color,
    portrait_image: &'static str,
    layout: Handle<TextureAtlasLayout>,
    inner_size: f32,
    portrait_config: PortraitConfig,
    personality: Personality,
    label: &str,
) {
    root.spawn(Node {
        position_type: PositionType::Absolute,
        left: Val::Px(top_left.x),
        top: Val::Px(top_left.y),
        width: Val::Px(PORTRAIT_BUBBLE_SIZE),
        height: Val::Px(PORTRAIT_BUBBLE_SIZE + 20.0),
        flex_direction: FlexDirection::Column,
        align_items: AlignItems::Center,
        row_gap: Val::Px(2.0),
        ..default()
    })
    .with_children(|col| {
        col.spawn(Node {
            width: Val::Px(PORTRAIT_BUBBLE_SIZE),
            height: Val::Px(PORTRAIT_BUBBLE_SIZE),
            position_type: PositionType::Relative,
            ..default()
        })
        .with_children(|bubble| {
            bubble.spawn((
                ImageNode {
                    image: asset_server.load("portrait_bubble_frame.png"),
                    color: tint,
                    ..default()
                },
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Px(PORTRAIT_BUBBLE_SIZE),
                    height: Val::Px(PORTRAIT_BUBBLE_SIZE),
                    ..default()
                },
            ));
            bubble.spawn((
                ImageNode {
                    image: asset_server.load(portrait_image),
                    texture_atlas: Some(TextureAtlas { layout, index: 0 }),
                    ..default()
                },
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Px(inner_size),
                    height: Val::Px(inner_size),
                    top: Val::Px((PORTRAIT_BUBBLE_SIZE - inner_size) / 2.0),
                    left: Val::Px((PORTRAIT_BUBBLE_SIZE - inner_size) / 2.0),
                    ..default()
                },
                portrait_config,
                PortraitTag(personality.clone()),
            ));
            bubble.spawn((
                ImageNode {
                    image: asset_server.load("fx_parasite_glitch-sheet.png"),
                    color: Color::srgba(1.0, 1.0, 1.0, 0.0),
                    ..default()
                },
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Px(PORTRAIT_BUBBLE_SIZE),
                    height: Val::Px(PORTRAIT_BUBBLE_SIZE * 48.0 / 192.0),
                    top: Val::Px(
                        (PORTRAIT_BUBBLE_SIZE - PORTRAIT_BUBBLE_SIZE * 48.0 / 192.0) / 2.0,
                    ),
                    ..default()
                },
                PortraitTag(personality),
                GlitchOverlay::default(),
            ));
        });
        col.spawn((
            Text::new(label),
            TextFont {
                font: FontSource::Handle(asset_server.load(FONT_REGULAR)),
                font_size: FontSize::Px(13.0),
                ..default()
            },
            TextColor(tint),
        ));
    });
}

// the fill is anchored at the bottom and grows up
fn spawn_suspicion_meter(
    root: &mut ChildSpawnerCommands,
    asset_server: &AssetServer,
    top_left: Vec2,
    tint: Color,
    personality: Personality,
) {
    root.spawn(Node {
        position_type: PositionType::Absolute,
        left: Val::Px(top_left.x),
        top: Val::Px(top_left.y),
        width: Val::Px(SUSPICION_METER_W),
        height: Val::Px(SUSPICION_METER_H),
        ..default()
    })
    .with_children(|meter| {
        meter.spawn((
            ImageNode {
                image: asset_server.load("ui_meter_suspicion.png"),
                ..default()
            },
            Node {
                position_type: PositionType::Absolute,
                width: Val::Px(SUSPICION_METER_W),
                height: Val::Px(SUSPICION_METER_H),
                ..default()
            },
        ));
        meter.spawn((
            BackgroundColor(tint),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(SUSPICION_FILL_INSET),
                right: Val::Px(SUSPICION_FILL_INSET),
                bottom: Val::Px(SUSPICION_FILL_INSET),
                height: Val::Px(0.0),
                ..default()
            },
            SuspicionMeterFill(personality),
        ));
    });
}

fn spawn_map_hologram(root: &mut ChildSpawnerCommands, asset_server: &AssetServer) {
    root.spawn((
        Button,
        ImageNode {
            image: asset_server.load("ui_icon_compass.png"),
            ..default()
        },
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(WINDOW_WIDTH - 56.0 * DISPLAY_SCALE),
            top: Val::Px(16.0 * DISPLAY_SCALE),
            width: Val::Px(40.0 * DISPLAY_SCALE),
            height: Val::Px(40.0 * DISPLAY_SCALE),
            ..default()
        },
        CompassButtonTag,
    ));

    root.spawn((
        ImageNode {
            image: asset_server.load("ui_panel_map.png"),
            ..default()
        },
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px((WINDOW_WIDTH - MAP_PANEL_W) / 2.0),
            top: Val::Px((WINDOW_HEIGHT - MAP_PANEL_H) / 2.0),
            width: Val::Px(MAP_PANEL_W),
            height: Val::Px(MAP_PANEL_H),
            ..default()
        },
        Visibility::Hidden,
        MapHologramPanel,
    ))
    .with_children(|panel| {
        panel.spawn((
            ImageNode {
                image: asset_server.load("ui_clock_phase.png"),
                ..default()
            },
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(16.0 * DISPLAY_SCALE),
                top: Val::Px(28.0 * DISPLAY_SCALE),
                width: Val::Px(24.0 * DISPLAY_SCALE),
                height: Val::Px(24.0 * DISPLAY_SCALE),
                ..default()
            },
        ));
        panel.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(48.0 * DISPLAY_SCALE),
                top: Val::Px(30.0 * DISPLAY_SCALE),
                ..default()
            },
            Text::new(""),
            TextFont {
                font: FontSource::Handle(asset_server.load(FONT_REGULAR)),
                font_size: FontSize::Px(14.0),
                ..default()
            },
            TextColor(Color::srgba(0.75, 0.9, 1.0, 0.9)),
            MapClockText,
        ));

        // lines first so the nodes draw on top
        for (i, &(a, b)) in CONNECTIONS.iter().enumerate() {
            let pa = map_node_local_pos(a);
            let pb = map_node_local_pos(b);
            let mid = (pa + pb) / 2.0;
            let delta = pb - pa;
            let length = delta.length();
            let angle = delta.y.atan2(delta.x);
            panel.spawn((
                ImageNode {
                    image: asset_server.load("map_line_dash.png"),
                    ..default()
                },
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(mid.x - length / 2.0),
                    top: Val::Px(mid.y - MAP_EDGE_THICKNESS / 2.0),
                    width: Val::Px(length),
                    height: Val::Px(MAP_EDGE_THICKNESS),
                    ..default()
                },
                UiTransform::from_rotation(Rot2::radians(angle)),
                MapEdgeTag(i),
            ));
        }

        for planet_idx in 0..7usize {
            let pos = map_node_local_pos(planet_idx);
            panel.spawn((
                ImageNode {
                    image: asset_server.load("map_node_known.png"),
                    ..default()
                },
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(pos.x - MAP_NODE_SIZE / 2.0),
                    top: Val::Px(pos.y - MAP_NODE_SIZE / 2.0),
                    width: Val::Px(MAP_NODE_SIZE),
                    height: Val::Px(MAP_NODE_SIZE),
                    ..default()
                },
                MapNodeTag((planet_idx + 1) as u32),
            ));
        }
    });
}

// --- Setup ---

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    tags: Res<AnimTags>,
    mut layouts: ResMut<Assets<TextureAtlasLayout>>,
    mut images: ResMut<Assets<Image>>,
    mut primary_cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mut ready: ResMut<ReadySignal>,
) {
    commands.spawn(Camera2d);

    commands.spawn((
        Sprite {
            image: asset_server.load("space_bg.png"),
            custom_size: Some(Vec2::new(WINDOW_WIDTH, WINDOW_HEIGHT)),
            ..default()
        },
        Transform::from_xyz(0.0, 0.0, -10.0),
    ));

    // tint over the whole screen (a stretched white pixel), see sync_vignette
    let white_px = images.add(Image::new_fill(
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[255, 255, 255, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    ));
    commands.spawn((
        Sprite {
            image: white_px,
            custom_size: Some(Vec2::new(WINDOW_WIDTH, WINDOW_HEIGHT)),
            color: Color::srgba(1.0, 0.55, 0.05, 0.05),
            ..default()
        },
        Transform::from_xyz(0.0, 0.0, 4.8),
        VignetteTag,
    ));

    // fixed pseudo-random positions so the sky looks the same every run
    let star_tex = asset_server.load("star.png");
    for i in 0..150_u32 {
        let x = ((i.wrapping_mul(7919).wrapping_add(13)) % WINDOW_WIDTH as u32) as f32
            - WINDOW_WIDTH / 2.0;
        let y = ((i.wrapping_mul(6271).wrapping_add(7)) % WINDOW_HEIGHT as u32) as f32
            - WINDOW_HEIGHT / 2.0;
        commands.spawn((
            Sprite {
                image: star_tex.clone(),
                custom_size: Some(Vec2::new(2.0, 2.0)),
                ..default()
            },
            Transform::from_xyz(x, y, -9.0),
        ));
    }

    // orange nebula = Solace, purple = Eclipse, crossfaded by
    // sync_nebula_personality
    for (file, personality, base) in [
        (
            "orange_nebula.png",
            Personality::Solace,
            Vec2::new(-150.0, 60.0) * DISPLAY_SCALE,
        ),
        (
            "purple_nebula.png",
            Personality::Eclipse,
            Vec2::new(150.0, -40.0) * DISPLAY_SCALE,
        ),
    ] {
        let phase = if personality == Personality::Solace {
            0.0
        } else {
            2.4
        };
        commands.spawn((
            Sprite {
                image: asset_server.load(file),
                color: Color::srgba(1.0, 1.0, 1.0, 0.0),
                custom_size: Some(Vec2::new(400.0, 300.0) * DISPLAY_SCALE),
                ..default()
            },
            Transform::from_xyz(base.x, base.y, -8.0),
            NebulaTag(personality),
            DriftBob {
                base,
                amp: Vec2::new(18.0, 12.0) * DISPLAY_SCALE,
                speed: 0.03,
                phase,
                rot_speed: 0.01,
            },
        ));
    }

    // pastel and pink nebulas are just decoration, always faintly visible
    let pastel_base = Vec2::new(0.0, 120.0) * DISPLAY_SCALE;
    commands.spawn((
        Sprite {
            image: asset_server.load("pastel_nebula.png"),
            color: Color::srgba(1.0, 1.0, 1.0, 0.12),
            custom_size: Some(Vec2::new(400.0, 300.0) * DISPLAY_SCALE),
            ..default()
        },
        Transform::from_xyz(pastel_base.x, pastel_base.y, -8.5),
        DriftBob {
            base: pastel_base,
            amp: Vec2::new(14.0, 10.0) * DISPLAY_SCALE,
            speed: 0.02,
            phase: 5.1,
            rot_speed: -0.008,
        },
    ));

    let pink_base = Vec2::new(-260.0, -140.0) * DISPLAY_SCALE;
    commands.spawn((
        Sprite {
            image: asset_server.load("pink_nebula.png"),
            color: Color::srgba(1.0, 1.0, 1.0, 0.12),
            custom_size: Some(Vec2::new(400.0, 300.0) * DISPLAY_SCALE),
            ..default()
        },
        Transform::from_xyz(pink_base.x, pink_base.y, -8.3),
        DriftBob {
            base: pink_base,
            amp: Vec2::new(12.0, 16.0) * DISPLAY_SCALE,
            speed: 0.025,
            phase: 1.7,
            rot_speed: 0.007,
        },
    ));

    // a few bigger twinkling stars
    let big_star_tex = asset_server.load("big_star.png");
    for i in 0..12_u32 {
        let x = ((i.wrapping_mul(5237).wrapping_add(101)) % WINDOW_WIDTH as u32) as f32
            - WINDOW_WIDTH / 2.0;
        let y = ((i.wrapping_mul(4111).wrapping_add(53)) % WINDOW_HEIGHT as u32) as f32
            - WINDOW_HEIGHT / 2.0;
        commands.spawn((
            Sprite {
                image: big_star_tex.clone(),
                custom_size: Some(Vec2::new(7.0, 7.0)),
                color: Color::srgba(1.0, 1.0, 1.0, 0.3),
                ..default()
            },
            Transform::from_xyz(x, y, -9.0),
            Twinkle {
                speed: 0.5 + (i as f32 * 0.13) % 1.0,
                phase: i as f32 * 1.7,
                base_alpha: 0.15,
                pulse_alpha: 0.7,
            },
        ));
    }

    if let Ok(mut cursor_options) = primary_cursor.single_mut() {
        cursor_options.visible = false;
    }
    commands.spawn((
        ImageNode {
            image: asset_server.load("cursor_star.png"),
            texture_atlas: Some(TextureAtlas {
                layout: layouts.add(TextureAtlasLayout::from_grid(
                    UVec2::new(16, 16),
                    2,
                    1,
                    None,
                    None,
                )),
                index: 0,
            }),
            ..default()
        },
        Node {
            position_type: PositionType::Absolute,
            width: Val::Px(16.0),
            height: Val::Px(16.0),
            ..default()
        },
        CursorTag,
    ));

    let planet_layout = layouts.add(TextureAtlasLayout::from_grid(
        UVec2::new(48, 48),
        28,
        1,
        None,
        None,
    ));
    let planet_files = [
        "planet_orbitron.png",
        "planet_skycartel.png",
        "planet_rustrelli.png",
        "planet_compiler.png",
        "planet_crabtorio.png",
        "planet_houston.png",
        "planet_enterprise.png",
    ];
    let planet_idle = tags.planet("idle");
    for (i, file) in planet_files.iter().enumerate() {
        let planet_id = (i + 1) as u32;
        let pos = planet_position(i);
        commands.spawn((
            Sprite {
                image: asset_server.load(*file),
                texture_atlas: Some(TextureAtlas {
                    layout: planet_layout.clone(),
                    index: planet_idle.0,
                }),
                custom_size: Some(Vec2::new(96.0, 96.0) * DISPLAY_SCALE),
                ..default()
            },
            Transform::from_xyz(pos.x, pos.y, 0.0),
            AnimationConfig::new(planet_idle.0, planet_idle.1, 10.0, true),
            PlanetTag(planet_id),
            PlanetState::Alive,
        ));
    }

    // Jeb is id 2 and starts on planet 1, Viviana is id 1 and starts on
    // planet 4 (same as builder.rs)
    let jeb_layout = layouts.add(TextureAtlasLayout::from_grid(
        UVec2::new(48, 48),
        20,
        1,
        None,
        None,
    ));
    let jeb_start = explorer_offset_pos(1, 2);
    commands.spawn((
        Sprite {
            image: asset_server.load("jeb.png"),
            texture_atlas: Some(TextureAtlas {
                layout: jeb_layout,
                index: 0,
            }),
            custom_size: Some(Vec2::new(72.0, 72.0) * DISPLAY_SCALE),
            ..default()
        },
        Transform::from_xyz(jeb_start.x, jeb_start.y, 2.0),
        ExplorerTag(2),
        ExplorerAnim::new_jeb(1, &tags),
    ));
    spawn_resource_badges(&mut commands, 2);

    let viv_layout = layouts.add(TextureAtlasLayout::from_grid(
        UVec2::new(48, 48),
        24,
        1,
        None,
        None,
    ));
    let viv_start = explorer_offset_pos(4, 1);
    commands.spawn((
        Sprite {
            image: asset_server.load("viviana.png"),
            texture_atlas: Some(TextureAtlas {
                layout: viv_layout,
                index: 0,
            }),
            custom_size: Some(Vec2::new(72.0, 72.0) * DISPLAY_SCALE),
            ..default()
        },
        Transform::from_xyz(viv_start.x, viv_start.y, 2.0),
        ExplorerTag(1),
        ExplorerAnim::new_viviana(4, &tags),
    ));
    spawn_resource_badges(&mut commands, 1);

    commands.insert_resource(ResourceIcons::load(&asset_server));
    commands.insert_resource(ProjectileAssets::load(&asset_server, &mut layouts));

    spawn_cockpit_ui(&mut commands, &asset_server, &mut layouts, &tags);

    if let Some(tx) = ready.0.take() {
        let _ = tx.send(());
    }
}
