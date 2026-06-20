# File Ownership

Quick reference so we know who is doing what. Some files are shared but most
have a clear owner. Update this if things shift around.

---

## Vale

| File | What to do |
|------|-----------|
| `src/galaxy/parser.rs` | Done. Parse galaxy.txt into Topology. Add more edge case tests if needed. |
| `src/galaxy/topology.rs` | Done. neighbors(), remove_planet() etc. |
| `src/probability/curves.rs` | Done. All 6 curve functions. Double check the math. |
| `src/probability/bipolar.rs` | Done. Just a toggle. |
| `src/probability/mod.rs` | Done. Random curve assignment + probability lookup. |
| `src/planet/factories/mod.rs` | Wire up `all_factories()` as each factory file gets filled in. |
| `src/planet/factories/orbitron.rs` | Check Orbitron repo API, fill in `create()`. |
| `src/planet/factories/skycartel.rs` | Same as above (copy orbitron.rs as a template). |
| `src/planet/factories/rustrelli.rs` | Same. |
| `src/planet/factories/thecompilerstrikesback.rs` | Same. |
| `src/planet/factories/crabtorio.rs` | Same. |
| `src/planet/factories/houstonwehaveaborrow.rs` | Same. |
| `src/planet/factories/enterprise.rs` | Same. |
| `src/routing/move_explorer.rs` | Finish the 3-step move protocol. The main TODO is step 2 (planet->explorer sender). |
| `src/api/mod.rs` | Fill in `move_explorer()` call to routing::move_explorer::execute(). |
| `src/main.rs` | Wire `commands::parse()` into `interactive_loop()`. Call `api.*` methods. |

## Vivi

| File | What to do |
|------|-----------|
| `src/planet/handle.rs` | Looks good. Verify send/join work correctly. |
| `src/planet/spawner.rs` | Fix the `todo!()` for the planet->explorer Sender. |
| `src/explorer/handle.rs` | Looks good. Extend if needed. |
| `src/explorer/mod.rs` | Looks good. |
| `src/logic/mod.rs` | Fill in the two TODOs in `run_logic_loop()` - dispatch per planet + drain explorer messages. |
| `src/logic/tick.rs` | Connect `dispatch_to_planet()` into the loop. Mostly written already. |
| `src/logic/event_handler.rs` | Wire `TravelToPlanetRequest` to `routing::move_explorer`. |
| `src/api/mod.rs` | Fill in the `todo!()` methods (waiting for acks from explorer channel). |
| `src/api/commands.rs` | Done. |
| `src/main.rs` | Fill in init block: spawn planets, spawn explorers, build OrchestratorApi. |

## Both together

| File | What to do |
|------|-----------|
| `Cargo.toml` (orchestrator) | Uncomment planet git deps one by one as factories get filled in. |
| `galaxy.txt` | Finalize the galaxy topology (7 planets, decide connections). |
| `src/main.rs` (init block) | Parse -> spawn planets -> spawn explorers -> construct OrchestratorApi. Do this together first session. |

---

## Separate repos

**Vale - Visualizer repo**
- Bevy app with Aseprite animations
- Reads game state via `OrchestratorApi::planet_state()` and `alive_planets()`
- Shows planets, energy cell charge bars, explorer positions, bipolar mode indicator

**Vivi - Second Explorer repo**
- Depends on `common-game`
- Different AI strategy from the first explorer

---

## Integration checklist (end of week 1)

- [ ] All 7 planet factories compile and return Ok(Planet)
- [ ] spawn_planet spawns all 7 in threads without panic
- [ ] send_sunray -> SunrayAck round trip works
- [ ] send_asteroid + planet destroyed -> bipolar flip visible in logs
- [ ] Explorer can do NeighborsRequest -> TravelToPlanetRequest -> move confirmed
