# Bipolar Orchestrator: Galaxy Visualizer

**Author:** Valentina Buitron ([@TheFIXmess](https://github.com/TheFIXmess))
**Made for:** the *Bipolar Orchestrator*, by Valentina Buitron ([@TheFIXmess](https://github.com/TheFIXmess)) and Viviana Fraccaroli ([@vivis2kuni](https://github.com/vivis2kuni))
**Course:** Advanced Programming, A.Y. 2025/2026: Galaxy visualizer (individual contribution)

This folder contains the graphical interface I designed, drew and programmed for our orchestrator: a Bevy application that shows the galaxy as seen from a spaceship cockpit, with every piece of art drawn by hand in Aseprite.

It is shared so that others can study it, learn from it, or take inspiration from it. Please read [Compatibility](#compatibility-read-this-first) and [Usage and credit](#usage-and-credit) before using it.

---

## Contents

1. [Compatibility (read this first)](#compatibility-read-this-first)
2. [Concept and lore](#concept-and-lore)
3. [What the visualizer shows](#what-the-visualizer-shows)
4. [How it works](#how-it-works)
5. [Controls](#controls)
6. [Assets and the art process](#assets-and-the-art-process)
7. [Adapting it to another orchestrator](#adapting-it-to-another-orchestrator)
8. [Folder structure](#folder-structure)
9. [Usage and credit](#usage-and-credit)

---

## Compatibility (read this first)

**This visualizer was made specifically for the Bipolar Orchestrator by Viviana Fraccaroli and me.** It is not a general-purpose visualizer, and it will not work with a different orchestrator as it is.

- It **does not compile on its own**: `Cargo.toml` depends on the `bipolar-orchestrator` crate, which is not included.
- It **calls our orchestrator directly**, both to build the galaxy (`builder::build_api`) and to read its state (`snapshot::build`), and it expects our API functions.
- Much of the design depends on **mechanics that only our orchestrator has**: the Solace/Eclipse personalities, the hostility value, the cycle timer and the suspicion system. Without them, the portraits, stances, nebulas, top screen and suspicion meters have nothing to show.
- The **explorers are hardcoded**: Viviana is id 1 (starting on planet 4) and Jebediah is id 2 (starting on planet 1), each with their own sprites and animations.
- The **galaxy layout is fixed** to 7 planets in the ring described in `galaxy.txt`, in the order given in `planets.toml`.
- The **screen layout is fixed** for 1920x1080.

Making it work with another orchestrator would take **significant rewiring**. I'm sharing it as it is. I can't take responsibility for adapting it, for making it work in another project, or for any problems that come from modifying it. If you want to try anyway, see [Adapting it to another orchestrator](#adapting-it-to-another-orchestrator).

---

## Concept and lore

The idea behind our orchestrator is that the galaxy is ruled by a single entity with **two personalities sharing one body**:

- **Solace** (gold): the nurturing side. She prefers sending **sunrays**, which charge the planets' energy cells.
- **Eclipse** (purple): the destructive side. She prefers sending **asteroids**, which can destroy planets that have no rocket.

Which one is in control depends on a global **hostility** value between 0 and 1. It slowly rises over time, and at 0.5 control flips from Solace to Eclipse. When a planet is destroyed, hostility drops back to 0, so the galaxy gets a calm period before the danger builds again. That's the "bipolar" in the name.

The visualizer is designed around this story:

- **You are inside the cockpit**, looking at the galaxy through the ship's window. The console gives you manual controls.
- **Both portraits are always visible** in the upper corners, but the one in control glows brighter. The two crossfade gradually as hostility changes, instead of switching all at once.
- **Only one full-body figure (stance) stands next to the galaxy at a time**, because Solace and Eclipse share one body. She can only be physically present as one of them.
- **Manual overrides are an infiltration.** When the player forces a sunray or an asteroid, it's like a parasite hacking into her decisions: her portrait glitches, and she reacts. If you force the action she likes, she only gets *confused*. If you keep forcing the one she dislikes, she becomes *suspicious*, and the third time she **refuses** and the request is never sent. Each personality has a suspicion meter next to her portrait.
- The two explorers, **Viviana** and **Jebediah**, walk between planets, carry the resources they collect (shown as icons above their heads), and notice each other when they land on the same planet.

---

## What the visualizer shows

- **The galaxy:** 7 animated planets on an elliptical ring, with the connections between them. Destroyed planets play a death animation and disappear, along with their connections.
- **Sunrays and asteroids** flying from the sending personality's stance to the target planet. The planet then plays its "receive" or "hit" animation.
- **Explorers** moving hop by hop. Every move reported by the orchestrator is animated in order, even when several happen quickly.
- **Top screen:** current personality and hostility percentage, cycle timer, and whether sunray or asteroid odds are rising.
- **Holograms (right-click):** an explorer's bag contents or a planet's energy cells and rocket.
- **Map hologram (compass button):** a small map of the alive planets and their connections.
- **Bottom screen (Chronicle):** a summary of the galaxy and the current selection. A full event log is planned but not implemented yet.
- **Game over** screen when all explorers have died.
- **Atmosphere:** starfield, twinkling stars, drifting nebulas that follow the active personality, occasional shooting stars, a custom star cursor, and a subtle color tint.

---

## How it works

The visualizer runs as part of the same program as our orchestrator, with two threads:

```
┌──────────────────────────────┐        GalaxySnapshot         ┌───────────────────────────┐
│  Bridge thread               │  ── every 250 ms ──────────▶  │  Bevy app (main thread)   │
│  owns the OrchestratorApi    │                               │  draws everything,        │
│  - builds the galaxy         │  ◀──────── UiCommand ───────  │  handles mouse/keyboard   │
│  - runs the commands         │   (button clicks, inspects)   │                           │
└──────────────────────────────┘                               └───────────────────────────┘
```

**1. Startup.** `main()` starts the *bridge thread* and the Bevy app. The bridge waits until the window is on screen, then builds the galaxy with our orchestrator's `builder::build_api(galaxy.txt, planets.toml)`. The game logic does not start until the player presses **Start**.

**2. Reading the state.** Every 250 ms the bridge calls our orchestrator's `snapshot::build(&api)` and stores the result, a `GalaxySnapshot` (defined in `shared/src/lib.rs`):

| Field | Meaning |
|---|---|
| `personality` | `Solace` or `Eclipse` |
| `hostility` | 0.0 – 1.0 |
| `phase_elapsed` | seconds in the current cycle |
| `alive_planets` | ids of the planets still alive |
| `neighbors` | current connections (they change when planets die) |
| `explorers` | each explorer's id, planet and bag |
| `events` | what happened since the last snapshot: `SunraySent`, `SunrayReceived`, `AsteroidSent`, `AsteroidDeflected`, `PlanetDestroyed`, `ExplorerKilled`, `ExplorerMoved` |

On the Bevy side, `poll_snapshot` copies this into a `GalaxyState` resource and turns each event into animations (portrait reactions, projectiles, planet reactions, explorer hops). All the other systems only read `GalaxyState`.

**3. Sending commands.** The orchestrator's API calls block while they wait for acknowledgements, so calling them from Bevy would freeze the window. Instead, the UI sends a `UiCommand` through a channel, and the bridge thread runs it in `drain_ui_commands`:

| UiCommand | Orchestrator call |
|---|---|
| `Sunray(planet)` | `send_sunray` |
| `Asteroid(planet)` | `send_asteroid` |
| `MoveExplorer { explorer_id, dst }` | `move_explorer` |
| `ToggleLogic` | `start_logic` / `stop_logic` |
| `InspectExplorer(id)` | `bag_content` |
| `InspectPlanet(id)` | `planet_state` |
| `DebugNudgeHostility(delta)` | `debug_nudge_hostility` |

**4. Animations.** Frame ranges are never hardcoded. At startup, `AnimTags::load` reads the tag names (`idle`, `hit`, `breaking`, …) from the Aseprite `.json` exports, so re-exporting a sprite with a different number of frames doesn't break anything. A misspelled tag makes the program stop at startup, on purpose.

---

## Controls

| Input | Action |
|---|---|
| **Start / Pause / Resume** button, or **Space** | start or pause the orchestrator logic |
| Left-click an explorer, then a highlighted (green) planet | move the explorer there |
| Left-click a planet (no explorer selected) | select it as a target |
| **Sunray** / **Asteroid** buttons | fire at the selected planet (subject to her suspicion) |
| Right-click an explorer or planet | open its hologram; right-click again or on empty space to close |
| **Compass** button (top right) | open/close the map hologram |
| **E** / **S** | debug: push hostility toward Eclipse / Solace |

The window opens in borderless fullscreen and is laid out for a **1920x1080** screen (a 1600x900 design scaled by `DISPLAY_SCALE = 1.2`).

---

## Assets and the art process

**All the pixel art in `assets/` was drawn by me in Aseprite**, frame by frame. I used reference images for ideas and inspiration, but every sprite, animation and UI element was drawn by hand. This was the biggest and most time-consuming part of the project.

How the assets were made:

- Each character, planet and effect is its own Aseprite file, animated with **named tags** and exported as a horizontal sprite sheet (`.png`) plus a data file (`.json`) that the code reads.
- The **cockpit** is one large Aseprite file where each part (window, top screen, bottom console, chronicle screen, each button, each hologram) is its own layer. It's exported with *Split Layers* into `cockpit.png`, and the code crops each part out by its exact pixel position.
- Layers like `glow`, `details` and `filter` were used on the characters to get a soft lighting look while staying in pixel art.

What's included:

| Asset | Frames | Animations |
|---|---|---|
| `solace.png` / `eclipse.png`, the portraits | 37 / 36 | idle, speaking, sending, satisfied, unwanted_event, confused, suspicious, refusal, breaking, awakening |
| `solace_stance.png` / `eclipse_stance.png`, the full-body figures | 4 each | stance |
| `viviana.png` | 24 | idle, moving, arrived, departing, collecting, react_other |
| `jeb.png` | 20 | idle, moving, arrived, departing, speaking, react_other |
| `planet_*.png` (Orbitron, SkyCartel, Rustrelli, The Compiler Strikes Back, Crabtorio, Houston We Have A Borrow, Enterprise) | 28 each | idle, hit, receive, dying, destroyed |
| `sunray.png` / `asteroid.png` | 6 / 9 | moving, impact (+ deflected) |
| `cockpit.png` | 13 layers | cockpit frame, screens, buttons, holograms |
| Resources (`resource_*.png`) | | oxygen, hydrogen, carbon, silicon, diamond, water, life, robot, dolphin |
| Effects and UI | | parasite glitch, portrait frame, suspicion meter, compass, map panel and nodes, clock, cursor |
| Background | | space background, stars, big stars, shooting star, four nebulas |

Some assets in the folder (for example the map stamps, dialog boxes, and the flip and overcharge effects) were made for features that are **not implemented yet**.

**Font:** the UI text uses **Pixeloid Sans** by GGBotNet, a free font under the SIL Open Font License. It's not my work, and its license applies to it.

---

## Adapting it to another orchestrator

If you still want to try, these are the places where it connects to our orchestrator. All of them are in `src/main.rs`:

1. **`main()`, the bridge thread:** replace `bipolar_orchestrator::builder::build_api(...)` with your own setup, and `bipolar_orchestrator::snapshot::build(&api)` with code that produces a `GalaxySnapshot` from your orchestrator's state.
2. **`drain_ui_commands`:** point each `UiCommand` at your own API functions.
3. **`PlanetNames::load`:** it uses our `galaxy::planet_config::parse_str` to read `planets.toml`.
4. **`Cargo.toml`:** remove the `bipolar-orchestrator` dependency and add yours.

After that, you would still need to decide what to do with the personality, hostility and suspicion features, the hardcoded explorers, and the fixed layout (see [Compatibility](#compatibility-read-this-first)).

---

## Folder structure

```
gui/
├── Cargo.toml        dependencies (Bevy 0.19, our orchestrator, shared types)
├── README.md         this file
├── src/main.rs       the whole visualizer (one file, organized in sections)
└── assets/           sprite sheets (.png), animation data (.json) and fonts

shared/src/lib.rs     GalaxySnapshot and the other types sent to the GUI
galaxy.txt            galaxy topology, embedded at compile time
planets.toml          which planet is at each id, embedded at compile time
```

---

## Usage and credit

This visualizer and **all of its artwork are my own individual work**, made with a lot of time and care. Please treat them with respect:

- **Credit me** (Valentina Buitron, [@TheFIXmess](https://github.com/TheFIXmess)) whenever you show, present or reuse any part of this visualizer or its art.
- **Don't present it as your own work**, in whole or in part.
- **Don't reuse the artwork outside this project**, or modify and redistribute it, without asking me first.

Thank you for respecting the work that went into it. I hope it's useful, even if only as a reference or inspiration. 💛

— Vale
