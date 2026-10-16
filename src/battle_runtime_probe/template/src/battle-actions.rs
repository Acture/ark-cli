// A battle program: ordered steps, each an operation behind conditions met in order, and skill
// policies applied every tick. Readiness and effects use original methods only; no DP, SP, HP or
// terminal state is written.
const KIND_SPAWN: u32 = 0;
const KIND_WITHDRAW: u32 = 1;
// Press the skill button: cast a ready skill, or suspend an active, suspendable one.
const KIND_SKILL: u32 = 2;
const KIND_SPEED_UP: u32 = 3;
const KIND_SKILL_DAEMON: u32 = 4;
const KIND_SKILL_USAGE: u32 = 5;
const KIND_CAST: u32 = 6;
const KIND_SUSPEND: u32 = 7;
const CONDITION_KILLS: u32 = 1;
const CONDITION_COST: u32 = 2;
// Cost change since the step became current: a positive value waits for a rise of at least that
// much, a negative one for a fall of at least that much.
const CONDITION_COST_CHANGE: u32 = 3;
const CONDITION_COOLING: u32 = 4;
const CONDITION_TICK: u32 = 5;
// Ticks since the previous condition was met (or since the step became current): a delay.
const CONDITION_WAIT: u32 = 6;
// The last condition only: once the other conditions are met, the operation must stay ready this
// many ticks before it is issued, restarting whenever it is not ready. A declared latency.
const CONDITION_HOLD: u32 = 7;
const MAX_CONDITIONS: usize = 8;
const MAX_ACTIONS: usize = 128;
const MAX_POLICIES: usize = 12;
const CARD_RESPAWNING: i32 = 3;
// Ticks a queued skill may take to start, for example while its begin animation plays.
const SKILL_START_WINDOW: u32 = 90;

#[derive(Clone, Copy)]
struct ProbeAction {
	tick: u32,
	kind: u32,
	// (kind, value) pairs, met in order: a met condition is not checked again.
	conditions: [(u32, i32); MAX_CONDITIONS],
	condition_count: usize,
	row: i32,
	col: i32,
	direction: i32,
	usage: u32,
	times: u32,
	// Skill/Retreat named with a tile: the unit must stand there when the command runs.
	located: bool,
	id: [u8; 96],
}

const EMPTY_ACTION: ProbeAction = ProbeAction {
	tick: 0,
	kind: 0,
	conditions: [(0, 0); MAX_CONDITIONS],
	condition_count: 0,
	row: 0,
	col: 0,
	direction: 0,
	usage: 0,
	times: 0,
	located: false,
	id: [0; 96],
};

// MAA skill_usage: 1 casts whenever ready; 2 casts `remaining` more times.
#[derive(Clone, Copy)]
struct SkillPolicy {
	usage: u32,
	remaining: u32,
	id: [u8; 96],
}

const EMPTY_POLICY: SkillPolicy = SkillPolicy {
	usage: 0,
	remaining: 0,
	id: [0; 96],
};

static mut PROBE_ACTIONS: [ProbeAction; MAX_ACTIONS] = [EMPTY_ACTION; MAX_ACTIONS];
static mut SKILL_POLICIES: [SkillPolicy; MAX_POLICIES] = [EMPTY_POLICY; MAX_POLICIES];
// Ticks a policy skill must stay ready before it is cast: the declared latency of `D`.
static POLICY_DELAY: AtomicU32 = AtomicU32::new(0);
// The tick each policy's skill became ready, u32::MAX while it is not ready.
static mut POLICY_READY_SINCE: [u32; MAX_POLICIES] = [u32::MAX; MAX_POLICIES];
static ACTION_COUNT: AtomicU32 = AtomicU32::new(0);
static POLICY_COUNT: AtomicU32 = AtomicU32::new(0);
static NEXT_ACTION: AtomicU32 = AtomicU32::new(0);
static VERIFIED_ACTIONS: AtomicU32 = AtomicU32::new(0);
static AUTO_SKILLS: AtomicU32 = AtomicU32::new(0);
// Cost when the current step started, and how many of its conditions are already met.
static BASE_INDEX: AtomicU32 = AtomicU32::new(u32::MAX);
static BASE_COST: AtomicU32 = AtomicU32::new(0);
static CONDITION_CURSOR: AtomicU32 = AtomicU32::new(0);
// The tick the current condition became the next one to meet.
static CONDITION_SINCE: AtomicU32 = AtomicU32::new(0);

struct ActionProof {
	trigger: u32,
	// The skill was active when clicked, so the click suspends it.
	suspend: bool,
	character: Option<Rooted>,
}

// The original skill button is clickable: ready to cast, or active and manually suspendable.
unsafe fn skill_clickable(api: &Api, game: Pointer, skill: Pointer) -> Result<bool, &'static str> {
	let skill_class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"BasicSkill\0")?;
	let mut side: i32 = 0;
	let boxed: Pointer = typed_call(
		api,
		skill_class,
		skill,
		b"IsClickable\0",
		&[b"Torappu.PlayerSide\0"],
		&mut [(&mut side as *mut i32).cast()],
	)?;
	let _root: Rooted = api.root(boxed)?;
	Ok((api.unbox)(boxed).cast::<u8>().read() != 0)
}

// Whether a skill operation of this kind can be issued now.
unsafe fn skill_ready(
	api: &Api,
	game: Pointer,
	skill: Pointer,
	kind: u32,
) -> Result<bool, &'static str> {
	let skill_class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"BasicSkill\0")?;
	match kind {
		KIND_SKILL => skill_clickable(api, game, skill),
		KIND_SUSPEND => Ok(managed_bool(api, skill_class, skill, b"get_isAffecting\0")?
			&& managed_bool(api, skill_class, skill, b"IsSuspendable\0")?),
		// Not get_canCastCheck: for targeted skills it runs the original target search, which
		// resets that search's timer, so a probe call would change the battle. Only the
		// original CastSkill.OnTick calls it, once per tick.
		_ => Ok(!managed_bool(api, skill_class, skill, b"get_isAffecting\0")?
			&& skill_clickable(api, game, skill)?),
	}
}

fn skill_kind(kind: u32) -> bool {
	matches!(kind, KIND_SKILL | KIND_CAST | KIND_SUSPEND)
}

enum Pending {
	Ordered(ProbeAction, ActionProof),
	Automatic(usize, ProbeAction, ActionProof),
}

// Operations queued but whose effect is not yet observed, with their queue tick. Like MAA, the
// job moves on once an operation is issued; effects are confirmed per unit in the background.
const MAX_IN_FLIGHT: usize = 8;
static mut IN_FLIGHT: [Option<(Pending, u32)>; MAX_IN_FLIGHT] = [const { None }; MAX_IN_FLIGHT];
// The next ordered command to issue. NEXT_ACTION counts completed (confirmed) commands.
static ORDER_CURSOR: AtomicU32 = AtomicU32::new(0);

unsafe fn initialize_probe_seed(
	api: &Api,
	module: Pointer,
	game: Pointer,
	controller: Pointer,
) -> Result<(), &'static str> {
	let text: Pointer = environment_string(module, b"ARK_RUNTIME_BATTLE_SEED")?;
	let _text_root: Rooted = api.root(text)?;
	let chars = level_bind!(
		module,
		b"il2cpp_string_chars\0",
		unsafe extern "C" fn(Pointer) -> *const u16
	);
	let length = level_bind!(
		module,
		b"il2cpp_string_length\0",
		unsafe extern "C" fn(Pointer) -> i32
	);
	let size: i32 = length(text);
	if size <= 0 || size > 10 {
		return Err("probe_seed_invalid");
	}
	let mut seed: u32 = 0;
	for index in 0..size {
		let ch: u16 = chars(text).offset(index as isize).read();
		if !(48..=57).contains(&ch) {
			return Err("probe_seed_invalid");
		}
		let digit: u32 = (ch - 48) as u32;
		if seed > (2147483647 - digit) / 10 {
			return Err("probe_seed_out_of_range");
		}
		seed = seed * 10 + digit;
	}
	let class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"BattleController\0")?;
	if api.integer(class, core::ptr::null_mut(), b"get_fixedFrameCnt\0")? != 0 {
		return Err("probe_seed_initialization_too_late");
	}
	let mut value: i32 = seed as i32;
	api.call(
		class,
		controller,
		b"ResetSeed\0",
		&mut [(&mut value as *mut i32).cast()],
	)?;
	if api.integer(class, controller, b"get_randomSeed\0")? != value {
		return Err("probe_seed_not_bound");
	}
	scene_number("original_battle_seed", seed);
	Ok(())
}

// One "A" (command) or "P" (initial skill policy) line: a tag, signed integers and an
// identifier, separated by '|'. Returns the tag, the integers and the identifier length.
unsafe fn parse_command_line(
	data: *const u16,
	start: i32,
	end: i32,
	numbers: &mut [i32; 32],
	id: &mut [u8; 96],
) -> Result<(u16, usize, usize), &'static str> {
	if end - start < 3 || data.offset(start as isize + 1).read() != 124 {
		return Err("probe_action_shape_invalid");
	}
	let tag: u16 = data.offset(start as isize).read();
	let mut count: usize = 0;
	let mut index: i32 = start + 2;
	loop {
		let mut field_end: i32 = index;
		while field_end < end && data.offset(field_end as isize).read() != 124 {
			field_end += 1;
		}
		if field_end == end {
			// The final field is the identifier.
			let mut length: usize = 0;
			for position in index..end {
				let ch: u16 = data.offset(position as isize).read();
				if length >= 95
					|| !((48..=57).contains(&ch)
						|| (65..=90).contains(&ch)
						|| (97..=122).contains(&ch)
						|| ch == 95)
				{
					return Err("probe_action_id_invalid");
				}
				id.as_mut_ptr().add(length).write(ch as u8);
				length += 1;
			}
			if length == 0 {
				return Err("probe_action_id_invalid");
			}
			return Ok((tag, count, length));
		}
		if count >= numbers.len() || field_end == index {
			return Err("probe_action_number_invalid");
		}
		let mut negative: bool = false;
		let mut position: i32 = index;
		if data.offset(position as isize).read() == 45 {
			negative = true;
			position += 1;
		}
		if field_end - position == 0 || field_end - position > 5 {
			return Err("probe_action_number_invalid");
		}
		let mut value: i32 = 0;
		while position < field_end {
			let ch: u16 = data.offset(position as isize).read();
			if !(48..=57).contains(&ch) {
				return Err("probe_action_number_invalid");
			}
			value = value * 10 + (ch - 48) as i32;
			position += 1;
		}
		numbers.as_mut_ptr().add(count).write(if negative { -value } else { value });
		count += 1;
		index = field_end + 1;
	}
}

unsafe fn load_probe_actions(
	api: &Api,
	module: Pointer,
	domain: Pointer,
) -> Result<(), &'static str> {
	let path: Pointer = environment_string(module, b"ARK_RUNTIME_BATTLE_ACTIONS")?;
	let _path_root: Rooted = api.root(path)?;
	let core: Pointer = (api.image)((api.assembly)(domain, b"mscorlib\0".as_ptr().cast()));
	let file: Pointer = api.class_named(core, b"System.IO\0", b"File\0")?;
	let text: Pointer = api.call(file, core::ptr::null_mut(), b"ReadAllText\0", &mut [path])?;
	let _text_root: Rooted = api.root(text)?;
	let chars = level_bind!(
		module,
		b"il2cpp_string_chars\0",
		unsafe extern "C" fn(Pointer) -> *const u16
	);
	let length = level_bind!(
		module,
		b"il2cpp_string_length\0",
		unsafe extern "C" fn(Pointer) -> i32
	);
	let size: i32 = length(text);
	if size <= 5 || size > 40000 {
		return Err("probe_action_text_size_invalid");
	}
	let data: *const u16 = chars(text);
	// "ARKP1\n"
	let header: [u16; 6] = [65, 82, 75, 80, 49, 10];
	for (index, expected) in header.iter().enumerate() {
		if data.add(index).read() != *expected {
			return Err("probe_action_header_invalid");
		}
	}
	record("original_maa_ordered_execution");
	let mut actions: u32 = 0;
	let mut policies: u32 = 0;
	let mut start: i32 = 6;
	for end in start..size {
		if data.offset(end as isize).read() != 10 {
			continue;
		}
		let mut numbers: [i32; 32] = [0; 32];
		let mut id: [u8; 96] = [0; 96];
		let (tag, count, _) = parse_command_line(data, start, end, &mut numbers, &mut id)?;
		if tag == 68 {
			// D|ticks|_ : declared latency of policy casts.
			if count != 1 || !(0..=900).contains(&numbers.as_ptr().read()) {
				return Err("probe_delay_invalid");
			}
			POLICY_DELAY.store(numbers.as_ptr().read() as u32, Ordering::Release);
		} else if tag == 83 {
			// S|usage|times|id
			if count != 2 || policies as usize >= MAX_POLICIES {
				return Err("probe_policy_shape_invalid");
			}
			let usage: i32 = numbers[0];
			let times: i32 = numbers[1];
			if !(1..=2).contains(&usage) || !(1..=99).contains(&times) {
				return Err("probe_policy_value_invalid");
			}
			core::ptr::addr_of_mut!(SKILL_POLICIES)
				.cast::<SkillPolicy>()
				.add(policies as usize)
				.write(SkillPolicy {
					usage: usage as u32,
					remaining: times as u32,
					id,
				});
			policies += 1;
		} else if tag == 84 {
			// T|kind|row|col|direction|usage|times|located|n|(condition kind|value)*n|id
			let conditions: usize =
				numbers.as_ptr().add(7).read().clamp(0, MAX_CONDITIONS as i32 + 1) as usize;
			if count < 8
				|| conditions > MAX_CONDITIONS
				|| count != 8 + 2 * conditions
				|| actions as usize >= MAX_ACTIONS
			{
				return Err("probe_action_count_or_line_invalid");
			}
			let mut action: ProbeAction = ProbeAction {
				tick: 0,
				kind: numbers[0] as u32,
				conditions: [(0, 0); MAX_CONDITIONS],
				condition_count: conditions,
				row: numbers[1],
				col: numbers[2],
				direction: numbers[3],
				usage: numbers[4] as u32,
				times: numbers[5] as u32,
				located: numbers[6] == 1,
				id,
			};
			for index in 0..conditions {
				let kind: i32 = numbers.as_ptr().add(8 + 2 * index).read();
				let value: i32 = numbers.as_ptr().add(9 + 2 * index).read();
				let valid: bool = match kind as u32 {
					CONDITION_KILLS | CONDITION_COST | CONDITION_TICK | CONDITION_WAIT => value >= 0,
					CONDITION_HOLD => index + 1 == conditions && (0..=900).contains(&value),
					CONDITION_COST_CHANGE => value != 0,
					CONDITION_COOLING => (0..=99).contains(&value),
					_ => false,
				};
				if kind < 0 || !valid {
					return Err("probe_condition_invalid");
				}
				action
					.conditions
					.as_mut_ptr()
					.add(index)
					.write((kind as u32, value));
			}
			if numbers[0] < 0
				|| action.kind > KIND_SUSPEND
				|| !(0..64).contains(&action.row)
				|| !(0..64).contains(&action.col)
				|| !(0..=3).contains(&action.direction)
				|| action.usage > 2
				|| action.times > 99
				|| !(0..=1).contains(&numbers[6])
			{
				return Err("probe_action_value_invalid");
			}
			core::ptr::addr_of_mut!(PROBE_ACTIONS)
				.cast::<ProbeAction>()
				.add(actions as usize)
				.write(action);
			actions += 1;
		} else {
			return Err("probe_action_tag_invalid");
		}
		start = end + 1;
	}
	if start != size || actions == 0 {
		return Err("probe_action_trailing_or_empty_input");
	}
	ACTION_COUNT.store(actions, Ordering::Release);
	POLICY_COUNT.store(policies, Ordering::Release);
	scene_number("original_probe_action_count", actions);
	scene_number("original_maa_skill_policy_count", policies);
	Ok(())
}

unsafe fn active_character(
	api: &Api,
	game: Pointer,
	controller: Pointer,
	name: Pointer,
) -> Result<Pointer, &'static str> {
	let class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"BattleController\0")?;
	let manager: Pointer = api.call(class, controller, b"get_unitManager\0", &mut [])?;
	let manager_class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"UnitManager\0")?;
	api.call(manager_class, manager, b"GetCharacterById\0", &mut [name])
}

// The unit with this original id standing on a tile. Summons from one card share an id, so a
// tile, not the id alone, identifies which one a command targets. Null when none stands there.
unsafe fn character_on_tile(
	api: &Api,
	module: Pointer,
	game: Pointer,
	controller: Pointer,
	name: Pointer,
	row: i32,
	col: i32,
) -> Result<Pointer, &'static str> {
	let class_of = level_bind!(
		module,
		b"il2cpp_object_get_class\0",
		unsafe extern "C" fn(Pointer) -> Pointer
	);
	let class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"BattleController\0")?;
	let manager: Pointer = api.call(class, controller, b"get_unitManager\0", &mut [])?;
	let manager_class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"UnitManager\0")?;
	let mut all_sides: i32 = 7;
	let units: Pointer = api.call(
		manager_class,
		manager,
		b"GetCharactersById_DISPOSE\0",
		&mut [name, (&mut all_sides as *mut i32).cast()],
	)?;
	if units.is_null() {
		return Ok(core::ptr::null_mut());
	}
	let _units_root: Rooted = api.root(units)?;
	let units_class: Pointer = class_of(units);
	let count: u32 = number_result(api, api.call(units_class, units, b"get_Count\0", &mut [])?)?;
	let visual: Pointer = api.class_named(game, b"Torappu.Battle\0", b"VisualObject\0")?;
	let mut found: Pointer = core::ptr::null_mut();
	for offset in 0..count.min(64) {
		let mut index: i32 = offset as i32;
		let unit: Pointer = api.call(
			units_class,
			units,
			b"get_Item\0",
			&mut [(&mut index as *mut i32).cast()],
		)?;
		let pos: Pointer = api.call(visual, unit, b"get_gridPosition\0", &mut [])?;
		let position: *const i32 = (api.unbox)(pos).cast();
		if position.read() == row && position.add(1).read() == col {
			found = unit;
			break;
		}
	}
	// The list comes from an original pool; return it as its _DISPOSE contract requires.
	api.call(units_class, units, b"Dispose\0", &mut [])?;
	Ok(found)
}

unsafe fn queue_probe_action(
	api: &Api,
	module: Pointer,
	game: Pointer,
	controller: Pointer,
	frame: Pointer,
	action: ProbeAction,
) -> Result<ActionProof, &'static str> {
	let name: Pointer = managed_literal(module, &action.id)?;
	let _name_root: Rooted = api.root(name)?;
	let class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"BattleController\0")?;
	let mut side: i32 = 0;
	let mut proof: ActionProof = ActionProof {
		trigger: 0,
		suspend: false,
		character: None,
	};
	let data: Pointer = if action.kind == KIND_SPAWN {
		let deck: Pointer = typed_call(
			api,
			class,
			controller,
			b"GetDeck\0",
			&[b"Torappu.PlayerSide\0"],
			&mut [(&mut side as *mut i32).cast()],
		)?;
		if deck.is_null() {
			return Err("deployment_deck_missing");
		}
		let deck_class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"Deck\0")?;
		let card: Pointer = api.call(deck_class, deck, b"FindCardById\0", &mut [name])?;
		if card.is_null() {
			return Err("deployment_card_not_found");
		}
		let card_class: Pointer = nested_class(module, deck_class, b"Card\0")?;
		if !managed_bool(api, card_class, card, b"get_readyToSpawn\0")? {
			return Err("deployment_card_not_ready");
		}
		let cost: u32 = number_result(api, api.call(card_class, card, b"get_cost\0", &mut [])?)?;
		scene_number("original_deployment_card_cost", cost);
		api.call(card_class, card, b"get_data\0", &mut [])?
	} else {
		let character: Pointer =
			character_on_tile(api, module, game, controller, name, action.row, action.col)?;
		if character.is_null() {
			return Err("probe_action_character_not_deployed");
		}
		let character_class: Pointer =
			api.class_named(game, b"Torappu.Battle\0", b"Character\0")?;
		proof.character = Some(api.root(character)?);
		if action.kind == KIND_WITHDRAW
			&& !managed_bool(
				api,
				character_class,
				character,
				b"get_manuallyWithdrawable\0",
			)? {
			return Err("probe_action_withdraw_not_ready");
		}
		if skill_kind(action.kind) {
			let skill: Pointer = api.call(character_class, character, b"get_skill\0", &mut [])?;
			if skill.is_null() {
				return Err("probe_action_character_has_no_skill");
			}
			let skill_class: Pointer =
				api.class_named(game, b"Torappu.Battle\0", b"BasicSkill\0")?;
			if !skill_ready(api, game, skill, action.kind)? {
				return Err("probe_action_skill_not_ready");
			}
			proof.suspend = managed_bool(api, skill_class, skill, b"get_isAffecting\0")?;
			proof.trigger = number_result(
				api,
				api.call(skill_class, skill, b"get_triggerCnt\0", &mut [])?,
			)?;
		}
		api.call(character_class, character, b"get_data\0", &mut [])?
	};
	let data_class: Pointer =
		api.class_named(game, b"Torappu.Battle\0", b"BattleCharacterData\0")?;
	let signature: Pointer = api.call(data_class, data, b"TakeSigniture\0", &mut [])?;
	let _signature_root: Rooted = api.root(signature)?;
	let grid_class: Pointer = api.class_named(game, b"Torappu\0", b"GridPosition\0")?;
	let grid: Pointer = (api.new_object)(grid_class);
	let _grid_root: Rooted = api.root(grid)?;
	let mut row: i32 = action.row;
	let mut col: i32 = action.col;
	api.call(
		grid_class,
		(api.unbox)(grid),
		b".ctor\0",
		&mut [(&mut row as *mut i32).cast(), (&mut col as *mut i32).cast()],
	)?;
	let frame_class: Pointer = nested_class(module, class, b"FrameData\0")?;
	let mut direction: i32 = action.direction;
	if action.kind == KIND_SPAWN {
		api.call(
			frame_class,
			frame,
			b"AppendSpawn\0",
			&mut [
				(api.unbox)(signature),
				(&mut direction as *mut i32).cast(),
				(api.unbox)(grid),
				(&mut side as *mut i32).cast(),
			],
		)?;
	} else {
		api.call(
			frame_class,
			frame,
			if action.kind == KIND_WITHDRAW {
				b"AppendWithdraw\0"
			} else {
				b"AppendSkill\0"
			},
			&mut [
				(api.unbox)(signature),
				(api.unbox)(grid),
				(&mut side as *mut i32).cast(),
			],
		)?;
	}
	scene_number("original_probe_action_kind", action.kind);
	scene_number("original_probe_action_tick", action.tick);
	managed_ascii(module, name, "original_probe_action_character")?;
	record("original_probe_action_queued");
	Ok(proof)
}

// Confirm the queued operation took effect in the original state after the tick.
unsafe fn confirm_effect(
	api: &Api,
	module: Pointer,
	game: Pointer,
	controller: Pointer,
	action: &ProbeAction,
	proof: &ActionProof,
) -> Result<bool, &'static str> {
	let name: Pointer = managed_literal(module, &action.id)?;
	let _name_root: Rooted = api.root(name)?;
	if action.kind == KIND_WITHDRAW {
		let target = level_bind!(
			module,
			b"il2cpp_gchandle_get_target\0",
			unsafe extern "C" fn(u32) -> Pointer
		);
		let root: &Rooted = proof
			.character
			.as_ref()
			.ok_or("probe_withdrawal_reference_missing")?;
		let entity: Pointer = api.class_named(game, b"Torappu.Battle\0", b"Entity\0")?;
		let reason: i32 = api.integer(entity, target(root.handle), b"get_finishReason\0")?;
		scene_number("original_probe_withdraw_finish_reason", reason as u32);
		if reason != 4 {
			return Err("probe_withdrawal_reason_not_observed");
		}
		return Ok(true);
	}
	let character: Pointer =
		character_on_tile(api, module, game, controller, name, action.row, action.col)?;
	if character.is_null() {
		return Err("probe_action_grid_mismatch");
	}
	if skill_kind(action.kind) {
		let character_class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"Character\0")?;
		let skill: Pointer = api.call(character_class, character, b"get_skill\0", &mut [])?;
		let skill_class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"BasicSkill\0")?;
		let trigger_after: u32 = number_result(
			api,
			api.call(skill_class, skill, b"get_triggerCnt\0", &mut [])?,
		)?;
		let affecting: bool = managed_bool(api, skill_class, skill, b"get_isAffecting\0")?;
		// A click on an active skill suspends it. Otherwise a chant or a begin animation can
		// delay the trigger; the original chant or affecting state also shows the cast.
		let started: bool = if proof.suspend {
			!affecting
		} else {
			trigger_after > proof.trigger
				|| affecting
				|| managed_bool(api, skill_class, skill, b"get_isInChant\0")?
		};
		if !started {
			return Ok(false);
		}
		if proof.suspend {
			scene_number("original_probe_skill_suspended", trigger_after);
		}
		scene_number("original_probe_skill_trigger_before", proof.trigger);
		scene_number("original_probe_skill_trigger_after", trigger_after);
	}
	Ok(true)
}

unsafe fn complete_command() {
	let completed: u32 = NEXT_ACTION.fetch_add(1, Ordering::AcqRel) + 1;
	scene_number("original_maa_completed_action_count", completed);
}

unsafe fn verify_pending(
	api: &Api,
	module: Pointer,
	game: Pointer,
	controller: Pointer,
	pending: &Pending,
) -> Result<bool, &'static str> {
	match pending {
		Pending::Ordered(action, proof) => {
			if !confirm_effect(api, module, game, controller, action, proof)? {
				return Ok(false);
			}
			let verified: u32 = VERIFIED_ACTIONS.fetch_add(1, Ordering::AcqRel) + 1;
			scene_number("original_probe_verified_action_count", verified);
			record("original_probe_action_verified");
			complete_command();
		}
		Pending::Automatic(index, action, proof) => {
			if !confirm_effect(api, module, game, controller, action, proof)? {
				return Ok(false);
			}
			let policy: *mut SkillPolicy = core::ptr::addr_of_mut!(SKILL_POLICIES)
				.cast::<SkillPolicy>()
				.add(*index);
			if (*policy).usage == 2 {
				(*policy).remaining -= 1;
			}
			let count: u32 = AUTO_SKILLS.fetch_add(1, Ordering::AcqRel) + 1;
			scene_number("original_maa_auto_skill_count", count);
			record("original_maa_auto_skill_verified");
		}
	}
	Ok(true)
}

unsafe fn in_flight() -> *mut Option<(Pending, u32)> {
	core::ptr::addr_of_mut!(IN_FLIGHT).cast::<Option<(Pending, u32)>>()
}

// Whether a skill of the unit on this tile was issued and has not visibly started yet. A skill
// in its begin animation or chant still reports castable, so it must not be issued again.
unsafe fn skill_in_flight(id: &[u8; 96], row: i32, col: i32) -> bool {
	(0..MAX_IN_FLIGHT).any(|slot| match &*in_flight().add(slot) {
		Some((Pending::Ordered(action, _) | Pending::Automatic(_, action, _), _)) => {
			skill_kind(action.kind)
				&& action.row == row
				&& action.col == col
				&& (0..96).all(|offset| action.id[offset] == id[offset])
		}
		None => false,
	})
}

// Issue this tick's operation, if any, while earlier effects are still being confirmed.
unsafe fn plan_in_flight(
	api: &Api,
	module: Pointer,
	domain: Pointer,
	game: Pointer,
	controller: Pointer,
	frame: Pointer,
	tick: u32,
) -> Result<(), &'static str> {
	let Some(free) = (0..MAX_IN_FLIGHT).find(|slot| (*in_flight().add(*slot)).is_none()) else {
		return Ok(());
	};
	if let Some(pending) = plan_tick(api, module, domain, game, controller, frame, tick)? {
		*in_flight().add(free) = Some((pending, tick));
	}
	Ok(())
}

// After a tick: confirm each in-flight effect once observed, failing after its window.
unsafe fn settle_in_flight(
	api: &Api,
	module: Pointer,
	game: Pointer,
	controller: Pointer,
	tick: u32,
) -> Result<(), &'static str> {
	for slot in 0..MAX_IN_FLIGHT {
		let entry: *mut Option<(Pending, u32)> = in_flight().add(slot);
		let Some((pending, queued)) = (*entry).as_ref() else {
			continue;
		};
		let queued: u32 = *queued;
		if verify_pending(api, module, game, controller, pending)? {
			scene_number("original_probe_effect_delay_ticks", tick - queued);
			*entry = None;
		} else if tick - queued >= SKILL_START_WINDOW {
			return Err("probe_skill_trigger_not_observed");
		}
	}
	Ok(())
}

unsafe fn card_cooling_count(
	api: &Api,
	module: Pointer,
	domain: Pointer,
	game: Pointer,
	controller: Pointer,
) -> Result<i32, &'static str> {
	let class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"BattleController\0")?;
	let mut side: i32 = 0;
	let deck: Pointer = typed_call(
		api,
		class,
		controller,
		b"GetDeck\0",
		&[b"Torappu.PlayerSide\0"],
		&mut [(&mut side as *mut i32).cast()],
	)?;
	let deck_class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"Deck\0")?;
	let cards: Pointer = api.call(deck_class, deck, b"get_cards\0", &mut [])?;
	let card_class: Pointer = nested_class(module, deck_class, b"Card\0")?;
	let length = level_bind!(
		module,
		b"il2cpp_array_length\0",
		unsafe extern "C" fn(Pointer) -> u32
	);
	let core: Pointer = (api.image)((api.assembly)(domain, b"mscorlib\0".as_ptr().cast()));
	let array_class: Pointer = api.class_named(core, b"System\0", b"Array\0")?;
	let mut cooling: i32 = 0;
	for index in 0..length(cards) {
		let card: Pointer = array_item(api, array_class, cards, index as i32)?;
		if !card.is_null() && api.integer(card_class, card, b"get_state\0")? == CARD_RESPAWNING {
			cooling += 1;
		}
	}
	Ok(cooling)
}

// Readiness is observed through original public methods. No DP/SP/HP is written.
// The tick the current step's operation became ready, u32::MAX while it is not ready.
static HOLD_SINCE: AtomicU32 = AtomicU32::new(u32::MAX);

// Readiness of the current step, with its hold applied: conditions met, the operation ready, and
// ready for the held ticks.
unsafe fn ordered_action_ready(
	api: &Api,
	module: Pointer,
	domain: Pointer,
	game: Pointer,
	controller: Pointer,
	index: u32,
	action: &mut ProbeAction,
) -> Result<bool, &'static str> {
	let hold: u32 = match action.condition_count {
		0 => 0,
		count => match action.conditions.as_ptr().add(count - 1).read() {
			(CONDITION_HOLD, value) => value as u32,
			_ => 0,
		},
	};
	if BASE_INDEX.load(Ordering::Acquire) != index {
		HOLD_SINCE.store(u32::MAX, Ordering::Release);
	}
	if !step_ready(api, module, domain, game, controller, index, action)? {
		HOLD_SINCE.store(u32::MAX, Ordering::Release);
		return Ok(false);
	}
	if hold == 0 {
		return Ok(true);
	}
	if HOLD_SINCE.load(Ordering::Acquire) == u32::MAX {
		HOLD_SINCE.store(action.tick, Ordering::Release);
	}
	if action.tick - HOLD_SINCE.load(Ordering::Acquire) < hold {
		return Ok(false);
	}
	HOLD_SINCE.store(u32::MAX, Ordering::Release);
	Ok(true)
}

unsafe fn step_ready(
	api: &Api,
	module: Pointer,
	domain: Pointer,
	game: Pointer,
	controller: Pointer,
	index: u32,
	action: &mut ProbeAction,
) -> Result<bool, &'static str> {
	let class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"BattleController\0")?;
	let mut side: i32 = 0;
	let cost: u32 = number_result(
		api,
		typed_call(
			api,
			class,
			controller,
			b"GetCost\0",
			&[b"Torappu.PlayerSide\0"],
			&mut [(&mut side as *mut i32).cast()],
		)?,
	)?;
	if BASE_INDEX.swap(index, Ordering::AcqRel) != index {
		BASE_COST.store(cost, Ordering::Release);
		CONDITION_CURSOR.store(0, Ordering::Release);
		CONDITION_SINCE.store(action.tick, Ordering::Release);
	}
	// Conditions are met in order; one met is not checked again.
	while (CONDITION_CURSOR.load(Ordering::Acquire) as usize) < action.condition_count {
		let (kind, value): (u32, i32) = action
			.conditions
			.as_ptr()
			.add(CONDITION_CURSOR.load(Ordering::Acquire) as usize)
			.read();
		let met: bool = match kind {
			CONDITION_KILLS => {
				let scheduler: Pointer = api.call(class, controller, b"get_scheduler\0", &mut [])?;
				let scheduler_class: Pointer =
					api.class_named(game, b"Torappu.Battle\0", b"Scheduler\0")?;
				let kills: u32 = number_result(
					api,
					api.call(
						scheduler_class,
						scheduler,
						b"get_killedEnemiesCnt\0",
						&mut [],
					)?,
				)?;
				kills >= value as u32
			}
			CONDITION_COST => cost >= value as u32,
			CONDITION_COST_CHANGE => {
				let change: i64 = cost as i64 - BASE_COST.load(Ordering::Acquire) as i64;
				if value > 0 {
					change >= value as i64
				} else {
					change <= value as i64
				}
			}
			CONDITION_COOLING => {
				card_cooling_count(api, module, domain, game, controller)? == value
			}
			CONDITION_WAIT => {
				action.tick >= CONDITION_SINCE.load(Ordering::Acquire) + value as u32
			}
			// The hold applies after readiness, in ordered_action_ready.
			CONDITION_HOLD => break,
			_ => action.tick >= value as u32,
		};
		if !met {
			return Ok(false);
		}
		scene_number("original_step_condition_met", kind);
		CONDITION_CURSOR.fetch_add(1, Ordering::AcqRel);
		CONDITION_SINCE.store(action.tick, Ordering::Release);
	}
	if matches!(
		action.kind,
		KIND_SPEED_UP | KIND_SKILL_DAEMON | KIND_SKILL_USAGE
	) {
		return Ok(true);
	}
	let name: Pointer = managed_literal(module, &action.id)?;
	let _name_root: Rooted = api.root(name)?;
	if action.kind == KIND_SPAWN {
		let deck: Pointer = typed_call(
			api,
			class,
			controller,
			b"GetDeck\0",
			&[b"Torappu.PlayerSide\0"],
			&mut [(&mut side as *mut i32).cast()],
		)?;
		let deck_class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"Deck\0")?;
		let card: Pointer = api.call(deck_class, deck, b"FindCardById\0", &mut [name])?;
		if card.is_null() {
			return Err("maa_deployment_card_missing");
		}
		let card_class: Pointer = nested_class(module, deck_class, b"Card\0")?;
		let required: u32 =
			number_result(api, api.call(card_class, card, b"get_cost\0", &mut [])?)?;
		if cost < required || !managed_bool(api, card_class, card, b"get_readyToSpawn\0")? {
			return Ok(false);
		}
		let map: Pointer = api.call(class, controller, b"get_map\0", &mut [])?;
		let map_class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"Map\0")?;
		let height: i32 = api.integer(map_class, map, b"get_height\0")?;
		let width: i32 = api.integer(map_class, map, b"get_width\0")?;
		if height <= 0 || width <= 0 || action.row >= height || action.col >= width {
			return Err("maa_grid_outside_original_map");
		}
		scene_number("original_maa_screen_row", action.row as u32);
		action.row = height - 1 - action.row;
		scene_number("original_maa_native_row", action.row as u32);
		return Ok(true);
	}
	let mut expected: Option<(i32, i32)> = None;
	if action.located {
		let map: Pointer = api.call(class, controller, b"get_map\0", &mut [])?;
		let map_class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"Map\0")?;
		let height: i32 = api.integer(map_class, map, b"get_height\0")?;
		if action.row >= height {
			return Err("maa_grid_outside_original_map");
		}
		expected = Some((height - 1 - action.row, action.col));
	}
	character_action_ready(api, module, game, controller, name, expected, action)
}

// Withdraw or skill readiness of a deployed operator; on success the action takes its grid.
unsafe fn character_action_ready(
	api: &Api,
	module: Pointer,
	game: Pointer,
	controller: Pointer,
	name: Pointer,
	expected: Option<(i32, i32)>,
	action: &mut ProbeAction,
) -> Result<bool, &'static str> {
	let character: Pointer = match expected {
		Some((row, col)) => {
			let located: Pointer =
				character_on_tile(api, module, game, controller, name, row, col)?;
			if located.is_null() && !active_character(api, game, controller, name)?.is_null() {
				return Err("maa_operator_not_at_location");
			}
			located
		}
		None => active_character(api, game, controller, name)?,
	};
	if character.is_null() {
		return Ok(false);
	}
	let character_class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"Character\0")?;
	let ready: bool = if action.kind == KIND_WITHDRAW {
		managed_bool(
			api,
			character_class,
			character,
			b"get_manuallyWithdrawable\0",
		)?
	} else {
		let skill: Pointer = api.call(character_class, character, b"get_skill\0", &mut [])?;
		if skill.is_null() {
			return Err("maa_character_has_no_skill");
		}
		skill_ready(api, game, skill, action.kind)?
	};
	if ready {
		let visual: Pointer = api.class_named(game, b"Torappu.Battle\0", b"VisualObject\0")?;
		let pos: Pointer = api.call(visual, character, b"get_gridPosition\0", &mut [])?;
		let position: *const i32 = (api.unbox)(pos).cast();
		action.row = position.read();
		action.col = position.add(1).read();
	}
	Ok(ready)
}

// The first policy operator whose original skill is ready; at most one cast per tick.
unsafe fn automatic_skill(
	api: &Api,
	module: Pointer,
	game: Pointer,
	controller: Pointer,
	tick: u32,
) -> Result<Option<(usize, ProbeAction)>, &'static str> {
	let since: *mut u32 = core::ptr::addr_of_mut!(POLICY_READY_SINCE).cast::<u32>();
	for index in 0..POLICY_COUNT.load(Ordering::Acquire) as usize {
		let policy: SkillPolicy = core::ptr::addr_of!(SKILL_POLICIES)
			.cast::<SkillPolicy>()
			.add(index)
			.read();
		if policy.usage == 0 || (policy.usage == 2 && policy.remaining == 0) {
			continue;
		}
		// A policy only starts a ready skill; it never suspends one.
		let mut action: ProbeAction = EMPTY_ACTION;
		action.kind = KIND_CAST;
		action.id = policy.id;
		let name: Pointer = managed_literal(module, &action.id)?;
		let _name_root: Rooted = api.root(name)?;
		if character_action_ready(api, module, game, controller, name, None, &mut action)?
			&& !skill_in_flight(&action.id, action.row, action.col)
		{
			// With a declared latency the skill must stay ready that long before the cast.
			if since.add(index).read() == u32::MAX {
				since.add(index).write(tick);
			}
			if tick - since.add(index).read() >= POLICY_DELAY.load(Ordering::Acquire) {
				since.add(index).write(u32::MAX);
				return Ok(Some((index, action)));
			}
		} else {
			since.add(index).write(u32::MAX);
		}
	}
	Ok(None)
}

// MAA SkillUsage replaces one operator's policy for the rest of the job.
unsafe fn apply_skill_usage(action: &ProbeAction) -> Result<(), &'static str> {
	let count: usize = POLICY_COUNT.load(Ordering::Acquire) as usize;
	let policies: *mut SkillPolicy = core::ptr::addr_of_mut!(SKILL_POLICIES).cast::<SkillPolicy>();
	let mut slot: usize = count;
	for index in 0..count {
		// Byte loop instead of array equality, which would require a C runtime memcmp.
		let id: *const u8 = (*policies.add(index)).id.as_ptr();
		if (0..96).all(|offset| id.add(offset).read() == action.id.as_ptr().add(offset).read()) {
			slot = index;
		}
	}
	if slot == MAX_POLICIES {
		return Err("maa_skill_policy_table_full");
	}
	policies.add(slot).write(SkillPolicy {
		usage: action.usage,
		remaining: action.times,
		id: action.id,
	});
	if slot == count {
		POLICY_COUNT.store(count as u32 + 1, Ordering::Release);
	}
	scene_number("original_maa_skill_usage_set", action.usage);
	ORDER_CURSOR.fetch_add(1, Ordering::AcqRel);
	complete_command();
	Ok(())
}

// Queue at most one operation for the coming tick: the current ordered command first, then
// any ready policy skill while the job is still running.
unsafe fn plan_tick(
	api: &Api,
	module: Pointer,
	domain: Pointer,
	game: Pointer,
	controller: Pointer,
	frame: Pointer,
	tick: u32,
) -> Result<Option<Pending>, &'static str> {
	let index: u32 = ORDER_CURSOR.load(Ordering::Acquire);
	if index >= ACTION_COUNT.load(Ordering::Acquire) {
		return Ok(None);
	}
	let mut action: ProbeAction = core::ptr::addr_of!(PROBE_ACTIONS)
		.cast::<ProbeAction>()
		.add(index as usize)
		.read();
	action.tick = tick;
	if ordered_action_ready(api, module, domain, game, controller, index, &mut action)?
		&& !(skill_kind(action.kind) && skill_in_flight(&action.id, action.row, action.col))
	{
		match action.kind {
			KIND_SPEED_UP => ordered_speed_up(api, game, controller)?,
			KIND_SKILL_USAGE => apply_skill_usage(&action)?,
			KIND_SKILL_DAEMON => {}
			_ => {
				scene_number("original_step_issued", index);
				scene_number("original_step_issued_tick", tick);
				let proof: ActionProof =
					queue_probe_action(api, module, game, controller, frame, action)?;
				// Like MAA, move to the next command once this one is issued.
				ORDER_CURSOR.fetch_add(1, Ordering::AcqRel);
				return Ok(Some(Pending::Ordered(action, proof)));
			}
		}
	}
	if let Some((policy, mut skill)) = automatic_skill(api, module, game, controller, tick)? {
		skill.tick = tick;
		let proof: ActionProof = queue_probe_action(api, module, game, controller, frame, skill)?;
		return Ok(Some(Pending::Automatic(policy, skill, proof)));
	}
	Ok(None)
}

unsafe fn ordered_speed_up(
	api: &Api,
	game: Pointer,
	controller: Pointer,
) -> Result<(), &'static str> {
	let class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"BattleController\0")?;
	let before: i32 = api.integer(class, controller, b"get_speedLevel\0")?;
	let mut after: i32 = match before {
		1 => 2,
		2 => 1,
		_ => return Err("maa_speed_level_unsupported"),
	};
	api.call(
		class,
		controller,
		b"set_speedLevel\0",
		&mut [(&mut after as *mut i32).cast()],
	)?;
	if api.integer(class, controller, b"get_speedLevel\0")? != after {
		return Err("maa_speed_change_not_observed");
	}
	scene_number("original_maa_speed_before", before as u32);
	scene_number("original_maa_speed_after", after as u32);
	ORDER_CURSOR.fetch_add(1, Ordering::AcqRel);
	complete_command();
	Ok(())
}
