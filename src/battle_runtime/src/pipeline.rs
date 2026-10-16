use crate::{
	comparison,
	environment::HostConfig,
	maa, probe, program,
	runtime::{self, Battle, Presentation, Request},
	squad::{self, SquadTraining, Training},
};
use anyhow::{ensure, Context, Result};
use ark_unity::{assets::Catalog, build, io};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
	collections::{BTreeMap, BTreeSet},
	fs,
	path::{Path, PathBuf},
};

pub type RunReport = Value;

/// Where `setup` takes the owned Player from.
#[derive(Clone, Debug)]
pub enum PlayerSource {
	/// Keep a verified installed Player, otherwise install the published prebuilt one.
	Installed,
	/// Install the prebuilt Player from a local release archive.
	Archive(PathBuf),
	/// Build it locally with Unity 2021.3.39f1 (maintainers).
	Build,
}
#[derive(Clone, Debug)]
pub struct RunOptions {
	pub seed: u32,
	pub repeat: bool,
	pub max_ticks: u32,
	pub resource_rounds: u32,
	/// Load the client's original Lua code hotfixes, as the official client does.
	pub lua_hotfix: bool,
	/// Record every change of the operators' original skill and unit state per tick.
	pub trace: bool,
	pub presentation: Presentation,
	/// Original speed level (1 or 2) a presented battle starts at.
	pub speed: u32,
	/// Unity time scale of a presented battle.
	pub time_scale: u32,
	/// Ticks every unit operation waits after its conditions are met: a declared latency.
	pub delay: u32,
}
impl Default for RunOptions {
	fn default() -> Self {
		Self {
			seed: 1,
			repeat: false,
			max_ticks: 36000,
			resource_rounds: 3,
			lua_hotfix: true,
			trace: false,
			presentation: Presentation::Fast,
			speed: 1,
			time_scale: 1,
			delay: 0,
		}
	}
}
impl RunOptions {
	pub fn validate(&self) -> Result<()> {
		ensure!(
			self.seed <= i32::MAX as u32,
			"Seed must be nonnegative Int32"
		);
		ensure!(
			(1..=54000).contains(&self.max_ticks),
			"max-ticks must be 1..54000"
		);
		ensure!(
			(1..=8).contains(&self.resource_rounds),
			"resource-rounds must be 1..8"
		);
		ensure!(self.delay <= 900, "delay must be 0..900 ticks");
		Ok(())
	}
}

pub struct Host {
	pub config: HostConfig,
}
struct Prepared {
	level: PathBuf,
	scene: PathBuf,
	services: PathBuf,
	plan: PathBuf,
}
impl Host {
	pub fn new(config: HostConfig) -> Self {
		Self { config }
	}
	fn current_host(&self) -> Result<PathBuf> {
		let pointer: Value = io::read_json(&self.config.state.join("current-host.json"))
			.context("Run `ark-cli battle setup` first")?;
		Ok(PathBuf::from(
			pointer["build"]
				.as_str()
				.context("Host build pointer missing")?,
		))
	}
	/// Prepare everything a run needs, idempotently: remember the client, place third-party
	/// tools and the probe, install the owned Player, pin the client and export original tables.
	pub fn setup(&self, player: &PlayerSource) -> Result<Value> {
		self.config.remember()?;
		let tools: Value = crate::tools::ensure_tools(&self.config.state)?;
		runtime::inventory(&self.config, true)?;
		let probe = probe::build(&self.config)?;
		let pointer = self.config.state.join("current-host.json");
		let reuse: bool = matches!(player, PlayerSource::Installed)
			&& pointer.is_file()
			&& self
				.current_host()
				.and_then(|host| build::verified_player(&host))
				.is_ok();
		let host: PathBuf = if reuse {
			self.current_host()?
		} else {
			match player {
				PlayerSource::Build => build::build(
					&self.config.state,
					&crate::bundle::materialize(
						&self.config.state,
						"unity",
						crate::bundle::UNITY_FILES,
					)?,
					&probe,
				)?,
				PlayerSource::Archive(zip) => {
					crate::tools::install_player(&self.config.state, Some(zip))?
				}
				PlayerSource::Installed => crate::tools::install_player(&self.config.state, None)?,
			}
		};
		build::verified_player(&host)?;
		let catalog = Catalog::new(&self.config.state, &self.config.client)?;
		let mut request = Request::control(probe.clone(), host.clone());
		request.deadline = 10;
		let (output, result) = runtime::run(&self.config, &request)?;
		ensure!(
			result["passed"] == true,
			"Original runtime control failed: {}",
			output.display()
		);
		io::write_json(&pointer, &json!({"build":host,"control_run":output}))?;
		let tables = self.tables(&catalog)?;
		Ok(
			json!({"passed":true,"client":self.config.client,"tools":tools,"host_build":host,"control_run":output,"probe":probe,"catalog":catalog.provenance(),"operators_exported":tables["characters"].as_object().map_or(0,|c|c.len()),"stages_exported":tables["stages"]["stages"].as_object().map_or(0,|s|s.len())}),
		)
	}

	/// Maintainer: package the current verified owned Player as the release asset.
	pub fn package_player(&self, output: &Path) -> Result<Value> {
		crate::tools::package_player(&self.current_host()?, output)
	}

	/// Maintainer: compile the probe locally and write it with its source digest for a release.
	pub fn build_probe(&self, output: &Path) -> Result<Value> {
		let dll: PathBuf = probe::compile(&self.config)?;
		fs::create_dir_all(output)?;
		fs::copy(&dll, output.join("ArkBattleProbe.dll"))?;
		fs::write(
			output.join("probe-source-digest.txt"),
			probe::source_digest(),
		)?;
		Ok(
			json!({"probe": output.join("ArkBattleProbe.dll"), "source_digest": probe::source_digest()}),
		)
	}
	fn prepare(
		&self,
		catalog: &Catalog,
		level_id: &str,
		squad: &Value,
		extra: &[String],
	) -> Result<Prepared> {
		let host = self.current_host()?;
		let ids: Vec<Value> = squad["operators"]
			.as_array()
			.context("Squad operators missing")?
			.iter()
			.map(|op| json!([op["character_id"], op["skin_id"]]))
			.collect();
		let key_source = json!({"catalog":catalog.provenance(),"level":level_id,"squad":ids,"extra":extra,"host":io::sha256_file(&host)?,"revision":runtime::source_revision()?});
		let key = format!("{:x}", Sha256::digest(serde_json::to_vec(&key_source)?));
		let cache = self
			.config
			.state
			.join("prepared")
			.join(format!("{}.json", &key[..24]));
		if cache.is_file() {
			let cached: Value = io::read_json(&cache)?;
			let paths: Vec<PathBuf> = ["level", "scene", "services", "plan"]
				.iter()
				.map(|k| {
					cached[*k]
						.as_str()
						.map(PathBuf::from)
						.context("Invalid prepared cache")
				})
				.collect::<Result<_>>()?;
			if paths.iter().all(|p| p.is_file()) {
				for (name, path) in ["level", "scene", "services", "plan"]
					.into_iter()
					.zip(&paths)
				{
					ensure!(
						cached["hashes"][name] == io::sha256_file(path)?,
						"Prepared manifest changed"
					);
				}
				return Ok(Prepared {
					level: paths[0].clone(),
					scene: paths[1].clone(),
					services: paths[2].clone(),
					plan: paths[3].clone(),
				});
			}
		}
		eprintln!("INFO ark_battle_runtime: preparing original level {level_id}");
		let level = catalog.prepare_level(level_id)?;
		let mut request = Request::control(probe::build(&self.config)?, host.clone());
		request.level = Some(level.clone());
		request.deadline = 10;
		let (decode_run, decoded) = runtime::run(&self.config, &request)?;
		ensure!(
			decoded["passed"] == true,
			"Original level decode failed: {}",
			decode_run.display()
		);
		let decoded: Value = io::read_json(Path::new(
			decoded["decoded_level"]["path"]
				.as_str()
				.context("Decoded level missing")?,
		))?;
		let (scene, services, plan) = catalog.prepare_scene_services(
			&build::verified_player(&host)?,
			&level,
			&decoded,
			squad,
			extra,
		)?;
		fs::create_dir_all(cache.parent().unwrap())?;
		io::write_json(
			&cache,
			&json!({"key_source":key_source,"level":level,"scene":scene,"services":services,"plan":plan,"hashes":{"level":io::sha256_file(&level)?,"scene":io::sha256_file(&scene)?,"services":io::sha256_file(&services)?,"plan":io::sha256_file(&plan)?}}),
		)?;
		Ok(Prepared {
			level,
			scene,
			services,
			plan,
		})
	}
	fn tables(&self, catalog: &Catalog) -> Result<Value> {
		let identity_inputs = json!({"catalog":catalog.provenance(),"source":runtime::source_revision()?,"host":io::sha256_file(&self.current_host()?)?,"runtime":runtime::inventory(&self.config,false)?});
		let key = format!(
			"{:x}",
			Sha256::digest(serde_json::to_vec(&identity_inputs)?)
		);
		let cached = self
			.config
			.state
			.join("identities")
			.join(format!("rust-{}.json", &key[..24]));
		if cached.is_file() {
			let value: Value = io::read_json(&cached)?;
			for pin in value["inputs"]
				.as_array()
				.context("Identity provenance missing")?
			{
				io::verify_pin(pin)?;
			}
			return Ok(value);
		}
		// Only the table export uses a reference scene. Job execution resolves its own level.
		let prepared = self.prepare(
			catalog,
			"obt/main/level_main_01-01",
			&json!({"schema":"ark-battle-squad-v2","operators":[]}),
			&[],
		)?;
		let mut request = Request::control(probe::build(&self.config)?, self.current_host()?);
		request.services = Some(prepared.services.clone());
		request.export_identities = true;
		let (output, result) = runtime::run(&self.config, &request)?;
		ensure!(
			result["passed"] == true,
			"Original identity export failed: {}",
			output.display()
		);
		let mut tables = json!({"export_run":output,"identity_inputs":identity_inputs,"inputs":[io::pin(&prepared.services)?]});
		for (_, label) in runtime::IDENTITY_TABLES {
			let path = io::verify_pin(&result["identity_tables"][label])?;
			tables[label] = io::read_json(&path)?;
			tables["inputs"]
				.as_array_mut()
				.unwrap()
				.push(io::pin(&path)?);
		}
		fs::create_dir_all(cached.parent().unwrap())?;
		io::write_json(&cached, &tables)?;
		Ok(tables)
	}
	pub fn squad_for(&self, job: &Path, output: &Path, training: &Training) -> Result<Value> {
		let catalog = Catalog::new(&self.config.state, &self.config.client)?;
		let tables = self.tables(&catalog)?;
		let squad = squad::for_job(
			&io::read_json(job)?,
			&squad_tables(&tables),
			&SquadTraining::team(*training),
		)?;
		io::write_json(output, &squad)?;
		Ok(
			json!({"squad":output,"training":training_json(training),"operators":squad["operators"].as_array().unwrap().iter().map(|op|op["character_id"].clone()).collect::<Vec<_>>()}),
		)
	}

	/// Lower training from the maximum while the original engine still wins, for the lowest
	/// passing team training. With `per_operator`, then lower each operator alone from that
	/// result. Every tier is an original-engine run; the search only chooses the next tier.
	pub fn ladder(
		&self,
		job_path: &Path,
		options: &RunOptions,
		search: LadderSearch,
		per_operator: bool,
		allow_leaks: bool,
	) -> Result<Value> {
		let catalog = Catalog::new(&self.config.state, &self.config.client)?;
		let tables = self.tables(&catalog)?;
		let job: Value = io::read_json(job_path)?;
		let output: PathBuf = io::stage_dir(&self.config.state, "ladder")?;
		let mut attempts: Vec<Value> = Vec::new();
		// Job requirements are floors and levels cap at the phase maximum, so distinct
		// trainings can generate one squad; each squad runs once.
		let mut outcomes: BTreeMap<String, bool> = BTreeMap::new();
		let mut run = |training: &SquadTraining, scope: &str| -> Result<bool> {
			let squad = match squad::for_job(&job, &squad_tables(&tables), training) {
				Ok(squad) => squad,
				Err(error) => {
					attempts.push(json!({"scope":scope,"training":squad_training_json(training),"skipped":format!("{error:#}")}));
					return Ok(false);
				}
			};
			let key: String = squad.to_string();
			if let Some(passed) = outcomes.get(&key) {
				attempts.push(json!({"scope":scope,"training":squad_training_json(training),"skipped":"same squad as an earlier attempt","passed":passed}));
				return Ok(*passed);
			}
			let squad_path: PathBuf = output.join(format!("step-{:02}.squad.json", attempts.len()));
			io::write_json(&squad_path, &squad)?;
			let report: Value = self.run_maa(job_path, &squad_path, options)?;
			// A tier passes when the original engine wins, even before later commands become due,
			// and, unless leaks are allowed, no enemy reached an exit.
			let passed: bool = report["passed"] == true
				&& report["battle_outcome"]["status"] == "original_win"
				&& (allow_leaks || report["battle_events"]["perfect"] == true);
			eprintln!(
				"INFO ark_battle_runtime: ladder {scope} {} passed={passed}",
				squad_training_json(training)
			);
			attempts.push(json!({"scope":scope,"training":squad_training_json(training),"squad":squad_path,"run":report["run"],"passed":passed,"battle_outcome":report["battle_outcome"]["status"],"fixed_frame":report["battle_outcome"]["fixed_frame"],"life_point":report["battle_outcome"]["life_point"],"leak_count":report["battle_events"]["leak_count"],"job_result":report["job_result"],"concrete_job":report["concrete_job"]}));
			outcomes.insert(key, passed);
			Ok(passed)
		};
		let mut current: SquadTraining = SquadTraining::default();
		let mut tradeoffs: Vec<Value> = Vec::new();
		let maximum_passed: bool = run(&current, "team")?;
		if maximum_passed {
			let ids: Vec<String> = if per_operator {
				squad::for_job(&job, &squad_tables(&tables), &current)?["operators"]
					.as_array()
					.into_iter()
					.flatten()
					.filter_map(|op| op["character_id"].as_str().map(str::to_owned))
					.collect()
			} else {
				Vec::new()
			};
			// The team first, then each operator alone in squad order.
			for scope in std::iter::once(None).chain(ids.iter().map(Some)) {
				let with = |current: &SquadTraining, training: Training| -> SquadTraining {
					match scope {
						None => SquadTraining::team(training),
						Some(id) => {
							let mut candidate: SquadTraining = current.clone();
							candidate.operators.insert(id.clone(), training);
							candidate
						}
					}
				};
				let label: &str = scope.map_or("team", String::as_str);
				let base = |current: &SquadTraining| -> Training {
					match scope {
						None => current.team,
						Some(id) => *current.of(id),
					}
				};
				match search {
					LadderSearch::Tiers => {
						for axis in AXES {
							for tier in axis.tiers(base(&current)) {
								let candidate: SquadTraining = with(&current, tier);
								if !run(&candidate, label)? {
									break;
								}
								current = candidate;
							}
						}
					}
					LadderSearch::Bisect => {
						// Mastery and module change skill mechanics and timing, so a lower
						// value can win where a higher one loses: try every pair, lowest first.
						let top: Training = base(&current);
						for mechanics in mechanics_pairs(top) {
							let candidate: SquadTraining = with(&current, mechanics);
							if run(&candidate, label)? {
								current = candidate;
								break;
							}
						}
						// Level and trust only scale stats and trade against each other, so report
						// each tradeoff: one at its floor with the other's lowest, and both lowered
						// together then each alone. The balanced point continues the search.
						let start: SquadTraining = current.clone();
						let top: Training = base(&start);
						let mut points: Vec<Value> = Vec::new();
						for (preference, floor, free) in [
							("trust_floor", Axis::Trust, Axis::Level),
							("level_floor", Axis::Level, Axis::Trust),
						] {
							let floored: Training = floor.at(top, 0);
							if !run(&with(&start, floored), label)? {
								continue;
							}
							let rank: usize = lowest_passing(free.rank(&floored), |rank| {
								run(&with(&start, free.at(floored, rank)), label)
							})?;
							points.push(json!({"preference":preference,
								"training":training_json(&free.at(floored, rank))}));
						}
						let step: usize = lowest_passing(STAT_STEPS, |step| {
							run(&with(&start, stats(top, step)), label)
						})?;
						current = with(&start, stats(top, step));
						for axis in [Axis::Level, Axis::Trust] {
							let top: Training = base(&current);
							let rank: usize = lowest_passing(axis.rank(&top), |rank| {
								run(&with(&current, axis.at(top, rank)), label)
							})?;
							current = with(&current, axis.at(top, rank));
						}
						points.push(json!({"preference":"balanced",
							"training":training_json(&base(&current))}));
						tradeoffs.push(json!({"scope":label,"points":points}));
					}
				}
			}
		}
		let (order, assumption): (&str, &str) = match search {
			LadderSearch::Tiers => (
				"team: module 3→0, mastery 3→0, level max→60→30→1 at the highest elite phase, trust max→100→50→0, each axis stopping at its first loss; per operator, the same axes for one operator at a time in squad order",
				"greedy; the axis and operator order can affect the result; job requirements stay floors",
			),
			LadderSearch::Bisect => (
				"team: every mastery and module pair, lowest sum first, keeping the first that wins; then level and trust as stat tradeoffs: trust 0 with the lowest level, level 1 with the lowest trust, and both lowered together in 16 proportional steps then each alone (balanced, which the search continues from), every bisection trying the floor first; per operator, the same for one operator at a time in squad order",
				"mechanics are enumerated because skill timing can make lower mastery or module win where higher loses; stat bisection assumes higher level and trust never lose where lower win, so where a job breaks that, the result passes but may not be the lowest; job requirements stay floors",
			),
		};
		let report = json!({"schema":"ark-battle-training-ladder-v3","job":job_path,"maximum_passed":maximum_passed,
			"search_mode":search.name(),"per_operator":per_operator,"allow_leaks":allow_leaks,
			"lowest_passing_training":if maximum_passed { squad_training_json(&current) } else { Value::Null },
			"stat_tradeoffs":tradeoffs,"runs":outcomes.len(),"attempts":attempts,"order":order,"search":assumption,
			"verification_scope":"original-engine observations per tier; not an Ark Solver verdict"});
		io::write_json(&output.join("ladder.json"), &report)?;
		Ok(report)
	}

	/// Run a job with an explicit squad, or with one generated from the original tables at the
	/// given training (default: the original maximum).
	pub fn run(
		&self,
		job_path: &Path,
		squad_path: Option<&Path>,
		training: &Training,
		options: &RunOptions,
	) -> Result<RunReport> {
		let generated: PathBuf;
		let squad_path: &Path = match squad_path {
			Some(path) => path,
			None => {
				let directory: PathBuf = io::stage_dir(&self.config.state, "squads")?;
				generated = directory.join("squad.json");
				self.squad_for(job_path, &generated, training)?;
				&generated
			}
		};
		let mut report: Value = self.run_maa(job_path, squad_path, options)?;
		report["squad"] = json!(squad_path);
		Ok(report)
	}

	/// Convert between formats by the input's kind: an MAA job becomes a battle program (with
	/// whether it exports back unchanged); a battle program becomes an MAA job, or every reason MAA
	/// cannot express it.
	pub fn convert(&self, input: &Path, output: &Path) -> Result<Value> {
		let source: Value = io::read_json(input)?;
		if !program::is_program(&source) {
			let program: Value = program::from_maa(&source)?;
			io::write_json(output, &program)?;
			let exportable: bool = program::to_maa(&program)?.is_ok();
			return Ok(json!({"program": output,
				"steps": program["steps"].as_array().map_or(0, Vec::len),
				"exports_to_maa": exportable}));
		}
		match program::to_maa(&source)? {
			Ok(job) => {
				io::write_json(output, &job)?;
				Ok(json!({"maa_job": output, "exported": true}))
			}
			Err(reasons) => Ok(json!({"exported": false, "reasons": reasons})),
		}
	}

	/// The squad a job or program runs with: the given one, or one generated at the training.
	fn squad_path(&self, job: &Path, squad: Option<&Path>, training: &Training) -> Result<PathBuf> {
		match squad {
			Some(path) => Ok(path.to_path_buf()),
			None => {
				let path: PathBuf = io::stage_dir(&self.config.state, "squads")?.join("squad.json");
				self.squad_for(job, &path, training)?;
				Ok(path)
			}
		}
	}

	/// The largest latency, in ticks before every unit operation, at which the job still wins
	/// without a leak. Bisection assumes more latency never helps.
	pub fn margin(
		&self,
		job: &Path,
		squad: Option<&Path>,
		training: &Training,
		options: &RunOptions,
		maximum: u32,
	) -> Result<Value> {
		let squad_path: PathBuf = self.squad_path(job, squad, training)?;
		let mut attempts: Vec<Value> = Vec::new();
		let mut perfect = |delay: u32| -> Result<bool> {
			let report: Value = self.run_maa(
				job,
				&squad_path,
				&RunOptions {
					delay,
					..options.clone()
				},
			)?;
			let passed: bool = report["passed"] == true
				&& report["battle_outcome"]["status"] == "original_win"
				&& report["battle_events"]["perfect"] == true;
			eprintln!("INFO ark_battle_runtime: margin delay {delay} perfect={passed}");
			attempts.push(
				json!({"delay_ticks": delay, "perfect": passed, "run": report["run"],
				"fixed_frame": report["battle_outcome"]["fixed_frame"],
				"life_point": report["battle_outcome"]["life_point"],
				"leak_count": report["battle_events"]["leak_count"]}),
			);
			Ok(passed)
		};
		let tolerated: Option<u32> = if !perfect(0)? {
			None
		} else if perfect(maximum)? {
			Some(maximum)
		} else {
			let (mut low, mut high): (u32, u32) = (0, maximum);
			while high - low > 1 {
				let middle: u32 = (low + high) / 2;
				if perfect(middle)? {
					low = middle;
				} else {
					high = middle;
				}
			}
			Some(low)
		};
		let report: Value = json!({"schema": "ark-battle-delay-margin-v1", "job": job, "squad": squad_path,
			"tolerated_delay_ticks": tolerated,
			"tolerated_delay_seconds": tolerated.map(|ticks| f64::from(ticks) / 30.0),
			"searched_up_to_ticks": maximum, "attempts": attempts,
			"meaning": "every unit operation waits this many ticks after its conditions are met, and every policy skill stays ready this long before its cast, and the original engine still wins with no leak; null when even no delay leaks",
			"search": "bisection, assuming more latency never helps"});
		io::write_json(
			&io::stage_dir(&self.config.state, "margins")?.join("margin.json"),
			&report,
		)?;
		Ok(report)
	}

	/// For each skill step, the delays after it is ready at which the job still wins with no leak,
	/// and the best one; then all best delays together, verified.
	pub fn advise(
		&self,
		job: &Path,
		squad: Option<&Path>,
		training: &Training,
		options: &RunOptions,
		waits: &[u32],
	) -> Result<Value> {
		let squad_path: PathBuf = self.squad_path(job, squad, training)?;
		let input: Value = io::read_json(job)?;
		let base: Value = if program::is_program(&input) {
			input
		} else {
			program::from_maa(&input)?
		};
		let output: PathBuf = io::stage_dir(&self.config.state, "advice")?;
		let run = |candidate: &Value, label: &str| -> Result<Value> {
			let path: PathBuf = output.join(format!("{label}.program.json"));
			io::write_json(&path, candidate)?;
			let report: Value = self.run_maa(&path, &squad_path, options)?;
			let perfect: bool = report["passed"] == true
				&& report["battle_outcome"]["status"] == "original_win"
				&& report["battle_events"]["perfect"] == true;
			Ok(json!({"perfect": perfect, "run": report["run"],
				"outcome": report["battle_outcome"]["status"],
				"fixed_frame": report["battle_outcome"]["fixed_frame"],
				"life_point": report["battle_outcome"]["life_point"],
				"leak_count": report["battle_events"]["leak_count"],
				"step_ticks": report["step_ticks"]}))
		};
		let with_wait = |program: &Value, step: usize, wait: u32| -> Value {
			// The wait is the step's delay: its operation stays ready that long first.
			let mut candidate: Value = program.clone();
			if wait > 0 {
				candidate["steps"][step]["delay"] = json!(wait);
			}
			candidate
		};

		let skills: Vec<usize> = base["steps"]
			.as_array()
			.context("Program steps missing")?
			.iter()
			.enumerate()
			.filter(|(_, step)| matches!(step["do"].as_str(), Some("press" | "cast")))
			.map(|(index, _)| index)
			.collect();
		let baseline: Value = run(&base, "baseline")?;
		let mut advised: Value = base.clone();
		let mut per_step: Vec<Value> = Vec::new();
		for step in &skills {
			let mut tried: Vec<Value> = Vec::new();
			for wait in waits {
				let result: Value = if *wait == 0 {
					baseline.clone()
				} else {
					run(
						&with_wait(&base, *step, *wait),
						&format!("step{step}-wait{wait}"),
					)?
				};
				eprintln!(
					"INFO ark_battle_runtime: advise step {step} wait {wait} perfect={}",
					result["perfect"]
				);
				tried.push(json!({"wait_ticks": wait, "result": result}));
			}
			// Best: a perfect win with the most life left, then the earliest finish, then the
			// smallest wait.
			let best: Option<&Value> = tried
				.iter()
				.filter(|entry| entry["result"]["perfect"] == true)
				.max_by_key(|entry| {
					(
						entry["result"]["life_point"].as_i64().unwrap_or(0),
						-entry["result"]["fixed_frame"].as_i64().unwrap_or(i64::MAX),
						-entry["wait_ticks"].as_i64().unwrap_or(0),
					)
				});
			let best_wait: Option<u64> = best.and_then(|entry| entry["wait_ticks"].as_u64());
			if let Some(wait) = best_wait.filter(|wait| *wait > 0) {
				advised = with_wait(&advised, *step, wait as u32);
			}
			let window: Vec<u64> = tried
				.iter()
				.filter(|entry| entry["result"]["perfect"] == true)
				.filter_map(|entry| entry["wait_ticks"].as_u64())
				.collect();
			per_step.push(json!({"step": step, "name": base["steps"][*step]["name"],
				"do": base["steps"][*step]["do"], "perfect_waits": window, "best_wait_ticks": best_wait,
				"issued_tick_without_wait": baseline["step_ticks"][step.to_string()],
				"tried": tried}));
		}
		let combined: Value = run(&advised, "advised")?;
		let path: PathBuf = output.join("advised.program.json");
		io::write_json(&path, &advised)?;
		let export = program::to_maa(&advised)?;
		let report: Value = json!({"schema": "ark-battle-skill-advice-v1", "job": job, "squad": squad_path,
			"waits_tried_ticks": waits, "baseline": baseline, "steps": per_step,
			"advised_program": path, "advised_result": combined,
			"advised_exports_to_maa": export.is_ok(),
			"maa_note": "a step delay is not an MAA condition; issued ticks and the kill counts at them in each run's events can be expressed as MAA kills conditions",
			"search": "each skill step alone over the given waits; the combined advice is verified by its own run"});
		io::write_json(&output.join("advice.json"), &report)?;
		Ok(report)
	}

	pub(crate) fn run_maa(
		&self,
		job_path: &Path,
		squad_path: &Path,
		options: &RunOptions,
	) -> Result<RunReport> {
		options.validate()?;
		let catalog = Catalog::new(&self.config.state, &self.config.client)?;
		let input: Value = io::read_json(job_path)?;
		// A battle program translates through its MAA-shaped job, then takes its own conditions.
		let program: Option<&Value> = program::is_program(&input).then_some(&input);
		let job: Value = match program {
			Some(program) => program::as_job(program)?,
			None => input.clone(),
		};
		let squad: Value = io::read_json(squad_path)?;
		squad::compile(&squad)?;
		let tables = self.tables(&catalog)?;
		let stage = resolve_stage(&job["stage_name"], &tables["stages"], &catalog.assets)?;
		let level_id = stage["level_id"].as_str().unwrap();
		let mut plan = maa::translate(&job, &squad, &tables["characters"], &tables["modules"])?;
		if let Some(program) = program {
			program::apply(&mut plan, program)?;
		}
		let native = maa::compile(&plan, options.delay)?;
		let mut extra: BTreeSet<String> = BTreeSet::new();
		// Summons deployed by name are not squad members; prepare their original prefabs too.
		for command in plan["commands"].as_array().into_iter().flatten() {
			if let Some(token) = command["character_id"]
				.as_str()
				.filter(|id| id.starts_with("token_"))
			{
				extra.extend(
					catalog
						.assets
						.keys()
						.filter(|key| {
							key.starts_with("battle/prefabs/")
								&& key.rsplit('/').next() == Some(token)
						})
						.cloned(),
				);
			}
		}
		for attempt in 1..=options.resource_rounds {
			let extra_list: Vec<String> = extra.iter().cloned().collect();
			let prepared = self.prepare(&catalog, level_id, &squad, &extra_list)?;
			let mut request = Request::control(probe::build(&self.config)?, self.current_host()?);
			request.level = Some(prepared.level);
			request.scene = Some(prepared.scene.clone());
			request.services = Some(prepared.services.clone());
			request.lua_hotfix = options.lua_hotfix;
			request.battle = Some(Battle {
				actions: native.clone(),
				actions_source: json!({"job":job_path.canonicalize()?,"job_sha256":io::sha256_file(job_path)?,"profile":maa::PROFILE,"plan":plan,"stage":stage}),
				squad: squad.clone(),
				seed: options.seed,
				max_ticks: options.max_ticks,
				trace: options.trace,
				presentation: options.presentation,
				speed: options.speed,
				time_scale: options.time_scale,
			});
			request.notes = json!({"plan":prepared.plan,"attempt":attempt});
			let (output, result) = runtime::run(&self.config, &request)?;
			let log = String::from_utf8_lossy(
				&fs::read(output.join("run/player.log")).unwrap_or_default(),
			)
			.into_owned();
			let new: Vec<String> = observed_requests(&log)
				.into_iter()
				.filter(|p| !extra.contains(p))
				.collect();
			if !new.is_empty() && attempt < options.resource_rounds {
				eprintln!("INFO ark_battle_runtime: attempt {attempt}, preparing {} additional original resource routes",new.len());
				extra.extend(new);
				continue;
			}
			let mut report = json!({"run":output,"passed":result["passed"],"level_id":level_id,"stage_ids":stage["stage_ids"],"automatic_skill_count":result["automatic_skill_count"],"battle_outcome":result["battle_outcome"],"maa_job_complete":result["maa_job_complete"],"completed_action_count":result["completed_action_count"],"remaining_action_count":result["remaining_action_count"],"runtime_diagnostics":result["runtime_diagnostics"],"extra_routes":extra,"catalog_version":catalog.version_id,"resource_origins":origins(&prepared.scene,&prepared.services)?});
			report["job_result"] = result["job_result"].clone();
			report["battle_events"] = crate::events::summarize(&output.join("run/observer.jsonl"))?;
			report["delay_ticks"] = json!(options.delay);
			report["step_ticks"] = step_ticks(&output.join("run/observer.jsonl"))?;
			if result["battle_outcome"]["terminal_verified"] == true {
				io::write_json(
					&output.join("battle-proof.json"),
					&crate::proof::summarize(&output.join("result.json"))?,
				)?;
			}
			// A won run establishes a concrete job: the executed actions with the trained squad.
			if result["passed"] == true && result["battle_outcome"]["status"] == "original_win" {
				let note: String = format!(
					"Established by ark-battle-runtime on the original battle code: {} at tick {}, job {}. Source job {} (sha256 {}), squad {}, level {}, resources {}.",
					result["battle_outcome"]["original_result_name"].as_str().unwrap_or_default(),
					result["battle_outcome"]["fixed_frame"],
					result["job_result"].as_str().unwrap_or_default(),
					job_path.display(),
					io::sha256_file(job_path)?,
					squad_path.display(),
					level_id,
					catalog.version_id
				);
				let completed: usize =
					result["completed_action_count"].as_u64().unwrap_or(0) as usize;
				let concrete: Value =
					maa::concrete_job(&job, &plan, &squad, &tables["modules"], completed, &note)?;
				// A program stays a program: its operators take the trained requirements and its
				// steps end at the last completed one.
				let (name, concrete): (&str, Value) = match program {
					Some(program) => {
						let mut concrete_program: Value = program.clone();
						for key in ["opers", "groups", "doc"] {
							if let Some(value) = concrete.get(key) {
								concrete_program[key] = value.clone();
							}
						}
						let kept: usize = concrete["actions"].as_array().map_or(0, Vec::len);
						let steps: Vec<Value> = program["steps"]
							.as_array()
							.map_or(Vec::new(), |steps| steps[..kept.min(steps.len())].to_vec());
						concrete_program["steps"] = json!(steps);
						("concrete-program.json", concrete_program)
					}
					None => ("concrete-job.json", concrete),
				};
				let path: PathBuf = output.join(name);
				io::write_json(&path, &concrete)?;
				report["concrete_job"] = json!(path);
			}
			if options.repeat {
				let (second, _) = runtime::run(&self.config, &request)?;
				let comparison =
					comparison::compare(&output.join("result.json"), &second.join("result.json"))?;
				report["repeat_run"] = json!(second);
				report["repeatable"] = comparison["passed"].clone();
				report["passed"] = json!(report["passed"] == true && comparison["passed"] == true);
			}
			io::write_json(&output.join("report.json"), &report)?;
			return Ok(report);
		}
		unreachable!("Validated resource round count is positive")
	}
}

fn resolve_stage(
	wanted: &Value,
	table: &Value,
	assets: &BTreeMap<String, String>,
) -> Result<Value> {
	let wanted = wanted
		.as_str()
		.filter(|s| !s.is_empty())
		.context("MAA stage_name missing")?
		.to_lowercase();
	let levels: BTreeMap<String, String> = assets
		.keys()
		.filter_map(|key| {
			key.strip_prefix("gamedata/levels/")
				.map(|level| (level.to_lowercase(), level.to_owned()))
		})
		.collect();
	let mut candidates: BTreeMap<String, Vec<String>> = BTreeMap::new();
	for (id, stage) in table["stages"]
		.as_object()
		.context("Original stages table missing")?
	{
		let Some(level) = stage["levelId"].as_str() else {
			continue;
		};
		let Some(original) = levels.get(&level.to_lowercase()) else {
			continue;
		};
		if [
			Some(id.as_str()),
			Some(level),
			level.rsplit('/').next(),
			stage["code"].as_str(),
			stage["name"].as_str(),
		]
		.into_iter()
		.flatten()
		.any(|name| name.to_lowercase() == wanted)
		{
			candidates
				.entry(original.clone())
				.or_default()
				.push(id.clone());
		}
	}
	if let Some(level) = levels.get(&wanted) {
		candidates.entry(level.clone()).or_default();
	}
	ensure!(
		candidates.len() == 1,
		"stage_name must identify one installed original level; candidates: {:?}",
		candidates.keys()
	);
	let (level, mut ids) = candidates.into_iter().next().unwrap();
	ids.sort();
	Ok(json!({"level_id":level,"stage_ids":ids.join(",")}))
}

fn observed_requests(log: &str) -> BTreeSet<String> {
	log.lines()
		.filter_map(|line| {
			line.split_once("[ObjectPool] Failed to create new object: ")
				.map(|(_, route)| {
					route
						.trim()
						.to_lowercase()
						.trim_start_matches("dyn/")
						.to_owned()
				})
		})
		.collect()
}
fn origins(scene: &Path, services: &Path) -> Result<Value> {
	let mut counts = json!({});
	for (label, path, key) in [
		("scene_bundles", scene, "bundles"),
		("tables", services, "tables"),
	] {
		let data: Value = io::read_json(path)?;
		let mut origins: BTreeMap<String, usize> = BTreeMap::new();
		for entry in data[key].as_array().context("Origin entries missing")? {
			let source = entry
				.get("source")
				.filter(|s| s.is_object())
				.unwrap_or(entry);
			let origin = source["origin"]
				.as_str()
				.context("Resource origin absent")?;
			*origins.entry(origin.to_owned()).or_default() += 1;
		}
		counts[label] = json!(origins);
	}
	Ok(counts)
}

/// How `ladder` chooses the next training to run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LadderSearch {
	/// Fixed tiers per axis, each axis stopping at its first loss.
	#[default]
	Tiers,
	/// Bisection, assuming higher training never loses where lower training wins.
	Bisect,
}

impl LadderSearch {
	fn name(self) -> &'static str {
		match self {
			Self::Tiers => "tiers",
			Self::Bisect => "bisect",
		}
	}
}

/// One training axis, ranked from its lowest value to the original maximum.
#[derive(Clone, Copy, Debug)]
enum Axis {
	Module,
	Mastery,
	Level,
	Trust,
}

const AXES: [Axis; 4] = [Axis::Module, Axis::Mastery, Axis::Level, Axis::Trust];
/// Level ranks are levels 1..=90 then the original maximum.
const LEVEL_MAXIMUM: usize = 90;
/// Trust ranks are percents 0..=200 then the original maximum.
const TRUST_MAXIMUM: usize = 201;
const STAT_STEPS: usize = 16;

impl Axis {
	fn rank(self, training: &Training) -> usize {
		match self {
			Self::Module => training.module as usize,
			Self::Mastery => training.mastery as usize,
			Self::Level => training
				.level
				.map_or(LEVEL_MAXIMUM, |level| (level - 1).clamp(0, 89) as usize),
			Self::Trust => training
				.trust
				.map_or(TRUST_MAXIMUM, |trust| trust.clamp(0, 200) as usize),
		}
	}

	fn at(self, base: Training, rank: usize) -> Training {
		match self {
			Self::Module => Training {
				module: rank as i64,
				..base
			},
			Self::Mastery => Training {
				mastery: rank as i64,
				..base
			},
			Self::Level => Training {
				level: (rank < LEVEL_MAXIMUM).then_some(rank as i64 + 1),
				..base
			},
			Self::Trust => Training {
				trust: (rank < TRUST_MAXIMUM).then_some(rank as i64),
				..base
			},
		}
	}

	/// The fixed tiers below `base`, highest first.
	fn tiers(self, base: Training) -> Vec<Training> {
		let current: usize = self.rank(&base);
		let ranks: Vec<usize> = match self {
			Self::Module | Self::Mastery => (0..current).rev().collect(),
			Self::Level => vec![59, 29, 0],
			Self::Trust => vec![100, 50, 0],
		};
		ranks
			.into_iter()
			.filter(|rank| *rank < current)
			.map(|rank| self.at(base, rank))
			.collect()
	}
}

/// Every mastery and module pair up to `top`'s, lowest sum first, then lower mastery first.
fn mechanics_pairs(top: Training) -> Vec<Training> {
	let mut pairs: Vec<Training> = (0..=top.mastery)
		.flat_map(|mastery| {
			(0..=top.module).map(move |module| Training {
				mastery,
				module,
				..top
			})
		})
		.collect();
	pairs.sort_by_key(|training| (training.mastery + training.module, training.mastery));
	pairs
}

/// Level and trust lowered together from `top`: step 0 is their lowest values and the last
/// step is `top`.
fn stats(top: Training, step: usize) -> Training {
	[Axis::Level, Axis::Trust]
		.into_iter()
		.fold(top, |training, axis| {
			axis.at(
				training,
				(axis.rank(&top) * step + STAT_STEPS / 2) / STAT_STEPS,
			)
		})
}

/// The lowest rank in `0..=high` that passes, given that `high` passes and that a higher rank
/// never loses where a lower one wins. The floor is tried first, since lowering often reaches it.
fn lowest_passing(high: usize, mut passes: impl FnMut(usize) -> Result<bool>) -> Result<usize> {
	if high == 0 || passes(0)? {
		return Ok(0);
	}
	let (mut low, mut high): (usize, usize) = (1, high);
	while low < high {
		let middle: usize = (low + high) / 2;
		if passes(middle)? {
			high = middle;
		} else {
			low = middle + 1;
		}
	}
	Ok(high)
}

/// The tick each program step was issued at, from the probe's records.
fn step_ticks(observer: &Path) -> Result<Value> {
	let mut ticks: serde_json::Map<String, Value> = serde_json::Map::new();
	let mut step: Option<i64> = None;
	for line in fs::read_to_string(observer)?.lines() {
		let Ok(event) = serde_json::from_str::<Value>(line) else {
			continue;
		};
		match event["stage"].as_str() {
			Some("original_step_issued") => step = event["value"].as_i64(),
			Some("original_step_issued_tick") => {
				if let Some(index) = step.take() {
					ticks.insert(index.to_string(), event["value"].clone());
				}
			}
			_ => {}
		}
	}
	Ok(Value::Object(ticks))
}

fn squad_tables(tables: &Value) -> squad::Tables<'_> {
	squad::Tables {
		characters: &tables["characters"],
		modules: &tables["modules"],
		favor: &tables["favor"],
	}
}

fn squad_training_json(training: &SquadTraining) -> Value {
	let operators: serde_json::Map<String, Value> = training
		.operators
		.iter()
		.map(|(id, training)| (id.clone(), training_json(training)))
		.collect();
	json!({"team": training_json(&training.team), "operators": operators})
}

fn training_json(training: &Training) -> Value {
	json!({"level":training.level.map_or(json!("max"), |level| json!(level)),"mastery":training.mastery,
		"module":training.module,"trust":training.trust.map_or(json!("max"), |trust| json!(trust))})
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn invalid_budgets_fail_before_preparation() {
		let options = RunOptions {
			resource_rounds: 0,
			..Default::default()
		};
		assert!(options.validate().is_err());
	}
	#[test]
	fn stage_codes_resolve_through_original_table() {
		let assets = BTreeMap::from([
			("gamedata/levels/a/level_foo".to_owned(), "a.ab".to_owned()),
			("gamedata/levels/a/level_bar".to_owned(), "b.ab".to_owned()),
		]);
		let table = json!({"stages":{"some_original_id":{"code":"EV-3","levelId":"a/level_foo"}}});
		assert_eq!(
			resolve_stage(&json!("ev-3"), &table, &assets).unwrap()["level_id"],
			"a/level_foo"
		);
		assert!(resolve_stage(&json!("1-1"), &table, &assets).is_err());
	}
	#[test]
	fn bisection_finds_the_lowest_passing_rank_trying_the_floor_first() {
		for threshold in 0..=20 {
			let mut tried: Vec<usize> = Vec::new();
			let rank: usize = lowest_passing(20, |rank| {
				tried.push(rank);
				Ok(rank >= threshold)
			})
			.unwrap();
			assert_eq!(rank, threshold);
			assert_eq!(tried[0], 0);
			assert!(tried.len() <= 6, "{threshold}: {tried:?}");
		}
	}
	#[test]
	fn axes_stats_and_mechanics_span_lowest_to_top() {
		let top: Training = Training::default();
		assert_eq!(stats(top, STAT_STEPS), top);
		assert_eq!(
			stats(top, 0),
			Training {
				level: Some(1),
				trust: Some(0),
				..top
			}
		);
		let pairs: Vec<(i64, i64)> = mechanics_pairs(Training {
			mastery: 1,
			module: 2,
			..top
		})
		.iter()
		.map(|training| (training.mastery, training.module))
		.collect();
		assert_eq!(pairs, [(0, 0), (0, 1), (1, 0), (0, 2), (1, 1), (1, 2)]);
		for axis in AXES {
			let top: usize = axis.rank(&Training::default());
			for rank in 0..=top {
				assert_eq!(axis.rank(&axis.at(Training::default(), rank)), rank);
			}
		}
		let levels: Vec<Option<i64>> = Axis::Level
			.tiers(Training::default())
			.iter()
			.map(|training| training.level)
			.collect();
		assert_eq!(levels, [Some(60), Some(30), Some(1)]);
	}
}
