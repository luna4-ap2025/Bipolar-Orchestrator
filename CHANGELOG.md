# Changelog

## Unreleased

### Added
- Explorer death on planet destruction: an explorer stationed on a planet when it's destroyed by an undeflected asteroid is now killed immediately (`OrchestratorToExplorer::KillExplorer`) instead of being silently left with a stale `current_planet`.
- Game end condition (spec §1.3): the logic loop now stops once every explorer has died, checked every tick right after planet dispatch.
- `OrchestratorEvent::ExplorerKilled` / `GalaxyEvent::ExplorerKilled` so the GUI can react to an explorer's death.
- `ExplorerRegistry::is_empty()`.
- 7th probability curve, `CurveKind::Staircase` (4 discrete steps down per period, then reset), needed so all 7 planets get a distinct curve instead of two sharing one.
- `main.rs` (CLI) now spawns both explorers (Viviana on planet 4, Jeb on planet 1), matching `builder.rs` (GUI). Previously only Viviana was spawned, so CLI commands referencing explorer 2 always failed with "not found".

### Changed
- `HOSTILITY_PER_SEC`: `1.0/60.0` -> `1.0/240.0` (60s -> 240s ramp to max hostility) — the galaxy was reaching permanent max danger after only 1 minute, causing games to end in ~3 minutes.
- `curves::PERIOD`: `40.0` -> `160.0`, scaled 4x alongside the hostility ramp to keep the same ratio between one planet's wobble cycle and the galaxy's overall danger ramp.

### Removed
- Stranded-explorer relocation-to-neighbor logic in `logic::tick::destroy_planet` (superseded by immediate death, per team decision).
