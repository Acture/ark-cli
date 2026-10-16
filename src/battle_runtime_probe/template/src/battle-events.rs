// Per-tick battle events from original state: enemies appearing and leaving (with their last
// raw hit points), life point and kill count changes, operators entering and leaving the field,
// and skills starting and ending. Each is recorded at the tick it is first observed.
const EVENT_ENEMIES: usize = 128;
const EVENT_UNITS: usize = 32;

struct EventState {
	life: i32,
	kills: i32,
	enemies: [(i32, i64); EVENT_ENEMIES],
	enemy_count: usize,
	units: [(i32, bool); EVENT_UNITS],
	unit_count: usize,
	scaled: bool,
}

static mut EVENTS: EventState = EventState {
	life: -1,
	kills: -1,
	enemies: [(0, 0); EVENT_ENEMIES],
	enemy_count: 0,
	units: [(0, false); EVENT_UNITS],
	unit_count: 0,
	scaled: false,
};

unsafe fn event_line(kind: &str, unit: i32, value: i64) {
	let mut line: Line = Line {
		bytes: [0; 384],
		length: 0,
	};
	line.text("{\"stage\":\"original_event\",\"tick\":");
	line.number(RUN_TICKS.load(Ordering::Acquire));
	line.text(",\"kind\":\"");
	line.text(kind);
	line.text("\",\"unit\":");
	line.number(unit as u32);
	line.text(",\"value\":");
	if value < 0 {
		line.text("-");
	}
	let magnitude: u64 = value.unsigned_abs();
	// Line::number takes u32; print the high and low decimal parts.
	let high: u64 = magnitude / 1_000_000_000;
	let low: u32 = (magnitude % 1_000_000_000) as u32;
	if high > 0 {
		line.number(high as u32);
		let mut divisor: u32 = 100_000_000;
		while divisor > 0 {
			line.number((low / divisor) % 10);
			divisor /= 10;
		}
	} else {
		line.number(low);
	}
	line.text("}\n");
	let mut written: u32 = 0;
	if WriteFile(
		LOG.load(Ordering::Acquire) as Pointer,
		line.bytes.as_ptr(),
		line.length as u32,
		&mut written,
		core::ptr::null_mut(),
	) == 0 || written != line.length as u32
	{
		STATUS.store(4, Ordering::Release);
	}
}

// The raw fixed-point value of an original FP; the first enemy's maximum hit points are also
// recorded as text once, so the battle runtime can derive the scale.
unsafe fn fixed_raw(api: &Api, value: Pointer) -> i64 {
	(api.unbox)(value).cast::<i64>().read()
}

unsafe fn observe_events(
	api: &Api,
	module: Pointer,
	game: Pointer,
	controller: Pointer,
) -> Result<(), &'static str> {
	let class_of = level_bind!(
		module,
		b"il2cpp_object_get_class\0",
		unsafe extern "C" fn(Pointer) -> Pointer
	);
	let state: *mut EventState = core::ptr::addr_of_mut!(EVENTS);
	let known_enemies: *mut (i32, i64) = core::ptr::addr_of_mut!((*state).enemies).cast();
	let known_units: *mut (i32, bool) = core::ptr::addr_of_mut!((*state).units).cast();
	let class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"BattleController\0")?;
	let mut side: i32 = 0;
	let life: i32 = number_result(
		api,
		typed_call(
			api,
			class,
			controller,
			b"GetLifePoint\0",
			&[b"Torappu.PlayerSide\0"],
			&mut [(&mut side as *mut i32).cast()],
		)?,
	)? as i32;
	if life != (*state).life {
		if (*state).life >= 0 {
			event_line("life", 0, life as i64);
		}
		(*state).life = life;
	}
	let scheduler: Pointer = api.call(class, controller, b"get_scheduler\0", &mut [])?;
	let scheduler_class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"Scheduler\0")?;
	let kills: i32 = number_result(
		api,
		api.call(
			scheduler_class,
			scheduler,
			b"get_killedEnemiesCnt\0",
			&mut [],
		)?,
	)? as i32;
	if kills != (*state).kills {
		if (*state).kills >= 0 {
			event_line("kills", 0, kills as i64);
		}
		(*state).kills = kills;
	}
	let manager: Pointer = api.call(class, controller, b"get_unitManager\0", &mut [])?;
	let manager_class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"UnitManager\0")?;
	let entity: Pointer = api.class_named(game, b"Torappu.Battle\0", b"Entity\0")?;
	// Enemies: compare this tick's identities with the previous tick's.
	let enemies: Pointer = api.call(manager_class, manager, b"get_enemies\0", &mut [])?;
	let enemies_class: Pointer = class_of(enemies);
	let count: u32 = number_result(api, api.call(enemies_class, enemies, b"get_count\0", &mut [])?)?;
	let mut current: [(i32, i64); EVENT_ENEMIES] = [(0, 0); EVENT_ENEMIES];
	let now_enemies: *mut (i32, i64) = current.as_mut_ptr();
	let mut current_count: usize = 0;
	for offset in 0..count.min(EVENT_ENEMIES as u32) {
		let mut index: i32 = offset as i32;
		let unit: Pointer = api.call(
			enemies_class,
			enemies,
			b"get_Item\0",
			&mut [(&mut index as *mut i32).cast()],
		)?;
		if unit.is_null() {
			continue;
		}
		let instance: i32 = api.integer(class_of(unit), unit, b"GetInstanceID\0")?;
		let hp: Pointer = api.call(entity, unit, b"get_hp\0", &mut [])?;
		let raw: i64 = fixed_raw(api, hp);
		now_enemies.add(current_count).write((instance, raw));
		current_count += 1;
		if !(0..(*state).enemy_count).any(|known| known_enemies.add(known).read().0 == instance) {
			let maximum: Pointer = api.call(entity, unit, b"get_maxHp\0", &mut [])?;
			event_line("enemy_enter", instance, fixed_raw(api, maximum));
			if !(*state).scaled {
				// One sample of raw and text for the fixed-point scale.
				let fp: Pointer = class_of(maximum);
				let text: Pointer = api.call(fp, (api.unbox)(maximum), b"ToString\0", &mut [])?;
				event_line("fixed_sample", instance, fixed_raw(api, maximum));
				managed_ascii(module, text, "original_event_fixed_text")?;
				(*state).scaled = true;
			}
		}
	}
	for known in 0..(*state).enemy_count {
		let (instance, raw): (i32, i64) = known_enemies.add(known).read();
		if !(0..current_count).any(|now| now_enemies.add(now).read().0 == instance) {
			event_line("enemy_leave", instance, raw);
		}
	}
	(*state).enemies = current;
	(*state).enemy_count = current_count;
	// Operators: presence and whether their skill is in effect.
	let characters: Pointer = api.call(manager_class, manager, b"get_characters\0", &mut [])?;
	let characters_class: Pointer = class_of(characters);
	let count: u32 =
		number_result(api, api.call(characters_class, characters, b"get_count\0", &mut [])?)?;
	let character_class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"Character\0")?;
	let skill_class: Pointer = api.class_named(game, b"Torappu.Battle\0", b"BasicSkill\0")?;
	let mut units: [(i32, bool); EVENT_UNITS] = [(0, false); EVENT_UNITS];
	let now_units: *mut (i32, bool) = units.as_mut_ptr();
	let mut unit_count: usize = 0;
	for offset in 0..count.min(EVENT_UNITS as u32) {
		let mut index: i32 = offset as i32;
		let unit: Pointer = api.call(
			characters_class,
			characters,
			b"get_Item\0",
			&mut [(&mut index as *mut i32).cast()],
		)?;
		if unit.is_null() {
			continue;
		}
		let instance: i32 = api.integer(class_of(unit), unit, b"GetInstanceID\0")?;
		let skill: Pointer = api
			.call(character_class, unit, b"get_skill\0", &mut [])
			.unwrap_or(core::ptr::null_mut());
		let affecting: bool = !skill.is_null()
			&& managed_bool(api, skill_class, skill, b"get_isAffecting\0").unwrap_or(false);
		now_units.add(unit_count).write((instance, affecting));
		unit_count += 1;
		match (0..(*state).unit_count).find(|known| known_units.add(*known).read().0 == instance) {
			None => {
				event_line("unit_enter", instance, 0);
				if affecting {
					event_line("skill_start", instance, 0);
				}
			}
			Some(known) => {
				if affecting != known_units.add(known).read().1 {
					event_line(
						if affecting { "skill_start" } else { "skill_end" },
						instance,
						0,
					);
				}
			}
		}
	}
	for known in 0..(*state).unit_count {
		let instance: i32 = known_units.add(known).read().0;
		if !(0..unit_count).any(|now| now_units.add(now).read().0 == instance) {
			event_line("unit_leave", instance, 0);
		}
	}
	(*state).units = units;
	(*state).unit_count = unit_count;
	Ok(())
}
