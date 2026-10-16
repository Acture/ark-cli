# Battle runtime

`ark-battle-runtime` runs the installed Arknights Windows client's original battle implementation
inside an owned Unity 2021.3.39f1 Player. It reads the official installation and copies inputs into
ignored local state. Every battle runs in a fresh owned Player process.

The runtime is a Rust library used by [`ark-cli`](../ark_cli/README.md).
[`ark-unity`](../ark_unity/README.md) handles Unity resources,
Player builds and Windows process compatibility. [`ark-battle-runtime-probe`](../battle_runtime_probe/)
provides sources for a separate Cargo `no_std` DLL preloaded through `UnityPluginLoad`. Its
no-CRT linker and panic profile belong to the isolated template project. The portable source
provider is a normal workspace dependency. `unity/` contains the Unity project template and the
C# Editor build entry point required by Unity's build pipeline. No Python interpreter is used
by these packages.

Probe sources and the Unity project template are embedded when the runtime library is compiled; the
`bundled-tools` feature also embeds the third-party tools and a prebuilt probe. The CLI
materializes and verifies those exact bytes under owned state; execution does not read the source
checkout or the original build machine's paths.

## Requirements

- Windows x64 and an installed, updated official PC client. Its files are read in place and never
  distributed.
- Nothing else for a Windows release of `ark-cli`: it embeds the third-party metadata tools and the
  prebuilt probe, and `setup` downloads the prebuilt owned Player (see below).
- Other builds (for example `cargo install`) download the pinned third-party wheels at `setup` and
  compile the probe, which needs Rust 1.89+, the `x86_64-pc-windows-msvc` target and a Windows SDK.
- Maintainers building the owned Player themselves need Unity 2021.3.39f1 with Windows IL2CPP
  support, an active Unity license and MSVC x64 build tools (`ARK_UNITY_CLI` if the Unity CLI is
  outside `~/.unity/bin` and `PATH`).

## Usage

```powershell
ark-cli battle setup
ark-cli battle run path/to/job.json
```

`setup` prepares everything and can be rerun at any time; finished steps are reused:

1. Find the client: `--client`, `ARK_PC_CLIENT`, the client a previous `setup` recorded, or the
   single Hypergryph Launcher default location (`<drive>:/[Program Files/]Hypergryph Launcher/games/Arknights`).
2. Place TypeTreeGeneratorAPI 0.0.10, Capstone and UnityPy 1.25.4's `lzma.tpk` under
   `STATE/tools` with pinned hashes: embedded in releases, otherwise downloaded from PyPI wheels.
   Their licenses are in `licenses/THIRD_PARTY.md`, which setup copies beside them.
3. Place the probe: the release's prebuilt DLL when it matches the embedded probe sources,
   otherwise compiled locally.
4. Install the owned Player: the pinned GitHub Release archive, `--player-zip` for a local copy of
   it, or `--build-player` to build it with Unity (maintainers).
5. Pin the client's runtime files, check that the original runtime loads, and export the original
   CharacterDB, StageDB, UniEquipDB and FavorDB.

Rerun `setup` after a client update; execution refuses changed pinned runtime files. State
defaults to `assets/battle-runtime/` under the working directory, or `--state` /
`ARK_BATTLE_RUNTIME_STATE`.

`run` resolves the job's stage from the original StageDB. Without `--squad` it generates one from
the original tables at the training options below (default: the original maximum) and records it
in the report. A squad file supplies identity and training absent from MAA jobs; see `examples/`.
Phase, potential rank and skill index are zero-based original values; operators without skills use
skill index -1. `favor_battle_phase` is the original trust battle phase, 0..100 with 50 at 100%
trust. `squad-for` writes the generated squad for inspection or editing:

```powershell
ark-cli battle run path/to/job.json --level 60 --mastery 1 --module 2 --trust 50 --repeat
ark-cli battle run path/to/job.json --squad my-squad.json
ark-cli battle squad path/to/job.json my-squad.json --mastery 2
```

| Option | Values | Meaning |
| --- | --- | --- |
| `--level` | `max`, 1..90 | Level at the highest elite phase, capped by the original phase maximum |
| `--mastery` | 0..3 | Skill level 7 plus this specialization rank (elite 2 only) |
| `--module` | 0..3 | Module level; the job's required type (χ γ α Δ β), else the first module |
| `--trust` | `max`, 0..200 | Trust percent through the original favor frames: its battle phase (50 at 100%) for stats, its favor point for module unlocks |

Job requirements stay floors: a lower option never generates training below what the job asks
for, and a required module type that cannot unlock at the chosen training is an error. A module
level also needs the original unlock trust, so low trust lowers it. Potential is the maximum.

`min-training` runs the job at maximum training, then lowers the whole team's module 3→0, mastery 3→0,
level max→60→30→1 and trust max→100→50→0 one axis at a time, keeping each tier the original engine
still wins. `--search bisect` treats the axes by kind. Mastery and module change skill mechanics
and timing, and a job condition such as "cast when ready" can then make a lower value win where a
higher one loses, so every pair is tried, lowest first, keeping the first that wins. Level and
trust only scale stats and trade against each other, so `stat_tradeoffs` reports each point
where lowering either loses: trust 0 with the lowest level, level 1 with the lowest trust, and a
balanced point from bisecting both together in proportional steps, then each alone (level 1..max,
trust 0..200%). Every bisection tries the floor first, and the search continues from the balanced
point. That assumes higher stats never lose where lower stats win; where a job breaks that, the
result passes without being the lowest.
`--per-operator` continues from the team result and lowers each operator alone, in squad order,
for per-operator minimums. Both searches are greedy across axes and operators, so their order can
matter. Every tier is an original-engine run, and a squad that two trainings generate runs once.
A tier passes when the original engine wins with no enemy reaching an exit; `--allow-leaks` also
passes wins that leak. `ladder.json` lists every attempt with its remaining life points and leaks,
its concrete job and the lowest passing training:

```powershell
ark-cli battle min-training path/to/job.json --search bisect --per-operator
```

### Battle programs

`run`, `min-training`, `latency` and `tune-skills` accept an MAA job or a battle program
(`"schema": "ark-battle-program-v1"`). A program keeps an MAA job's `stage_name`, `opers` and
`groups`; its `steps` are operations behind conditions met in order:

```json
{"schema": "ark-battle-program-v1", "stage_name": "main_02-10",
 "opers": [{"name": "维什戴尔", "skill": 3}],
 "steps": [
  {"do": "deploy", "name": "维什戴尔", "location": [3, 5], "direction": "Right"},
  {"when": [{"kills": 4}, {"wait": 30}], "do": "cast", "name": "维什戴尔", "location": [3, 5]},
  {"do": "end"}]}
```

| Field | Values |
| --- | --- |
| `when` | `kills`, `cost` (at least), `cost_change` (since the step became current), `cooling` (exactly), `tick` (at least), `wait` (ticks after the previous condition) |
| `do` | `deploy`, `retreat`, `press` (MAA's Skill), `cast`, `suspend`, `speed`, `policy` (MAA's SkillUsage), `end` (MAA's SkillDaemon) |
| `delay` | Ticks the operation must stay ready, after the conditions, before it is issued |
| other | `name`, `location`, `direction`, `skill_usage`, `skill_times` as in MAA actions |

`battle convert INPUT OUTPUT` converts by the input's kind: an MAA job becomes a program, and a
program becomes an MAA job when MAA can express it, otherwise every reason it cannot is listed
(tick and wait conditions, a step delay, cast-only and suspend-only operations, a condition twice,
or conditions out of MAA's order). A won program's evidence is `concrete-program.json`.

### Latency and skill timing

Results assume ideal timing. `--delay N` gives every unit operation, and every skill policy, a
declared latency such as a screen-recognition client's: once its conditions are met, the
operation must stay ready N ticks (30 per second at speed 1) before it is issued, restarting
whenever it is not ready. A step's own `delay` takes precedence.
`battle latency JOB` bisects the largest delay, up to `--max-delay` ticks, at which the job still
wins without a leak, so a result can be stated with the latency it tolerates.

`battle tune-skills JOB` tries each skill step (press or cast) alone with each of `--waits` ticks as its
`delay`, records which still win without a leak, picks the best (most life left, then
earliest finish, then smallest wait), and verifies all best waits together in
`advice/advised.program.json`. Each run's `step_ticks` gives the tick a step was issued at; with
the kill counts in its events, a wait can be rewritten as an MAA kills condition.

Maintainers publish the owned Player and the probe bundle with hidden commands:
`battle package-player OUTPUT.zip` packages the verified local Player with its complete inventory;
`battle build-probe DIR` writes the probe DLL and its source digest for a `bundled-tools` build.

## Execution and evidence

The catalog verifies current resource-list hashes and prefers installed hot updates. Rust extracts
the original level, asks the original decoder to decode it, prepares its scene CAB dependency
closure, and routes original services, tables and engine settings through the owned Resources map.
Serialized object payloads must survive byte-identical roundtrips and independent reparsing.

Battle runs load the client's Lua code hotfixes by default, as the client does: the original
`LuaManager` creates the xLua environment (the client's own `xlua.dll`), and the encrypted entry
and hotfix scripts load through the original decrypter from the current, hot-update-first bundles.
Registration is verified on the original `Scheduler` hotfix bridge before any battle code runs.
`run --no-lua-hotfix` skips them, to compare their effect on a battle.

The probe calls original LoadGame, StartGame and ManualFrameTick and runs a battle program,
evaluated every tick on observed original state:

- **Steps**, in order: an operation behind conditions met in order (a met condition is not checked
  again). Conditions: kills at least, cost at least, cost change since the step became current,
  exactly N cards cooling, tick at least, and ticks waited since the previous condition.
- **Operations**: deploy (card ready and affordable), retreat (manually withdrawable), press a
  skill button (`BasicSkill.IsClickable`: cast a ready skill, or suspend an active, suspendable
  one), cast only, suspend only, speed toggle, set a skill policy, wait for the battle end.
- **Policies**, every tick: cast a unit's skill whenever it is ready to start (not active and
  its button clickable), or a bounded number of times. A policy never suspends.

Readiness uses only original members without side effects. `BasicSkill.get_canCastCheck` is not
one: for skills with a target selector it runs the original target search, which caches the
target and restarts the search timer. In the original game only `CastSkill.OnTick` calls it,
once per tick, so the probe never does.

The next step starts once an operation is issued; each effect is confirmed in the background on
original state. A skill may take a few ticks to start (a begin animation or chant; Surtr's and
Wiš'adel's S3 take 17 and 24): it must start within 90 ticks, and that unit's skill is not issued
again meanwhile. Missing ObjectPool resources trigger bounded preparation rounds; exhausted or
unsupported input never becomes a success verdict.

Every tick the probe also records battle events (`original_event`): enemies entering and leaving
with their raw fixed-point hit points, life point and kill count changes, operators entering and
leaving, and skills starting and ending. These are read-only original getters; a run with and
without them is identical. `report.json` summarizes them as `battle_events`: each leak (an enemy
leaving with hit points left) with its tick and hit points, life point changes, and `perfect` when
nothing leaked.

### Presentation

`--presentation` chooses how the battle is driven (`run` and `min-training`):

| Value | Driver | Use |
| --- | --- | --- |
| `fast` (default) | The original manual ticks back to back: a deterministic game mode that allows manual ticks, no Unity frames during the battle | Searches and results: fastest and repeatable |
| `frames` | The original loop, as in the client, hidden | Checking behavior that depends on frames (UI, coroutines, Update) |
| `watch` | The original loop in a visible 1280×720 window | Watching a battle |

In `frames` and `watch` the probe keeps the original default `GameModeMeta` (in the current
client not deterministic, no manual ticks, automatic Physics2D), lets the original launcher start
the battle, and leaves ticking to the original `FixedUpdate`, which invokes
`BattleController.OnTick` through the game mode. `OnTick`'s method pointer, found by name, is
replaced before that delegate is built, so every original tick passes through a forwarder: it
issues due operations into the original `PlayerOperationQueue` (the field found by its type, the
path player input and `ManualFrameTick` use), runs the original tick through
`il2cpp_runtime_invoke` (a managed exception is recorded and the battle continues, as Unity does
for `FixedUpdate`), then confirms effects and records events. A tick that bypasses the forwarder
stops the run. Pacing follows game seconds: `--speed 1|2` sets the original speed level a battle
starts at (a job's SpeedUp toggles it), and `--time-scale 1|2|4|8|16` sets game seconds per real
second.

On 2-10 #50610 both drivers end the same: a win at tick 5774 with 36 enemies and no leak; their
operations differ by at most one tick, with one automatic skill 17 ticks apart.

`run --trace` records, per tick, every change of each deployed operator's parameterless original
skill and Character getters and predicates (`original_unit_trace`), and once per unit their method
names and parameter types, to find which original state a job depends on. It calls every such
member, and some have side effects (such as `get_canCastCheck` above): a traced battle can differ
by a few ticks, so trace only to investigate, never to establish a result.

`job_result` is `complete` when every command executed, `won_early` when the original engine won
before later commands became due (for example kill-count conditions), and `incomplete` otherwise.
A win with every issued operation confirmed establishes the job: `concrete-job.json` keeps the
actions up to the last completed command and sets each operator's requirements to the training
that ran, with the source job, squad, level and resource version in `doc`. MAA can execute it.

Each MAA attempt writes `result.json`; the final attempt also writes `report.json` and, for a
verified original terminal state, `battle-proof.json`. Control, level-decode and identity-export
runs write `result.json`.
Inputs include hashes of the entire effective Player, external resources, manifests, actions,
squad, original runtime, source
and Player executable. External execution files are checked again after the process exits.
`--repeat` starts another fresh Player and compares input hashes and normalized engine events.
`ark-cli battle compare FIRST_RESULT_JSON SECOND_RESULT_JSON` compares saved evidence without starting a client
or Player, and saves `repeatability.json` beside the second result.

Exit status is 0 when the original battle terminates naturally with every issued operation
confirmed, and either all commands executed or the engine won first, and a requested repeat
comparison passes; 1 means failed qualification and 2 means an error. A natural
`original_loss` is a valid observation. `original_win` / `original_loss` are separate from Ark
Solver's `ExpectedVictory` / `ExpectedDefeat`; the runtime does not certify official-client equivalence.

## MAA profile `original-state-v3`

MAA jobs compile to battle programs. The adapter keeps each job's intent and leaves out what MAA
needs only as a screen-recognition client: screenshot polling, click delays, the 1.5 s auto-skill
interval and retry limits. Results are therefore those of ideal timing.

| Content | Behavior |
| --- | --- |
| Deploy, Skill, Retreat | Deploy, press the skill button, retreat steps |
| Skill on an active, suspendable skill | The press suspends the skill, as MAA's click does |
| kills, costs, cost_changes, cooling | Conditions in MAA's order: cost_changes, kills, costs, cooling (exactly N cooling) |
| Summon names (for example `“弦惊”`) | Undeclared action names resolve to one original token |
| Skill/Retreat with name and location | The named unit must stand on that tile when it acts |
| SpeedUp | Original speed toggle verified |
| SkillUsage, skill_usage 0/1/2 | Manual, automatic, or bounded automatic use with skill_times |
| SkillDaemon | Wait for the natural battle end while applying skill policies |
| Output | Accept doc/doc_color presentation fields; no battle effect |
| opers, groups, requirements | Original names and explicit squad; one eligible member per group; module types match the original UniEquipDB |
| stage_name | Unique original stage code, name, stageId or levelId in the current catalog |

Nonzero delays, finite timeouts, timers, camera/bullet-time actions, challenge mode, location-only
Skill/Retreat without a name and skill_usage 3 are rejected. Presence in the catalog
alone does not guarantee that a stage's runtime dependencies can be loaded.

## Limits and validation

The owned Player uses the Editor's matching Unity engine; the official client's UnityPlayer does
not initialize in this runtime. Some presentation, story and
camera features remain unavailable and appear in runtime diagnostics. Execution starts a fresh
Player for each run, so loading dominates short battles.

```powershell
cargo test -p ark-battle-runtime -p ark-unity
cargo clippy -p ark-battle-runtime -p ark-unity --all-targets -- -D warnings
cargo fmt --all -- --check
```

Never commit client files, extracted resources, third-party tool inputs, Player binaries or local
qualification evidence.
