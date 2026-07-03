use bevy::prelude::*;
use bevy::asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::window::{CursorOptions, PrimaryWindow, WindowResolution};
use bipolar_shared::{GalaxyEvent, GalaxySnapshot, Personality, ResourceKind};
use crossbeam_channel::{Receiver as CmdReceiver, Sender as CmdSender};
use rand::RngExt;
use std::collections::{HashMap, HashSet, VecDeque};
use std::f32::consts::{FRAC_PI_2, PI};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ── User-triggered orchestrator commands ───────────────────────────────────────
//
// Bevy systems never call `OrchestratorApi` directly (its methods block on
// channel acks with multi-second timeouts, which would freeze rendering).
// Instead input systems push a `UiCommand` here; the bridge thread drains it
// once per tick right alongside the autonomous logic loop, exactly like the
// CLI's `interactive_loop` shares the same channels with a running logic loop.
enum UiCommand {
    Sunray(u32),
    Asteroid(u32),
    MoveExplorer { explorer_id: u32, dst: u32 },
    ToggleLogic,
    /// Fetches an explorer's bag content on demand (right-click) — not part of
    /// the regular snapshot since it requires a channel round-trip.
    InspectExplorer(u32),
    /// Fetches a planet's energy cell / rocket state on demand (right-click).
    InspectPlanet(u32),
    /// Debug-only: forces hostility toward Eclipse (E key) or Solace (S key)
    /// instantly, to verify the flip without waiting through real playtime.
    DebugNudgeHostility(f64),
}

#[derive(Resource, Clone)]
struct UiCommands(CmdSender<UiCommand>);

/// The explorer currently selected for a manual move (click explorer, then a
/// neighboring planet to confirm).
#[derive(Resource, Default)]
struct Selection {
    explorer: Option<u32>,
    /// The planet selected via clicking, fired on by the Sunray/Asteroid
    /// cockpit buttons rather than immediately on click.
    planet: Option<u32>,
}

/// Client-side mirror of the logic loop's state, for the button label.
/// Updated optimistically when the user presses Space or clicks the button.
/// `NotStarted` and `Paused` both mean "toggling sends the same command that
/// calls `start_logic()`" on the bridge thread — the distinction only matters
/// for what the button should say.
#[derive(Resource, Clone, Copy, PartialEq)]
enum LogicRunState {
    NotStarted,
    Running,
    Paused,
}

impl LogicRunState {
    /// The state after toggling (pressing Start/Pause/Resume).
    fn toggled(self) -> Self {
        match self {
            LogicRunState::NotStarted | LogicRunState::Paused => LogicRunState::Running,
            LogicRunState::Running => LogicRunState::Paused,
        }
    }
}

/// One-shot signal so the bridge thread waits for the galaxy scene to exist
/// before spawning planets and starting the autonomous logic loop. Without
/// this, `build_api`/`start_logic` ran the instant the process launched, on a
/// plain OS thread racing Bevy's own multi-second window/GPU-adapter init —
/// planets could take (and lose) hits before the player ever saw the window.
#[derive(Resource)]
struct ReadySignal(Option<CmdSender<()>>);

// ── Cockpit UI: layout constants, colors, marker components ───────────────────
//
// The whole cockpit shell (window frame, top/bottom screens, buttons,
// holograms) comes from one artwork file, `cockpit.png` (a 4-wide packed
// sheet, 1600x900 per slot — see `cockpit.json`, exported with Aseprite's
// "Split Layers"). Every rectangle below was measured directly from that
// file's alpha channel / per-layer bounding boxes, not estimated — see
// `COCKPIT_*` below. If the art changes, these need re-measuring.

const SOLACE_GOLD: Color = Color::srgb(1.0, 0.85, 0.35);
const ECLIPSE_PURPLE: Color = Color::srgb(0.65, 0.45, 0.95);

/// Pixeloid font family — regular weight, used for all cockpit UI text.
/// `PixeloidSans-Bold.ttf` and `PixeloidMono.ttf` are also delivered in
/// `assets/fonts/Pixeloid_Font_1_0/` for future emphasis/mono use, not yet wired.
const FONT_REGULAR: &str = "fonts/Pixeloid_Font_1_0/PixeloidSans.ttf";

/// All measurements below (`cockpit.png` layer positions, ring layout,
/// viewport bounds, etc.) were taken at a 1600x900 reference design. The
/// actual window is displayed at `DISPLAY_SCALE` of that. Vale's presentation
/// laptop is confirmed 1920x1080 — exactly 16:9, the same aspect ratio as the
/// 1600x900 reference — so `1920/1600 = 1080/900 = 1.2` fills the screen
/// edge-to-edge in true fullscreen with zero letterboxing. This is a fixed,
/// known scale for that specific screen, not a dynamically-computed one; if
/// this ever needs to run well on a different-resolution monitor, this
/// (and/or the window mode below) needs revisiting.
///
/// Important: `DISPLAY_SCALE` only applies to *destination* (on-screen)
/// positions/sizes. `cockpit.png` itself is NOT re-cropped — `cockpit_slot_rect`
/// / `cockpit_full_slot` stay in the original 1600x900-per-slot source-image
/// space, since that's real pixel data in the actual file, not a design unit.
const DISPLAY_SCALE: f32 = 1.2;
const WINDOW_WIDTH: f32 = 1600.0 * DISPLAY_SCALE;
const WINDOW_HEIGHT: f32 = 900.0 * DISPLAY_SCALE;

/// Bounding box of the transparent viewport hole in `cockpit.png`, measured
/// from its alpha channel (not a perfect rectangle — the window is angled at
/// the top corners — but this bounding box is used as a permissive click-gate
/// so galaxy clicks don't also land on cockpit chrome). Scaled to display space.
const VIEWPORT_HOLE_MIN: Vec2 = Vec2::new(110.0 * DISPLAY_SCALE, 97.0 * DISPLAY_SCALE);
const VIEWPORT_HOLE_MAX: Vec2 = Vec2::new(1494.0 * DISPLAY_SCALE, 646.0 * DISPLAY_SCALE);

/// One slot in the 4-wide, 1600x900-per-slot packed cockpit sheet.
fn cockpit_slot_rect(col: u32, row: u32, local: Rect) -> Rect {
    let origin = Vec2::new(col as f32 * 1600.0, row as f32 * 900.0);
    Rect::new(origin.x + local.min.x, origin.y + local.min.y, origin.x + local.max.x, origin.y + local.max.y)
}

fn cockpit_full_slot(col: u32, row: u32) -> Rect {
    cockpit_slot_rect(col, row, Rect::new(0.0, 0.0, 1600.0, 900.0))
}

/// A full-canvas cockpit shell layer (window / top screen / bottom screen /
/// chronicle screen backdrop) stacked at (0,0), 1600x900 — each slot is
/// transparent except that one layer's own art, so stacking several
/// recreates the full composited look.
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

/// One clickable cockpit button, cropped tightly to its own art (not the
/// full slot) and positioned at its exact on-screen pixel rectangle — layer
/// content coordinates equal on-screen coordinates since every layer is
/// canvas-aligned.
struct CockpitButtonSpec {
    col: u32,
    row: u32,
    local: Rect,
}

const BTN_START: CockpitButtonSpec = CockpitButtonSpec { col: 1, row: 1, local: Rect { min: Vec2::new(491.0, 650.0), max: Vec2::new(590.0, 733.0) } };
const BTN_PAUSE: CockpitButtonSpec = CockpitButtonSpec { col: 2, row: 1, local: Rect { min: Vec2::new(586.0, 650.0), max: Vec2::new(697.0, 733.0) } };
const BTN_RESUME: CockpitButtonSpec = CockpitButtonSpec { col: 3, row: 1, local: Rect { min: Vec2::new(688.0, 650.0), max: Vec2::new(805.0, 733.0) } };
const BTN_SUNRAY: CockpitButtonSpec = CockpitButtonSpec { col: 0, row: 2, local: Rect { min: Vec2::new(792.0, 650.0), max: Vec2::new(904.0, 733.0) } };
const BTN_ASTEROID: CockpitButtonSpec = CockpitButtonSpec { col: 1, row: 2, local: Rect { min: Vec2::new(896.0, 650.0), max: Vec2::new(1006.0, 733.0) } };
const BTN_MOVE: CockpitButtonSpec = CockpitButtonSpec { col: 2, row: 2, local: Rect { min: Vec2::new(1001.0, 650.0), max: Vec2::new(1102.0, 733.0) } };
const HOLOGRAM_LEFT: CockpitButtonSpec = CockpitButtonSpec { col: 3, row: 2, local: Rect { min: Vec2::new(98.0, 342.0), max: Vec2::new(353.0, 739.0) } };
const HOLOGRAM_RIGHT: CockpitButtonSpec = CockpitButtonSpec { col: 0, row: 3, local: Rect { min: Vec2::new(1248.0, 368.0), max: Vec2::new(1495.0, 743.0) } };

fn cockpit_button_bundle(asset_server: &AssetServer, spec: &CockpitButtonSpec) -> impl Bundle {
    // Source crop stays in the original 1600x900-per-slot image space;
    // only the on-screen (destination) position/size is scaled down.
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

/// Portrait bubble frame size and the (smaller, centered) portrait sizes inside
/// it. Solace's art sits slightly bigger in its frame than Eclipse's at equal
/// Node size, hence the smaller inner size here to visually match.
const PORTRAIT_BUBBLE_SIZE: f32 = 128.0 * DISPLAY_SCALE;
const SOLACE_INNER_SIZE: f32 = 84.0 * DISPLAY_SCALE;
const ECLIPSE_INNER_SIZE: f32 = 96.0 * DISPLAY_SCALE;

/// The stance sprite is a standing full-body figure for whichever personality
/// currently controls the shared body. Unlike the portrait bubbles (a
/// close-up on her current mood, tucked in the corner), the stance is how
/// she's actually present in front of the galaxy — a large watching figure
/// flanking the viewport, not a UI readout. Sized well above the portrait
/// bubble but capped short of "overpowering": about a third of the
/// viewport's height.
const STANCE_SIZE: f32 = 240.0 * DISPLAY_SCALE;

#[derive(Component)]
struct TopBarPersonalityText;

#[derive(Component)]
struct TopBarCycleText;

/// Tags each of the Start/Pause/Resume cockpit buttons so
/// `sync_transport_buttons` can show only the one matching `LogicRunState`.
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

/// Placeholder text inside the chronicle screen area, until the real Cosmic
/// Chronicle log is built. Shows the same overview stats the old "Galaxy
/// Overview" panel had, so that data isn't lost in this pass.
#[derive(Component)]
struct ChronicleScreenText;

// ── Entity inspection (holograms) ───────────────────────────────────────────
//
// Right-click a planet or explorer to open its hologram. `Inspecting` is the
// client-side toggle (which hologram should be visible, and for which id);
// `Inspection` is the async result of the on-demand data fetch the bridge
// thread performs (bag content / planet state both require a channel
// round-trip, so they're fetched once on click rather than every snapshot).

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
    ExplorerBag { id: u32, bag_lines: Vec<String> },
    PlanetInfo { id: u32, energy_cells: usize, charged_cells: usize, has_rocket: bool },
}

#[derive(Resource, Clone)]
struct Inspection(Arc<Mutex<InspectionData>>);

/// Marks the left hologram panel (shows explorer bag contents).
#[derive(Component)]
struct ExplorerHologram;

/// Marks the right hologram panel (shows planet state).
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

// ── Map hologram ────────────────────────────────────────────────────────────
//
// A toggleable overview panel (compass button opens/closes it) showing the
// real galaxy topology in miniature: one node per alive planet, dashed
// connection art between real neighbor pairs (same adjacency `draw_connections`
// already draws in the main view), plus the current phase/personality —
// all read from the same `GalaxyState` the rest of the GUI uses, nothing new
// simulated just for this panel.

/// Toggled by the compass button; drives the panel's visibility.
#[derive(Resource, Default)]
struct MapHologramOpen(bool);

#[derive(Component)]
struct CompassButtonTag;

#[derive(Component)]
struct MapHologramPanel;

#[derive(Component)]
struct MapNodeTag(u32);

/// Index into `CONNECTIONS` — both ends' planet ids are derived from it when
/// deciding whether this edge is still visible.
#[derive(Component)]
struct MapEdgeTag(usize);

#[derive(Component)]
struct MapClockText;

const MAP_PANEL_W: f32 = 480.0 * DISPLAY_SCALE;
const MAP_PANEL_H: f32 = 384.0 * DISPLAY_SCALE;
const MAP_RING_RADIUS_X: f32 = 150.0 * DISPLAY_SCALE;
const MAP_RING_RADIUS_Y: f32 = 100.0 * DISPLAY_SCALE;
/// Local (top-left-origin, y-down) center of the mini ring within the panel —
/// offset down from the panel's geometric center to leave room for the
/// clock/phase readout at the top.
const MAP_RING_CENTER: Vec2 = Vec2::new(MAP_PANEL_W / 2.0, MAP_PANEL_H / 2.0 + 30.0 * DISPLAY_SCALE);
const MAP_NODE_SIZE: f32 = 28.0 * DISPLAY_SCALE;
const MAP_EDGE_THICKNESS: f32 = 6.0 * DISPLAY_SCALE;

/// Same elliptical-ring layout as `planet_position`, but in UI-local
/// (y-down) space sized to fit inside the map panel.
fn map_node_local_pos(index: usize) -> Vec2 {
    let angle = FRAC_PI_2 - index as f32 * 2.0 * PI / 7.0;
    MAP_RING_CENTER + Vec2::new(MAP_RING_RADIUS_X * angle.cos(), -MAP_RING_RADIUS_Y * angle.sin())
}

// ── Embedded galaxy config ────────────────────────────────────────────────────

const GALAXY_SRC: &str = include_str!("../../galaxy.txt");
const PLANETS_SRC: &str = include_str!("../../planets.toml");

/// Planet id -> display name, parsed once from `planets.toml` client-side
/// (the same file the bridge thread's `build_api` reads) so the GUI never
/// has to round-trip to the orchestrator just to know who owns a planet.
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

/// Explorer id -> character name. Fixed at spawn time everywhere the
/// explorers are created (`builder.rs`, `main.rs`): Viviana is always 1,
/// Jebediah always 2.
fn explorer_display_name(id: u32) -> &'static str {
    match id {
        1 => "Viviana",
        2 => "Jebediah",
        _ => "Unknown Explorer",
    }
}

/// Maps a factory crate name (from `planets.toml`) to the team's display
/// name, per the professor's "PLANETS REPO'S" list.
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

// ── Bevy resources ─────────────────────────────────────────────────────────────

/// `None` until the bridge thread has written a real snapshot at least once.
/// Distinguishing "not ready yet" from "a real snapshot with zero alive
/// planets" matters: without it, the default empty snapshot present during
/// the first ~250ms would look identical to "the whole galaxy just died",
/// which used to make every planet incorrectly skip its death animation (see
/// `sync_planets`).
#[derive(Resource, Clone)]
struct LiveSnapshot(Arc<Mutex<Option<GalaxySnapshot>>>);

#[derive(Resource, Default)]
struct GalaxyState {
    alive: HashSet<u32>,
    explorer_planet: HashMap<u32, u32>,
    /// Real bag content per explorer, fetched live from the explorer thread
    /// each snapshot poll (`OrchestratorApi::bag_content`) — not GUI-guessed.
    explorer_bag: HashMap<u32, Vec<(ResourceKind, usize)>>,
    /// Real, live adjacency from the backend topology — replaces the old
    /// hardcoded ring, which never reflected planets dying.
    neighbors: HashMap<u32, Vec<u32>>,
    personality: Personality,
    /// Global hostility in `[0.0, 1.0]`; drives the Solace/Eclipse crossfade.
    hostility: f64,
    phase_elapsed: f64,
    /// False until `poll_snapshot` has read a real snapshot at least once.
    /// Without this, `alive` starts as an empty `HashSet` by default —
    /// indistinguishable from "every planet just died" — and systems like
    /// `sync_planets` would read that empty default during the startup
    /// window before the bridge thread's first real update arrives, kicking
    /// off every planet's death animation before the game even begins.
    ready: bool,
}

/// Queues a one-shot glitch overlay flash on a portrait, fired when a manual
/// sunray/asteroid override "infiltrates" that personality's mood.
#[derive(Resource, Default)]
struct GlitchQueue(Vec<Personality>);

/// Marks the glitch-flash overlay child of one portrait bubble. `remaining`
/// counts down from [`GLITCH_DURATION_SECS`]; `0.0` means hidden.
#[derive(Component, Default)]
struct GlitchOverlay {
    remaining: f32,
}

const GLITCH_DURATION_SECS: f32 = 1.2;

/// How suspicious each personality is of the player, as a discrete tier: 0 =
/// calm, 1 = confused, 2 = suspicious. Driven entirely by real manual
/// overrides (the same "infiltration" moments that already trigger
/// `GlitchQueue`) via `apply_manual_override` — not a fabricated stat, and
/// not a continuously-decaying meter either: each personality has a
/// preferred action (Solace: Sunray, Eclipse: Asteroid). An override that
/// matches her own preference only ever nudges her to "confused" — she isn't
/// against it, just puzzled she didn't choose to act. An override AGAINST
/// her preference escalates one tier per occurrence; the third one is a
/// refusal — the request is blocked outright and the tier resets to 0,
/// exactly like the pressure "resolved itself" for her.
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

/// Highest resting tier (2 = suspicious); tier 3 (refusal) is instantaneous —
/// it blocks the action and immediately resets to 0, so it's never a resting
/// state the meter needs to render.
const SUSPICION_MAX_RESTING_TIER: u8 = 2;

enum SuspicionOutcome {
    /// The request goes through. `reaction` is the portrait animation tag to
    /// queue, if any (only fires the moment a tier is first reached).
    Allowed { reaction: Option<&'static str> },
    /// The third against-preference override in a row: blocked outright.
    Refused,
}

/// Applies one manual Sunray/Asteroid override to `personality`'s suspicion
/// tier and decides whether the request is allowed through.
fn apply_manual_override(state: &mut SuspicionState, personality: &Personality, sunray: bool) -> SuspicionOutcome {
    let preferred = match personality {
        Personality::Solace => sunray,
        Personality::Eclipse => !sunray,
    };
    let tier = state.tier_mut(personality);
    if preferred {
        let was_calm = *tier == 0;
        *tier = (*tier).max(1);
        SuspicionOutcome::Allowed { reaction: if was_calm { Some("confused") } else { None } }
    } else if *tier >= SUSPICION_MAX_RESTING_TIER {
        *tier = 0;
        SuspicionOutcome::Refused
    } else {
        *tier += 1;
        let reaction = if *tier == 1 { "confused" } else { "suspicious" };
        SuspicionOutcome::Allowed { reaction: Some(reaction) }
    }
}

/// Marks a suspicion meter's fill bar, bottom-anchored, height driven by the
/// matching personality's live suspicion tier.
#[derive(Component)]
struct SuspicionMeterFill(Personality);

/// Top-anchored at the same level as the portrait bubble (see
/// `spawn_suspicion_meter` call sites) — the top position is fixed, so the
/// bar's bottom edge is what moves when its size changes.
const SUSPICION_METER_W: f32 = 30.0 * DISPLAY_SCALE;
const SUSPICION_METER_H: f32 = 88.0 * DISPLAY_SCALE;
const SUSPICION_FILL_INSET: f32 = 3.0 * DISPLAY_SCALE;

// ── ECS components ─────────────────────────────────────────────────────────────

#[derive(Component)]
struct AnimationConfig {
    first: usize,
    last: usize,
    timer: Timer,
    looping: bool,
    // When the current one-shot ends, optionally play another range before hiding.
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

/// Loaded once at startup: one icon texture per real carried-resource kind.
#[derive(Resource)]
struct ResourceIcons(HashMap<ResourceKind, Handle<Image>>);

impl ResourceIcons {
    fn load(asset_server: &AssetServer) -> Self {
        use ResourceKind::*;
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
        Self(pairs.into_iter().map(|(k, f)| (k, asset_server.load(f))).collect())
    }
}

/// How many carried-resource icons to show at once above an explorer's
/// sprite. `slot` indexes into that explorer's real bag content; slots past
/// the end of the bag are simply hidden.
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
    /// One-shot "react_other" — played when this explorer notices it's sharing
    /// a planet with the other explorer. They can't actually communicate, so
    /// this is a GUI-only flourish reacting to real, shared position data.
    ReactingOther,
}

const CO_REACT_HOLD_SECS: f32 = 1.0;

/// Per-explorer movement + animation state machine.
/// Explorers do NOT use AnimationConfig — this owns all their animation state.
#[derive(Component)]
struct ExplorerAnim {
    // Frame ranges (inclusive), all read from this explorer's own Aseprite
    // JSON export (jeb.json / viviana.json) rather than hand-copied.
    moving_range: (usize, usize),
    arrived_range: (usize, usize),
    departing_range: (usize, usize),
    // The "settled" range is what plays when the explorer is resting on a
    // planet. Viviana: collecting. Jeb: idle.
    settled_range: (usize, usize),
    react_other_range: (usize, usize),
    fps: f32,
    anim_timer: Timer,
    // Movement state.
    cur_planet: u32,
    target_planet: u32,
    from_pos: Vec2,
    to_pos: Vec2,
    move_t: f32,
    phase: ExplorerPhase,
    phase_timer: Timer,
    /// Whether this explorer was sharing a planet with another explorer as of
    /// the last check. Edge-triggered: react_other fires the instant this
    /// flips false -> true (checked every frame against real backend
    /// positions, not sampled periodically), and resets to false as soon as
    /// this explorer leaves the planet, so two explorers get a fresh "oh, you
    /// again" every time their paths actually cross rather than once per
    /// fixed interval.
    was_co_located: bool,
    /// Real hops (destination planet ids), one per confirmed `ExplorerMoved`
    /// event, in the order the backend actually reported them. Consumed one
    /// at a time from `Settled`/`ReactingOther` — this is what guarantees a
    /// burst of several real moves landing between two ~250ms snapshot polls
    /// still gets animated through every intermediate planet, in order,
    /// instead of only ever showing wherever the explorer most recently was.
    pending_hops: VecDeque<u32>,
}

impl ExplorerAnim {
    fn new_viviana(start_planet: u32, tags: &AnimTags) -> Self {
        let pos = explorer_offset_pos(start_planet, 1);
        Self {
            moving_range:      tags.viviana("moving"),
            arrived_range:     tags.viviana("arrived"),
            departing_range:   tags.viviana("departing"),
            settled_range:     tags.viviana("collecting"),
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
            moving_range:      tags.jeb("moving"),
            arrived_range:     tags.jeb("arrived"),
            departing_range:   tags.jeb("departing"),
            settled_range:     tags.jeb("idle"),
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
    // Reactions queued to play after the current one finishes holding.
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
    // true = interrupt whatever is playing and clear pending; false = queue after current reaction
    priority: bool,
}

// ── Animation tags (loaded from the Aseprite JSON exports) ────────────────────
//
// solace.json/eclipse.json each ship a `meta.frameTags` array naming every
// animation ("idle", "sending", "unwanted_event", "breaking", "awakening",
// etc.) with its frame range. Frame ranges used to be hand-copied into the
// event-dispatch match arms below as bare numbers — correct today, but silently
// stale the moment either sheet is ever re-exported with a different frame
// count, with no compiler error to catch it. Reading the tag names by name
// from the JSON at startup means the source of truth is the export itself.
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
    /// All 7 `planet_*.json` exports share identical tag names/ranges
    /// (checked directly: idle/hit/receive/dying/destroyed are byte-for-byte
    /// the same across every planet type) — one shared map covers all of them,
    /// no per-planet-type lookup needed.
    planet: HashMap<String, (usize, usize)>,
}

impl AnimTags {
    /// Reads every sheet's `.json` export straight from the assets folder —
    /// the same folder Bevy's own asset server resolves `asset_server.load`
    /// paths against (`CARGO_MANIFEST_DIR`/assets in dev builds), so this
    /// stays in lockstep with whatever sheet is actually being rendered.
    fn load() -> Self {
        Self {
            solace: Self::load_one("solace.json"),
            eclipse: Self::load_one("eclipse.json"),
            solace_stance: Self::load_one("solace_stance.json"),
            eclipse_stance: Self::load_one("eclipse_stance.json"),
            jeb: Self::load_one("jeb.json"),
            viviana: Self::load_one("viviana.json"),
            planet: Self::load_one("planet_orbitron.json"),
        }
    }

    fn load_one(file: &str) -> HashMap<String, (usize, usize)> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join(file);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read animation tags from {path:?}: {e}"));
        let export: AsepriteExport = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("failed to parse animation tags from {path:?}: {e}"));
        export.meta.frame_tags.into_iter().map(|t| (t.name, (t.from, t.to))).collect()
    }

    /// Frame range for a named tag on `personality`'s portrait sheet. Panics
    /// on an unrecognized tag — a typo here should fail loudly at startup
    /// instead of silently playing nothing (or the wrong frames).
    fn get(&self, personality: &Personality, tag: &str) -> (usize, usize) {
        let map = match personality {
            Personality::Solace => &self.solace,
            Personality::Eclipse => &self.eclipse,
        };
        Self::lookup(map, tag, "solace.json/eclipse.json")
    }

    /// Frame range for a named tag on `personality`'s stance sheet.
    fn stance(&self, personality: &Personality, tag: &str) -> (usize, usize) {
        let map = match personality {
            Personality::Solace => &self.solace_stance,
            Personality::Eclipse => &self.eclipse_stance,
        };
        Self::lookup(map, tag, "solace_stance.json/eclipse_stance.json")
    }

    /// Frame range for a named tag on Jeb's sheet (`jeb.json`).
    fn jeb(&self, tag: &str) -> (usize, usize) {
        Self::lookup(&self.jeb, tag, "jeb.json")
    }

    /// Frame range for a named tag on Viviana's sheet (`viviana.json`).
    fn viviana(&self, tag: &str) -> (usize, usize) {
        Self::lookup(&self.viviana, tag, "viviana.json")
    }

    /// Frame range for a named tag shared by every planet sheet.
    fn planet(&self, tag: &str) -> (usize, usize) {
        Self::lookup(&self.planet, tag, "planet_*.json")
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

// ── Ambient background movement ────────────────────────────────────────────────

/// Slow sine-wave bob + rotation for world-space sprites (nebulas). Avoids
/// needing wrap-around logic — it just oscillates gently around a fixed point.
#[derive(Component)]
struct DriftBob {
    base: Vec2,
    amp: Vec2,
    speed: f32,
    phase: f32,
    rot_speed: f32,
}

/// Which nebula variant is tied to which personality; only the active one
/// fades in, the other fades out — same crossfade idea as the vignette tint.
#[derive(Component)]
struct NebulaTag(Personality);

/// Marks one of the two stance sprites (the standing full-body figure for a
/// personality). Unlike the portrait bubbles — which are both always visible,
/// just dimmed/brightened by dominance — only ONE stance is ever shown: the
/// shared body has a single physical presence, so there can never be two
/// stances visible at once. `sync_stance_visibility` hides whichever tag
/// doesn't match the currently active personality.
#[derive(Component)]
struct StanceTag(Personality);

/// Drives the small 4-frame standing-idle loop on a stance sprite. Separate
/// from `PortraitConfig`/`AnimationConfig` since a stance never reacts to
/// events or plays one-shots — it just idles forever while visible.
#[derive(Component)]
struct StanceAnim {
    first: usize,
    last: usize,
    timer: Timer,
}

impl StanceAnim {
    fn new(range: (usize, usize)) -> Self {
        Self { first: range.0, last: range.1, timer: Timer::from_seconds(1.0 / 6.0, TimerMode::Repeating) }
    }
}

/// Periodic brightness pulse for "flashing" bright stars.
#[derive(Component)]
struct Twinkle {
    speed: f32,
    phase: f32,
    base_alpha: f32,
    pulse_alpha: f32,
}

/// Same sine-wave bob as `DriftBob`, but for UI-space stars drifting *inside*
/// a panel (Concept 3, "Nebula Glass") — driven through `Node.left/top`
/// instead of `Transform`, since these are bevy_ui children, not world sprites.
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

/// Custom star cursor — the OS cursor is hidden and this follows it instead.
#[derive(Component)]
struct CursorTag;

// ── Galaxy layout ─────────────────────────────────────────────────────────────

// Mirrors the real topology in `galaxy.txt` — a circulant C7(1,2) graph
// (distance-1 ring edges plus distance-2 cross-links), not the old plain
// 7-edge ring.
const CONNECTIONS: &[(usize, usize)] = &[
    (0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (5, 6), (6, 0),
    (0, 2), (1, 3), (2, 4), (3, 5), (4, 6), (5, 0), (6, 1),
];

/// Elliptical, not circular — the viewport hole has much more horizontal
/// room than vertical (top screen + bottom console eat into height far more
/// than the walls eat into width), so a uniform radius either clips the top
/// planet under the top screen or wastes the available width. Radii chosen
/// with real margin, not a knife-edge fit: topmost planet's sprite edge
/// lands ~140px below the top screen's bottom edge (y=68), and the bottom
/// stays ~50px clear of the console (y=647).
const RING_RADIUS_X: f32 = 230.0 * DISPLAY_SCALE;
const RING_RADIUS_Y: f32 = 175.0 * DISPLAY_SCALE;

/// Center of the ring in world space. The cockpit's viewport hole (measured
/// directly from `cockpit.png`'s alpha channel) is centered slightly above
/// screen-center because the bottom console (253px) is much taller than the
/// top screen (64px) — this offsets the ring to match, not screen center.
const RING_CENTER: Vec2 = Vec2::new(0.0, 78.0 * DISPLAY_SCALE);

fn planet_position(index: usize) -> Vec2 {
    let angle = FRAC_PI_2 - index as f32 * 2.0 * PI / 7.0;
    RING_CENTER + Vec2::new(RING_RADIUS_X * angle.cos(), RING_RADIUS_Y * angle.sin())
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

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() {
    let snapshot: Arc<Mutex<Option<GalaxySnapshot>>> = Arc::new(Mutex::new(None));
    let snap_write = Arc::clone(&snapshot);

    let inspection = Inspection(Arc::new(Mutex::new(InspectionData::None)));
    let inspection_write = inspection.clone();

    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<UiCommand>();
    let (ready_tx, ready_rx) = crossbeam_channel::bounded::<()>(1);

    std::thread::spawn(move || {
        // Wait for the galaxy scene to actually be on screen (signaled by
        // `setup`) before spawning planets and starting the logic loop.
        let _ = ready_rx.recv();

        let mut api = bipolar_orchestrator::builder::build_api(GALAXY_SRC, PLANETS_SRC);

        // Logic does NOT auto-start anymore — the galaxy sits static (planets,
        // explorers, everything spawned but idle) until the user presses the
        // Start button. Lets a demo show the assets first, then trigger motion.
        *snap_write.lock().unwrap() = Some(bipolar_orchestrator::snapshot::build(&api));

        let mut logic_running = false;
        loop {
            // Drain manual commands from the GUI before sleeping. These share the
            // same orchestrator channels as the autonomous logic loop — the same
            // way the CLI's `interactive_loop` does when logic is running.
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
                        // `with_scale_factor_override(1.0)` forces 1 logical
                        // pixel == 1 physical pixel, ignoring whatever the OS
                        // DPI/display-scaling setting is. Without it, the
                        // window itself comes out at the exact WINDOW_WIDTH/
                        // HEIGHT physical size, but bevy_ui's `Val::Px` layout
                        // (which all our cockpit positions use) gets scaled
                        // by the OS's scale factor on top of that — the two
                        // stop agreeing, leaving dead space or clipping
                        // depending on which way the mismatch goes.
                        resolution: WindowResolution::new(WINDOW_WIDTH as u32, WINDOW_HEIGHT as u32)
                            .with_scale_factor_override(1.0),
                        // True borderless fullscreen: no title bar, sits above
                        // the taskbar entirely — needed for the presentation.
                        // Bevy overrides the requested resolution to match the
                        // monitor's actual physical size in this mode, which
                        // is exactly why DISPLAY_SCALE above is tuned to
                        // Vale's confirmed 1920x1080 screen (1.2x of the
                        // 1600x900 reference, matching precisely).
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
        .add_systems(Update, (sync_hologram_visibility, sync_hologram_content, sync_explorers, sync_action_buttons_enabled, sync_selection_validity, sync_stance_visibility, drive_stance_anim, sync_resource_badges))
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

/// Non-blocking drain of every pending [`UiCommand`], executed on the bridge
/// thread. Each variant maps directly to the same `OrchestratorApi` method the
/// CLI's `interactive_loop` calls.
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
                    Ok(bag) => bag.iter().map(|(res, count)| format!("{res:?} x{count}")).collect(),
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
                        InspectionData::PlanetInfo { id, energy_cells: 0, charged_cells: 0, has_rocket: false }
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

// ── Bridge / snapshot systems ──────────────────────────────────────────────────

fn poll_snapshot(
    live: Res<LiveSnapshot>,
    tags: Res<AnimTags>,
    mut state: ResMut<GalaxyState>,
    mut queue: ResMut<PortraitEventQueue>,
    mut planet_queue: ResMut<PlanetReactionQueue>,
    mut explorer_anims: Query<(&ExplorerTag, &mut ExplorerAnim)>,
) {
    let Ok(mut guard) = live.0.try_lock() else { return };
    let Some(snap) = guard.as_mut() else { return };
    // Drain events immediately. This system runs every rendered frame (up to
    // 60-144fps) but the bridge thread only produces a new snapshot every
    // ~250ms — reading `&snap.events` without draining replayed the same
    // events dozens of times per snapshot, ballooning the portrait reaction
    // queue into the thousands and burying every new event under a backlog
    // that would take minutes to play through. That's why nothing new ever
    // visibly triggered.
    let events = std::mem::take(&mut snap.events);
    state.ready = true;

    let new_personality = snap.personality.clone();

    if new_personality != state.personality {
        // The loser's "breaking" plays, the winner's "awakening" plays —
        // frame ranges read from each sheet's own JSON export, not hand-copied.
        let (loser, winner) = match &new_personality {
            Personality::Eclipse => (Personality::Solace, Personality::Eclipse),
            Personality::Solace  => (Personality::Eclipse, Personality::Solace),
        };
        let loser_break = tags.get(&loser, "breaking");
        let winner_awaken = tags.get(&winner, "awakening");
        queue.0.push_back(PortraitReaction { target: loser,  first: loser_break.0,  last: loser_break.1,  priority: true });
        queue.0.push_back(PortraitReaction { target: winner, first: winner_awaken.0, last: winner_awaken.1, priority: true });
    }

    // Use the OLD personality (state.personality) to read intent: events fired under it.
    for event in &events {
        match event {
            GalaxyEvent::SunraySent { .. } => {
                // Solace reacts by "sending"; Eclipse's sunray is unwelcome —
                // her "unwanted_event" tag.
                let tag = match state.personality {
                    Personality::Solace  => "sending",
                    Personality::Eclipse => "unwanted_event",
                };
                let (first, last) = tags.get(&state.personality, tag);
                queue.0.push_back(PortraitReaction {
                    target: state.personality.clone(), first, last, priority: false,
                });
            }
            GalaxyEvent::SunrayReceived { planet_id } => {
                if state.personality == Personality::Solace {
                    // Sunray landed — Solace is happy
                    let (first, last) = tags.get(&Personality::Solace, "satisfied");
                    queue.0.push_back(PortraitReaction {
                        target: Personality::Solace, first, last, priority: false,
                    });
                }
                let (first, last) = tags.planet("receive");
                planet_queue.0.push_back(PlanetReaction { planet_id: *planet_id, first, last });
            }
            GalaxyEvent::AsteroidSent { .. } => {
                // Solace's "unwanted_event" (guilty — she doesn't want to kill)
                // is high priority: it interrupts whatever's queued/playing
                // instead of waiting its turn, so it's never buried behind
                // routine sunray reactions and missed. Eclipse's "sending" is
                // routine, stays low priority.
                let (tag, priority) = match state.personality {
                    Personality::Eclipse => ("sending", false),
                    Personality::Solace  => ("unwanted_event", true),
                };
                let (first, last) = tags.get(&state.personality, tag);
                queue.0.push_back(PortraitReaction {
                    target: state.personality.clone(), first, last, priority,
                });
            }
            GalaxyEvent::AsteroidDeflected { planet_id } => {
                if state.personality == Personality::Eclipse {
                    // Asteroid hit — Eclipse is satisfied
                    let (first, last) = tags.get(&Personality::Eclipse, "satisfied");
                    queue.0.push_back(PortraitReaction {
                        target: Personality::Eclipse, first, last, priority: false,
                    });
                }
                let (first, last) = tags.planet("hit");
                planet_queue.0.push_back(PlanetReaction { planet_id: *planet_id, first, last });
            }
            GalaxyEvent::PlanetDestroyed { planet_id } => {
                // Trigger the final hit visual; the personality flip handles portraits.
                let (first, last) = tags.planet("hit");
                planet_queue.0.push_back(PlanetReaction { planet_id: *planet_id, first, last });
            }
            GalaxyEvent::ExplorerKilled { explorer_id } => {
                // No dedicated visual yet — logged so it's visible during testing.
                println!("[gui] Explorer {explorer_id} died with their planet");
            }
            GalaxyEvent::ExplorerMoved { explorer_id, to, .. } => {
                // Queue the real hop rather than relying on the polled
                // `explorer_planet` position diff — a burst of several real
                // moves landing between two ~250ms snapshot polls would
                // otherwise only ever show the *last* one, silently skipping
                // every real intermediate planet. drive_explorers drains this
                // queue one hop at a time, in order.
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
    state.explorer_bag = snap.explorers.iter().map(|e| (e.id, e.bag.clone())).collect();
    state.neighbors = snap.neighbors.clone();
    state.hostility = snap.hostility;
    state.phase_elapsed = snap.phase_elapsed;
}

/// Clears a stale `Selection` once its target no longer exists (destroyed
/// planet, dead explorer). Without this, a selected-then-destroyed planet
/// stayed "selected" forever — every subsequent Sunray/Asteroid press kept
/// targeting the dead planet id and silently failing with "not found",
/// which looked exactly like the buttons had stopped working.
fn sync_selection_validity(state: Res<GalaxyState>, mut selection: ResMut<Selection>) {
    if !state.ready {
        return;
    }
    if let Some(p) = selection.planet {
        if !state.alive.contains(&p) {
            selection.planet = None;
        }
    }
    if let Some(e) = selection.explorer {
        if !state.explorer_planet.contains_key(&e) {
            selection.explorer = None;
        }
    }
}

// ── Explorer movement ──────────────────────────────────────────────────────────

/// Hides an explorer's sprite once it's no longer in the backend snapshot
/// (killed when their planet was destroyed). Mirrors `sync_planets` — without
/// this, a dead explorer's sprite just sits frozen on screen forever, since
/// nothing else despawns or hides it.
fn sync_explorers(
    state: Res<GalaxyState>,
    mut query: Query<(&ExplorerTag, &mut Visibility)>,
) {
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

/// Spawns the (initially hidden) resource-badge sprites for one explorer.
/// Their image/position/visibility are all driven each frame by
/// `sync_resource_badges` from the real bag content in `GalaxyState`.
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

/// Positions and shows/hides each carried-resource badge above its explorer,
/// reading the real bag content fetched live from the explorer thread each
/// snapshot poll. Runs after `drive_explorers` so badges follow the
/// explorer's current (possibly mid-hop) position rather than lagging a
/// frame behind.
fn sync_resource_badges(
    state: Res<GalaxyState>,
    icons: Res<ResourceIcons>,
    explorer_q: Query<(&ExplorerTag, &Transform), Without<ResourceBadge>>,
    mut badge_q: Query<(&ResourceBadge, &mut Transform, &mut Sprite, &mut Visibility), Without<ExplorerTag>>,
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
                let x_offset = (badge.slot as f32 - (RESOURCE_BADGE_SLOTS as f32 - 1.0) / 2.0) * 22.0 * DISPLAY_SCALE;
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

    // Snapshot every explorer's real backend planet up front so the
    // co-location check below (used by react_other) can see the *other*
    // explorer's position without a second, nested query over the same
    // component set.
    let backend_positions: Vec<(u32, u32)> = state.explorer_planet.iter().map(|(&id, &p)| (id, p)).collect();

    for (tag, mut tf, mut sprite, mut ea) in &mut query {
        ea.anim_timer.tick(delta);
        let tick = ea.anim_timer.just_finished();

        // Only decide on a new destination while resting — a move already in
        // flight finishes its own hop first, so real moves get replayed in
        // order rather than a later one cutting an earlier one short.
        let can_start_new_move = matches!(ea.phase, ExplorerPhase::Settled | ExplorerPhase::ReactingOther);

        if can_start_new_move {
            // Prefer a queued real hop (from a confirmed `ExplorerMoved`
            // event) over the raw polled `explorer_planet` position: the
            // queue preserves every hop in order even if several land between
            // two ~250ms snapshot polls, where the polled position alone
            // would only ever show the *last* one and silently skip the rest
            // — which is exactly what read as "wrong planet, moving too
            // fast". Falling back to the polled position keeps things working
            // even if a move event was ever missed for some reason.
            //
            // A queued hop's planet can die *after* being queued but *before*
            // the animation catches up to it (the queue can lag several real
            // seconds behind during a burst) — animating a full walk to, and
            // settling on, a planet whose sprite has since been hidden is
            // exactly what read as "the explorer is standing in empty space."
            // Skip any now-dead hops instantly (no wasted travel animation to
            // a place that no longer visually exists) rather than visiting
            // them.
            let mut next = None;
            while let Some(bp) = ea.pending_hops.pop_front() {
                if state.alive.contains(&bp) {
                    next = Some(bp);
                    break;
                }
            }
            let next = next
                .or_else(|| state.explorer_planet.get(&tag.0).copied().filter(|&bp| bp != ea.cur_planet));

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
                if tick {
                    if let Some(atlas) = &mut sprite.texture_atlas {
                        let (f, l) = ea.settled_range;
                        if atlas.index >= l { atlas.index = f; } else { atlas.index += 1; }
                    }
                }
                // Explorers can't actually talk to each other, but it's a fun
                // touch to have them notice/react when they end up sharing a
                // planet — checked every frame against the real backend
                // positions of every other explorer, edge-triggered so it
                // fires the instant paths cross rather than on a periodic
                // sample that could miss a brief overlap.
                // Compare this explorer's own *real* backend planet (not
                // `ea.cur_planet`, which can lag several real hops behind
                // during a burst — see `pending_hops`) against the other
                // explorer's real backend planet. Comparing a possibly-stale
                // local value against a live one was why this almost never
                // fired even when the sprites visually looked adjacent.
                let co_located = state
                    .explorer_planet
                    .get(&tag.0)
                    .is_some_and(|&my_p| backend_positions.iter().any(|&(id, p)| id != tag.0 && p == my_p));
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
                if tick {
                    if let Some(atlas) = &mut sprite.texture_atlas {
                        let (f, l) = ea.react_other_range;
                        if atlas.index >= l { atlas.index = f; } else { atlas.index += 1; }
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
                if tick {
                    if let Some(atlas) = &mut sprite.texture_atlas {
                        let (_, l) = ea.departing_range;
                        if atlas.index < l { atlas.index += 1; }
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
                if tick {
                    if let Some(atlas) = &mut sprite.texture_atlas {
                        let (f, l) = ea.moving_range;
                        if atlas.index >= l { atlas.index = f; } else { atlas.index += 1; }
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
                if tick {
                    if let Some(atlas) = &mut sprite.texture_atlas {
                        let (_, l) = ea.arrived_range;
                        if atlas.index < l { atlas.index += 1; }
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

// ── Planet death ───────────────────────────────────────────────────────────────

/// Detects when a planet leaves alive_planets and triggers its death animation sequence.
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
    // Don't touch anything until the first real snapshot has arrived — the
    // default-empty `state.alive` before that point would otherwise look
    // identical to "every planet just died".
    if !state.ready {
        return;
    }

    for (tag, mut vis, mut pstate, mut anim, mut sprite) in &mut query {
        match *pstate {
            PlanetState::Alive => {
                if state.alive.contains(&tag.0) {
                    *vis = Visibility::Inherited;
                } else {
                    // Planet just died — start dying, then destroyed, then hide.
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
            // Dying and Dead are managed entirely by drive_planet_anim.
            PlanetState::Dying | PlanetState::Dead => {}
        }
    }
}

/// Drives all planet animations, including the two-phase death sequence.
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
                    // One-shot reaction (hit/receive) finished — return to idle.
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

/// Applies queued hit/receive reactions to alive planets.
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

// ── Portrait sync ───────────────────────────────────────────────────────────────
//
// Both portraits are always visible in their bottom-bar corners. Only the
// active personality glows at full brightness; the inactive one is dimmed —
// per the design, this should communicate who's in control without reading
// any text.

const PORTRAIT_ACTIVE_TINT: Color = Color::srgba(1.0, 1.0, 1.0, 1.0);
const PORTRAIT_DIM_TINT: Color = Color::srgba(0.55, 0.55, 0.6, 0.55);

fn sync_portrait_glow(
    state: Res<GalaxyState>,
    mut query: Query<(&PortraitTag, &mut ImageNode), Without<GlitchOverlay>>,
) {
    for (tag, mut image) in &mut query {
        // Solace owns the calm end (hostility -> 0), Eclipse the hostile end
        // (hostility -> 1) — crossfading continuously instead of snapping.
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
                    // Priority reactions (flip) interrupt immediately; idle portraits start at once.
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

/// Shows only the active personality's stance, hiding the other — the shared
/// body can only physically be standing as one personality at a time, unlike
/// the portrait bubbles (which are both always on screen, just dimmed).
fn sync_stance_visibility(state: Res<GalaxyState>, mut query: Query<(&StanceTag, &mut Visibility)>) {
    for (tag, mut vis) in &mut query {
        *vis = if tag.0 == state.personality { Visibility::Visible } else { Visibility::Hidden };
    }
}

/// Loops the 4-frame standing-idle animation on whichever stance is currently
/// visible. Runs unconditionally on both (cheap, and keeps the hidden one's
/// frame in sync so there's no visible jump the instant it reappears).
fn drive_stance_anim(time: Res<Time>, mut query: Query<(&mut StanceAnim, &mut ImageNode)>) {
    for (mut anim, mut image) in &mut query {
        anim.timer.tick(time.delta());
        if anim.timer.just_finished() {
            if let Some(atlas) = &mut image.texture_atlas {
                atlas.index = if atlas.index >= anim.last { anim.first } else { atlas.index + 1 };
            }
        }
    }
}

/// Flashes a portrait's glitch overlay when a manual override "infiltrates"
/// it, then fades it back out. A single static frame flickered via alpha
/// jitter reads as a hack landing, at zero extra art cost.
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
        let flicker = ((elapsed * 45.0).sin().abs() * 0.5 + 0.5) as f32;
        image.color.set_alpha(fraction * flicker);
    }
}

/// Fills each personality's suspicion meter to match their live tier.
/// `apply_manual_override` (called directly from the Sunray/Asteroid button
/// handlers) is what actually changes the tier and queues portrait
/// reactions — this system just keeps the little side-stat bar in sync.
fn sync_suspicion_meter(suspicion: Res<SuspicionState>, mut query: Query<(&SuspicionMeterFill, &mut Node)>) {
    let max_h = SUSPICION_METER_H - SUSPICION_FILL_INSET * 2.0;
    for (fill, mut node) in &mut query {
        let frac = suspicion.tier(&fill.0) as f32 / SUSPICION_MAX_RESTING_TIER as f32;
        node.height = Val::Px(max_h * frac);
    }
}

// ── Generic sprite animation ───────────────────────────────────────────────────

/// Drives looping AnimationConfig sprites. Planets are excluded — use drive_planet_anim.
fn animate_sprites(
    time: Res<Time>,
    mut query: Query<(&mut Sprite, &mut AnimationConfig), Without<PlanetState>>,
) {
    for (mut sprite, mut anim) in &mut query {
        anim.timer.tick(time.delta());
        if anim.timer.just_finished() {
            if let Some(atlas) = &mut sprite.texture_atlas {
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
}

// ── Connection lines ───────────────────────────────────────────────────────────

fn draw_connections(mut gizmos: Gizmos, state: Res<GalaxyState>) {
    for &(a, b) in CONNECTIONS {
        let pid_a = (a + 1) as u32;
        let pid_b = (b + 1) as u32;
        if !state.alive.is_empty() {
            if !state.alive.contains(&pid_a) || !state.alive.contains(&pid_b) {
                continue;
            }
        }
        gizmos.line_2d(
            planet_position(a),
            planet_position(b),
            Color::srgba(0.4, 0.4, 0.7, 0.4),
        );
    }
}

// ── Vignette ──────────────────────────────────────────────────────────────────

fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let a = a.to_linear();
    let b = b.to_linear();
    Color::linear_rgba(
        a.red   + (b.red   - a.red)   * t,
        a.green + (b.green - a.green) * t,
        a.blue  + (b.blue  - a.blue)  * t,
        a.alpha + (b.alpha - a.alpha) * t,
    )
}

fn sync_vignette(
    state: Res<GalaxyState>,
    time: Res<Time>,
    mut query: Query<&mut Sprite, With<VignetteTag>>,
) {
    // Kept extremely low on purpose — this is ambient mood lighting, not a
    // color wash. Solace in particular reads yellow very easily even at low
    // alpha, so it's cut much further than Eclipse.
    let target = match state.personality {
        Personality::Solace  => Color::srgba(1.0, 0.55, 0.05, 0.015),
        Personality::Eclipse => Color::srgba(0.25, 0.0,  0.45, 0.04),
    };
    let speed = time.delta_secs() * 3.0;
    for mut sprite in &mut query {
        sprite.color = lerp_color(sprite.color, target, speed.min(1.0));
    }
}

// ── Ambient movement systems ────────────────────────────────────────────────────

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
        sprite.color.set_alpha(tw.base_alpha + pulse * tw.pulse_alpha);
    }
}

/// Drives the small drifting stars inside each panel (the "Nebula Glass" effect).
fn drive_panel_stars(time: Res<Time>, mut query: Query<(&mut Node, &PanelStarBob)>) {
    let t = time.elapsed_secs();
    for (mut node, bob) in &mut query {
        node.left = Val::Px(bob.base.x + (t * bob.speed + bob.phase).sin() * bob.amp.x);
        node.top = Val::Px(bob.base.y + (t * bob.speed * 0.8 + bob.phase).cos() * bob.amp.y);
    }
}

/// Fires roughly every 20-40s: spawns a streak crossing the screen from one
/// random edge, moving toward roughly the opposite side.
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
    // The art faces down-left natively. Half the time it travels that way
    // unflipped; the other half it's mirrored (flip_x) and travels down-right.
    let flipped = rng.random_bool(0.5);
    let start = Vec2::new(rng.random_range(-300.0..500.0), rng.random_range(200.0..360.0)) * DISPLAY_SCALE;
    let dir = if flipped { Vec2::new(1.0, -1.0) } else { Vec2::new(-1.0, -1.0) };
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

/// Custom star cursor following the OS cursor position (which is hidden).
fn follow_cursor(
    windows: Query<&Window, With<PrimaryWindow>>,
    mut query: Query<&mut Node, With<CursorTag>>,
) {
    let Ok(window) = windows.single() else { return };
    let Ok(mut node) = query.single_mut() else { return };
    if let Some(pos) = window.cursor_position() {
        node.left = Val::Px(pos.x - 8.0);
        node.top = Val::Px(pos.y - 8.0);
    }
}

// ── User input ──────────────────────────────────────────────────────────────────
//
// Left-click an explorer to select it, then left-click one of the neighboring
// planets (highlighted in green) to move it there. Left-click a planet with no
// explorer selected to target it for the console's Sunray/Asteroid buttons.
// Right-click either one to open its hologram (fetches real bag/planet data).
// Right-click empty space, or the same entity again, to close the hologram.
// Space toggles the logic loop.

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

    // Debug-only: E forces hostility toward Eclipse, S forces it toward
    // Solace — verifies the flip instantly instead of waiting through real
    // playtime for enough deaths to accumulate.
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
    let Some(cursor) = window.cursor_position() else { return };

    // Only treat clicks inside the cockpit viewport hole as galaxy clicks —
    // everything else (cockpit chrome, buttons) is handled by its own
    // Interaction-based systems and shouldn't also fire a sunray/asteroid.
    if cursor.x < VIEWPORT_HOLE_MIN.x
        || cursor.x > VIEWPORT_HOLE_MAX.x
        || cursor.y < VIEWPORT_HOLE_MIN.y
        || cursor.y > VIEWPORT_HOLE_MAX.y
    {
        return;
    }

    let Ok((camera, cam_transform)) = cameras.single() else { return };
    let Ok(world_pos) = camera.viewport_to_world_2d(cam_transform, cursor) else { return };

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
            if let Some(current) = current {
                if state.neighbors.get(&current).is_some_and(|ns| ns.contains(&tag.0)) {
                    let _ = cmds.0.send(UiCommand::MoveExplorer {
                        explorer_id,
                        dst: tag.0,
                    });
                    selection.explorer = None;
                }
            }
            return;
        }

        // No explorer selected — clicking a planet now just selects it for
        // the console's Sunray/Asteroid buttons to act on (spaceship-cockpit
        // theme: the board issues commands, direct clicks just point at things).
        selection.planet = if selection.planet == Some(tag.0) { None } else { Some(tag.0) };
        return;
    }

    // Right-click hit nothing inside the viewport — close whatever hologram
    // is open.
    if right {
        inspecting.0 = None;
    }
}

/// Fires a manual sunray or asteroid on `Selection::planet`. The glitch
/// always flashes on whoever is *currently in control* (`state.personality`)
/// — a manual override is "infiltrating" the one steering right now,
/// regardless of whether the action itself is a sunray or an asteroid. It
/// used to be hardcoded (sunray -> always Solace, asteroid -> always
/// Eclipse), which meant an asteroid sent while Solace was in control
/// glitched Eclipse instead of her.
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
    let Some(planet_id) = selection.planet else { return };
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
                queue.0.push_back(PortraitReaction { target: active, first, last, priority: true });
            }
        }
        // Refused: she notices the pattern and shuts it down — the request
        // never reaches the orchestrator at all.
        SuspicionOutcome::Refused => {
            let (first, last) = tags.get(&active, "refusal");
            queue.0.push_back(PortraitReaction { target: active, first, last, priority: true });
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
            fire_on_selected_planet(&selection, &cmds, &mut glitch, &mut suspicion, &tags, &mut queue, state.personality.clone(), true);
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
            fire_on_selected_planet(&selection, &cmds, &mut glitch, &mut suspicion, &tags, &mut queue, state.personality.clone(), false);
        }
    }
}

/// Shows only the Start/Pause/Resume button matching the current
/// [`LogicRunState`], and wires each of the three to the same toggle command.
fn is_active_transport(button: &TransportButton, state: LogicRunState) -> bool {
    matches!(
        (button, state),
        (TransportButton::Start, LogicRunState::NotStarted)
            | (TransportButton::Pause, LogicRunState::Running)
            | (TransportButton::Resume, LogicRunState::Paused)
    )
}

/// Full brightness for the active Start/Pause/Resume button; dimmed (but
/// still visible, not `Display::None`) for the other two — a gap where a
/// hidden button used to sit read as broken, not "inactive."
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

/// Shows/hides each hologram based on `Inspecting`, and only one of the two
/// at a time (matches the "walls light up one at a time" framing).
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
        *v = if show_explorer { Visibility::Visible } else { Visibility::Hidden };
    }
    for mut v in &mut planet_holo {
        *v = if show_planet { Visibility::Visible } else { Visibility::Hidden };
    }
}

/// Writes the bridge thread's fetched bag/planet-state data into whichever
/// hologram text is currently showing it.
fn sync_hologram_content(
    inspection: Res<Inspection>,
    names: Res<PlanetNames>,
    mut expl_text: Query<&mut Text, (With<ExplorerHologramText>, Without<PlanetHologramText>)>,
    mut planet_text: Query<&mut Text, (With<PlanetHologramText>, Without<ExplorerHologramText>)>,
) {
    let Ok(data) = inspection.0.try_lock() else { return };
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
        InspectionData::PlanetInfo { id, energy_cells, charged_cells, has_rocket } => {
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

/// Toggles the map hologram open/closed when the compass button is pressed.
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
        *vis = if open.0 { Visibility::Visible } else { Visibility::Hidden };
    }
}

/// Shows a node only while its planet is still alive — same real data
/// `sync_planets` uses, just read here for the mini-map's own node icons.
fn sync_map_nodes(state: Res<GalaxyState>, mut query: Query<(&MapNodeTag, &mut Visibility)>) {
    if !state.ready {
        return;
    }
    for (tag, mut vis) in &mut query {
        *vis = if state.alive.contains(&tag.0) { Visibility::Inherited } else { Visibility::Hidden };
    }
}

/// Shows an edge only while both its planets are still alive — mirrors
/// `draw_connections`'s exact rule (and its "not ready yet, show everything"
/// fallback before the first real snapshot arrives).
fn sync_map_edges(state: Res<GalaxyState>, mut query: Query<(&MapEdgeTag, &mut Visibility)>) {
    for (tag, mut vis) in &mut query {
        let (a, b) = CONNECTIONS[tag.0];
        let pid_a = (a + 1) as u32;
        let pid_b = (b + 1) as u32;
        let alive = state.alive.is_empty() || (state.alive.contains(&pid_a) && state.alive.contains(&pid_b));
        *vis = if alive { Visibility::Inherited } else { Visibility::Hidden };
    }
}

/// Mirrors `update_top_bar_personality`/`update_top_bar_cycle` into one line
/// inside the map hologram panel.
fn update_map_clock_text(state: Res<GalaxyState>, mut query: Query<(&mut Text, &mut TextColor), With<MapClockText>>) {
    let Ok((mut text, mut color)) = query.single_mut() else { return };
    let pct = (state.hostility * 100.0).round() as i32;
    let (label, tint) = match state.personality {
        Personality::Solace => ("SOLACE", SOLACE_GOLD),
        Personality::Eclipse => ("ECLIPSE", ECLIPSE_PURPLE),
    };
    **text = format!("{label} ({pct}%) — {}", format_mmss(state.phase_elapsed));
    color.0 = tint;
}

/// Simple hover feedback for cockpit buttons — brightens on hover, dims on
/// press. Skips repositioning (buttons are absolute-positioned at an exact
/// cockpit coordinate; nudging `top` directly would move them, not offset
/// them, since there's no separate "base position" tracked per button).
fn cockpit_button_hover(
    mut query: Query<
        (&Interaction, &mut ImageNode),
        (Changed<Interaction>, With<Button>, Without<SunrayButtonTag>, Without<AsteroidButtonTag>),
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

/// Dims Sunray/Asteroid when no planet is selected — pressing them then does
/// nothing (they only act on `Selection::planet`), which read as a silent,
/// confusing no-op. Now it's visually obvious nothing will happen.
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

/// Highlights the selected explorer's current planet and its valid move
/// destinations (neighbors that are still alive).
fn draw_selection_highlight(selection: Res<Selection>, state: Res<GalaxyState>, mut gizmos: Gizmos) {
    let Some(explorer_id) = selection.explorer else { return };
    let Some(&current) = state.explorer_planet.get(&explorer_id) else { return };

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

// ── Command-center UI: update systems ──────────────────────────────────────────

fn update_top_bar_personality(
    state: Res<GalaxyState>,
    mut query: Query<(&mut Text, &mut TextColor), With<TopBarPersonalityText>>,
) {
    let Ok((mut text, mut color)) = query.single_mut() else { return };
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

fn update_top_bar_cycle(state: Res<GalaxyState>, mut query: Query<&mut Text, With<TopBarCycleText>>) {
    let Ok(mut text) = query.single_mut() else { return };
    **text = format!("Cycle {}", format_mmss(state.phase_elapsed));
}

/// Temporary content for the chronicle screen until the real Cosmic
/// Chronicle log is built — shows the same overview stats the old "Galaxy
/// Overview" panel had, plus the current planet/explorer selection, so nothing
/// useful is lost while the cockpit redesign is in progress.
fn update_chronicle_screen_text(
    state: Res<GalaxyState>,
    selection: Res<Selection>,
    names: Res<PlanetNames>,
    mut query: Query<&mut Text, With<ChronicleScreenText>>,
) {
    let Ok(mut text) = query.single_mut() else { return };
    let personality = match state.personality {
        Personality::Solace => "Solace",
        Personality::Eclipse => "Eclipse",
    };
    let planet_sel = selection.planet.map_or("none".to_string(), |p| names.get(p).to_string());
    let explorer_sel = selection.explorer.map_or("none".to_string(), |e| explorer_display_name(e).to_string());
    **text = format!(
        "Planets alive: {}/7 | Phase: {personality} ({:.0}%) | Cycle: {}\n\
         Selected planet: {planet_sel} | Selected explorer: {explorer_sel}\n\
         (Cosmic Chronicle coming online...)",
        state.alive.len(),
        state.hostility * 100.0,
        format_mmss(state.phase_elapsed),
    );
}

/// Builds the docked command-center UI: top bar (phase/cycle/pause), left
/// panel (expeditions), transparent center spacer (galaxy shows through), and
/// right panel (galaxy overview + the Solace/Eclipse portraits). Skeleton
/// pass: static structure and layout, content wired to real state where
/// cheap, no holographic styling yet.
/// Builds the spaceship cockpit UI: the whole shell comes from `cockpit.png`
/// (window frame, top/bottom screens, buttons, chronicle screen), with the
/// galaxy showing through the transparent viewport hole. Portraits sit in
/// the upper corners of that viewport; Start/Pause/Resume/Sunray/Asteroid are
/// real cockpit buttons. Move-explorer and the holograms are not wired yet
/// (holograms need their own open/close system — next pass).
fn spawn_cockpit_ui(
    commands: &mut Commands,
    asset_server: &AssetServer,
    layouts: &mut Assets<TextureAtlasLayout>,
    tags: &AnimTags,
) {
    let solace_layout = layouts.add(TextureAtlasLayout::from_grid(UVec2::new(128, 128), 37, 1, None, None));
    let eclipse_layout = layouts.add(TextureAtlasLayout::from_grid(UVec2::new(128, 128), 36, 1, None, None));
    // Both stance sheets are a plain 4-frame idle loop (128x128 per frame,
    // confirmed against solace_stance.json/eclipse_stance.json's own "stance"
    // tag, frames 0-3) — one shared layout works for either.
    let stance_layout = layouts.add(TextureAtlasLayout::from_grid(UVec2::new(128, 128), 4, 1, None, None));

    commands
        .spawn(Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            position_type: PositionType::Relative,
            ..default()
        })
        .with_children(|root| {
            // ── Cockpit shell (stacked full-canvas layers, each transparent
            // except its own art) ──────────────────────────────────────────
            root.spawn(cockpit_shell_layer(asset_server, 0, 0)); // window frame + walls
            root.spawn(cockpit_shell_layer(asset_server, 1, 0)); // top screen backdrop
            root.spawn(cockpit_shell_layer(asset_server, 2, 0)); // bottom console backdrop
            root.spawn(cockpit_shell_layer(asset_server, 3, 0)); // chronicle screen backdrop
            // NOTE: no "buttons" (0,1) layer here on purpose — it's the whole
            // button row pre-rendered with all 6 always showing, which is
            // exactly what was defeating the Start/Pause/Resume show-only-one
            // logic below. The individual button layers already have complete
            // art (icon + label) on their own, so this backdrop is redundant.

            // ── Cockpit buttons ──────────────────────────────────────────────
            root.spawn((Button, cockpit_button_bundle(asset_server, &BTN_START), TransportButton::Start));
            root.spawn((Button, cockpit_button_bundle(asset_server, &BTN_PAUSE), TransportButton::Pause));
            root.spawn((Button, cockpit_button_bundle(asset_server, &BTN_RESUME), TransportButton::Resume));
            root.spawn((Button, cockpit_button_bundle(asset_server, &BTN_SUNRAY), SunrayButtonTag));
            root.spawn((Button, cockpit_button_bundle(asset_server, &BTN_ASTEROID), AsteroidButtonTag));
            // Move-explorer button: art is wired, click handling is not yet —
            // moving still works via the existing click-explorer-then-click-
            // neighbor flow. Left as a Button so a future pass can hook it up.
            root.spawn((Button, cockpit_button_bundle(asset_server, &BTN_MOVE)));

            // ── Top screen readout ───────────────────────────────────────────
            // Font sizes intentionally NOT scaled by DISPLAY_SCALE — kept at
            // their original readable px sizes even though the chrome around
            // them shrank, since legibility during a presentation matters
            // more than strict proportion-matching.
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
                    TextFont { font: FontSource::Handle(asset_server.load(FONT_REGULAR)), font_size: FontSize::Px(16.0), ..default() },
                    TextColor(SOLACE_GOLD),
                    TopBarPersonalityText,
                ));
                bar.spawn((
                    Text::new("Cycle 00:00"),
                    TextFont { font: FontSource::Handle(asset_server.load(FONT_REGULAR)), font_size: FontSize::Px(15.0), ..default() },
                    TextColor(Color::srgba(0.85, 0.95, 1.0, 0.9)),
                    TopBarCycleText,
                ));
            });

            // ── Chronicle screen placeholder (real log comes later) ──────────
            // The panel is chamfered (trapezoidal), not a clean rectangle —
            // confirmed by sampling cockpit.png directly: at local x=345 only
            // a thin sliver near y=860-870 is opaque, while at x=400 the full
            // y=760-880 band is opaque. These bounds (430-1200, 760-860) stay
            // inside the reliably-opaque interior at every x, clear of the
            // frame and the angled corners.
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
                TextFont { font: FontSource::Handle(asset_server.load(FONT_REGULAR)), font_size: FontSize::Px(12.0), ..default() },
                TextColor(Color::srgba(0.75, 0.9, 1.0, 0.85)),
                ChronicleScreenText,
            ));

            // ── Portrait bubbles — upper corners of the viewport ─────────────
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

            // ── Stances — the shared body's single standing presence. Only
            // one is ever visible (see sync_stance_visibility). Flanking the
            // ring itself (not the far viewport wall), close to the galaxy —
            // a godlike figure standing watch, not another corner readout
            // like the portrait bubbles above.
            //
            // Ring math in UI px: world (0,0) is screen center and world y is
            // up, so world_x -> WINDOW_WIDTH/2 + world_x and world_y ->
            // WINDOW_HEIGHT/2 - world_y. Planet sprites are 96*DISPLAY_SCALE
            // wide (48 half-width), so the ring's real visual edge sits
            // `RING_RADIUS_X + 48*DISPLAY_SCALE` out from center — the stance
            // sits a further ~48px gap beyond that.
            let stance_ring_half_width = RING_RADIUS_X + 48.0 * DISPLAY_SCALE;
            let stance_gap = 48.0 * DISPLAY_SCALE;
            let stance_ring_center_ui = Vec2::new(WINDOW_WIDTH / 2.0 + RING_CENTER.x, WINDOW_HEIGHT / 2.0 - RING_CENTER.y);
            let stance_top_y = stance_ring_center_ui.y - STANCE_SIZE / 2.0;
            for (top_left, image, personality) in [
                (Vec2::new(stance_ring_center_ui.x - stance_ring_half_width - stance_gap - STANCE_SIZE, stance_top_y), "solace_stance.png", Personality::Solace),
                (Vec2::new(stance_ring_center_ui.x + stance_ring_half_width + stance_gap, stance_top_y), "eclipse_stance.png", Personality::Eclipse),
            ] {
                let stance_range = tags.stance(&personality, "stance");
                root.spawn((
                    ImageNode {
                        image: asset_server.load(image),
                        texture_atlas: Some(TextureAtlas { layout: stance_layout.clone(), index: stance_range.0 }),
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

            // ── Holograms — right-click an explorer/planet to open, right-click
            // again (or empty space) to close. Left = explorer bag contents,
            // right = planet energy cell / rocket state.
            let holo_left_w = (HOLOGRAM_LEFT.local.max.x - HOLOGRAM_LEFT.local.min.x) * DISPLAY_SCALE;
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
                    TextFont { font: FontSource::Handle(asset_server.load(FONT_REGULAR)), font_size: FontSize::Px(13.0), ..default() },
                    TextColor(Color::srgba(0.6, 0.9, 1.0, 0.95)),
                    ExplorerHologramText,
                ));
            });

            let holo_right_w = (HOLOGRAM_RIGHT.local.max.x - HOLOGRAM_RIGHT.local.min.x) * DISPLAY_SCALE;
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
                    TextFont { font: FontSource::Handle(asset_server.load(FONT_REGULAR)), font_size: FontSize::Px(13.0), ..default() },
                    TextColor(Color::srgba(1.0, 0.85, 0.6, 0.95)),
                    PlanetHologramText,
                ));
            });

            // ── Map hologram — compass button + topology overview panel ──────
            spawn_map_hologram(root, asset_server);

            // ── Suspicion meters — top-aligned with the portrait bubbles
            // (same top y), 30px gap from the bubble frame, on the inner
            // side toward the galaxy (not the cockpit wall). Top position is
            // fixed; the bar's own bottom edge is what moves when its size
            // changes.
            let suspicion_meter_top = 150.0 * DISPLAY_SCALE;
            spawn_suspicion_meter(
                root,
                asset_server,
                Vec2::new(180.0 * DISPLAY_SCALE + PORTRAIT_BUBBLE_SIZE + 30.0 * DISPLAY_SCALE, suspicion_meter_top),
                SOLACE_GOLD,
                Personality::Solace,
            );
            spawn_suspicion_meter(
                root,
                asset_server,
                Vec2::new(1292.0 * DISPLAY_SCALE - SUSPICION_METER_W - 30.0 * DISPLAY_SCALE, suspicion_meter_top),
                ECLIPSE_PURPLE,
                Personality::Eclipse,
            );
        });
}

/// Spawns one portrait bubble (frame + animated portrait + glitch overlay +
/// name label) at `top_left`, sized [`PORTRAIT_BUBBLE_SIZE`].
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
                    top: Val::Px((PORTRAIT_BUBBLE_SIZE - PORTRAIT_BUBBLE_SIZE * 48.0 / 192.0) / 2.0),
                    ..default()
                },
                PortraitTag(personality),
                GlitchOverlay::default(),
            ));
        });
        col.spawn((
            Text::new(label),
            TextFont { font: FontSource::Handle(asset_server.load(FONT_REGULAR)), font_size: FontSize::Px(13.0), ..default() },
            TextColor(tint),
        ));
    });
}

/// Spawns one personality's suspicion meter: a static frame image plus a
/// bottom-anchored colored fill bar whose height `sync_suspicion_meter`
/// keeps in sync with the real, live `SuspicionState` value.
fn spawn_suspicion_meter(root: &mut ChildSpawnerCommands, asset_server: &AssetServer, top_left: Vec2, tint: Color, personality: Personality) {
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
            ImageNode { image: asset_server.load("ui_meter_suspicion.png"), ..default() },
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

/// Spawns the compass toggle button (fixed top-right corner) and the map
/// hologram panel it opens: a miniature of the real galaxy topology, built
/// from the same `CONNECTIONS`/ring layout the main view uses. Node and edge
/// visibility are driven by real `GalaxyState` (alive planets), not faked.
fn spawn_map_hologram(root: &mut ChildSpawnerCommands, asset_server: &AssetServer) {
    root.spawn((
        Button,
        ImageNode { image: asset_server.load("ui_icon_compass.png"), ..default() },
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
        ImageNode { image: asset_server.load("ui_panel_map.png"), ..default() },
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
            ImageNode { image: asset_server.load("ui_clock_phase.png"), ..default() },
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
            TextFont { font: FontSource::Handle(asset_server.load(FONT_REGULAR)), font_size: FontSize::Px(14.0), ..default() },
            TextColor(Color::srgba(0.75, 0.9, 1.0, 0.9)),
            MapClockText,
        ));

        // Connection lines first so the node icons draw on top of them.
        for (i, &(a, b)) in CONNECTIONS.iter().enumerate() {
            let pa = map_node_local_pos(a);
            let pb = map_node_local_pos(b);
            let mid = (pa + pb) / 2.0;
            let delta = pb - pa;
            let length = delta.length();
            let angle = delta.y.atan2(delta.x);
            panel.spawn((
                ImageNode { image: asset_server.load("map_line_dash.png"), ..default() },
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
                ImageNode { image: asset_server.load("map_node_known.png"), ..default() },
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

// ── Scene setup ───────────────────────────────────────────────────────────────

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

    // Space background
    commands.spawn((
        Sprite {
            image: asset_server.load("space_bg.png"),
            custom_size: Some(Vec2::new(WINDOW_WIDTH, WINDOW_HEIGHT)),
            ..default()
        },
        Transform::from_xyz(0.0, 0.0, -10.0),
    ));

    // Atmospheric tint layer — 1x1 white pixel scaled to fullscreen.
    // sync_vignette lerps its color between amber (SOLACE) and dark purple (ECLIPSE).
    let white_px = images.add(Image::new_fill(
        Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
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

    // Stars — 150 instances, deterministic scatter
    let star_tex = asset_server.load("star.png");
    for i in 0..150_u32 {
        let x = ((i.wrapping_mul(7919).wrapping_add(13)) % WINDOW_WIDTH as u32) as f32 - WINDOW_WIDTH / 2.0;
        let y = ((i.wrapping_mul(6271).wrapping_add(7)) % WINDOW_HEIGHT as u32) as f32 - WINDOW_HEIGHT / 2.0;
        commands.spawn((
            Sprite {
                image: star_tex.clone(),
                custom_size: Some(Vec2::new(2.0, 2.0)),
                ..default()
            },
            Transform::from_xyz(x, y, -9.0),
        ));
    }

    // Nebula clouds — one warm variant tied to Solace, one purple tied to
    // Eclipse. Both are always present; sync_nebula_personality crossfades
    // between them based on which personality is active. Each drifts slowly
    // via DriftBob so the "static illustration" still feels alive.
    for (file, personality, base) in [
        ("orange_nebula.png", Personality::Solace, Vec2::new(-150.0, 60.0) * DISPLAY_SCALE),
        ("purple_nebula.png", Personality::Eclipse, Vec2::new(150.0, -40.0) * DISPLAY_SCALE),
    ] {
        let phase = if personality == Personality::Solace { 0.0 } else { 2.4 };
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

    // A third nebula, not tied to either personality — always present at a
    // low, constant alpha, drifting independently in the background.
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

    // A fourth nebula variant, same treatment as pastel — always present,
    // independent of personality, placed at a different corner so it doesn't
    // overlap the pastel one.
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

    // Big flashing stars — a handful of bright twinkle accents, distinct from
    // the plain star field above.
    let big_star_tex = asset_server.load("big_star.png");
    for i in 0..12_u32 {
        let x = ((i.wrapping_mul(5237).wrapping_add(101)) % WINDOW_WIDTH as u32) as f32 - WINDOW_WIDTH / 2.0;
        let y = ((i.wrapping_mul(4111).wrapping_add(53)) % WINDOW_HEIGHT as u32) as f32 - WINDOW_HEIGHT / 2.0;
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

    // Custom cursor — OS cursor hidden, this small sprite follows the mouse.
    if let Ok(mut cursor_options) = primary_cursor.single_mut() {
        cursor_options.visible = false;
    }
    commands.spawn((
        ImageNode {
            image: asset_server.load("cursor_star.png"),
            texture_atlas: Some(TextureAtlas {
                layout: layouts.add(TextureAtlasLayout::from_grid(UVec2::new(16, 16), 2, 1, None, None)),
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

    // Planets — 28 frames, 48x48, idle 0-7 at 10fps
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

    // Portraits now live as small ImageNode entries docked in the bottom bar —
    // see spawn_command_ui. Their TextureAtlasLayout handles are built there.

    // JEB — id 2, 20 frames, 48x48, starts on planet 1
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

    // VIVIANA — id 1, 24 frames, 48x48, starts on planet 4
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

    spawn_cockpit_ui(&mut commands, &asset_server, &mut layouts, &tags);

    if let Some(tx) = ready.0.take() {
        let _ = tx.send(());
    }
}
