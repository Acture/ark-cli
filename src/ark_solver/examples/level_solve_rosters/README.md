# Level-Solve Rosters

Roster entries select an operator and skill slot. The v1 adapter resolves each entry as
E2/max-level, trust 200, skill level 10, potential 6, and no module. Unsupported skill mechanics
remain visible as structured exclusions.

Level solve consumes an exact source MAA job file rather than a separate planning document.
The guide must materialize exactly one executable selection and contain exactly one fixed-operator
`Deploy` action; that action is the source deployment being replaced. Group-selected deployments
remain fixed. The workflow derives the stage, retained plan, source action, responsibility gaps,
and legal search domain from the guide, pinned snapshots, and authoritative retained-only replay.

```fish
set source_guide /path/to/exact-source-guide.maa.json
set gamedata_dir /path/from/rogue.toml/gamedata
set analysis_dir /path/from/rogue.toml/assets/analysis

cargo run -p arknights-cli -- level solve BD-7 \
	--snapshot $gamedata_dir \
	--prefab-snapshot $analysis_dir/battle-prefabs \
	--guide $source_guide \
	--difficulty normal \
	--roster src/ark_solver/examples/level_solve_rosters/bd7-exusiai-s2.json \
	--top-k 1 \
	--output-dir $analysis_dir/bd7-exusiai-s2

cargo run -p arknights-cli -- level validate BD-7 \
	--snapshot $gamedata_dir \
	--prefab-snapshot $analysis_dir/battle-prefabs \
	--source-guide $source_guide \
	--difficulty normal \
	--strategy $analysis_dir/bd7-exusiai-s2/solution-000/strategy.maa.json \
	--artifact $analysis_dir/bd7-exusiai-s2/solution-000/strategy.rgs \
	--output $analysis_dir/bd7-exusiai-s2/solution-000/validation.json
```

`strategy.maa.json` is the canonical executable MAA job file. `strategy.rgs` is the internal,
non-executable schema-v4 verification evidence consumed by independent validation; it is not a
second strategy format. The solve and validation reports record the exact source-guide digest,
materialized selection, selected slots, source action, and source operator.

The prefab snapshot must match the gamedata revision and include charpacks for retained and roster
operators. Its `mechanics_revision` is embedded in every report and strategy artifact. Omitting
`--roster` still scans the legacy reviewed skill registry during the script-handler migration;
unknown script IDs and custom buffs without serialized attribute semantics fail closed.

The tracked Exusiai-only result below is historical evidence from before the workspace migration,
not the current post-migration baseline. It remains a rejection regression: planning groups every enemy due by each
goal deadline, compares the group’s cumulative total HP with retained-plus-replacement output, and
uses incomplete optimistic capacities only for search ordering. In that former checkpoint, the
proposals then reached full replay and every tested Exusiai-only plan was rejected. The historical
local-window bundle is not exported as a new job.

## Main story 1-1, with a retained deployment-point granter

`tests/fixtures/maa_job_main_01_01_solve_source.json` retains 讯使 on her
automatic first skill, which grants nine deployment points on every cast, and hands the solver the
`圣聆初雪` slot to replace. It exports, and `tests/main_story_01_01_level_solve.rs`
pins that export and its independent validation.

```fish
cargo run -p arknights-cli -- level solve main_01-01 \
	--snapshot $gamedata_dir \
	--prefab-snapshot $analysis_dir/battle-prefabs \
	--guide src/ark_solver/tests/fixtures/maa_job_main_01_01_solve_source.json \
	--difficulty normal \
	--roster src/ark_solver/examples/level_solve_rosters/bd7-exusiai-s2.json \
	--top-k 1 \
	--output-dir $analysis_dir/main-01-01-exusiai-s2
```

The exported DP ledger records the grant that pays for it: 讯使 deploys at tick 0 for the stage's
whole initial DP, and 蛇屠箱's nineteen at tick 510 is covered by seventeen regenerated plus nine
granted. Regeneration alone would not have afforded her until tick 570, and before the grants were
scheduled the export rejected every proposal with `artifact_deployment_tick_mismatch`.

## Main story 1-1, with a gap only in the third wave

`tests/fixtures/maa_job_main_01_01_late_gap_source.json` retains 讯使, 蛇屠箱 and
黍 and hands the solver the `圣聆初雪` slot. Without her the first two waves are fully defeated and
only three third-wave enemies leak, so a battlefield view that stops early sees no gap.
`tests/main_story_01_01_late_gap_solve.rs` pins the solve on it. The source slot
is the guide's last `Deploy`: candidate replay appends the replacement's skill schedule after the
source's last command and refuses to materialize when another deployment follows it.
