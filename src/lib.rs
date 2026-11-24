mod keybinds;
mod recipe;
mod rules;
mod sort;
mod transfer;
mod trading;

use std::{
    collections::{HashSet, VecDeque},
    sync::Mutex,
};

use mod_api::{
    ClientHost, ClientTickPostEvent, Flow, Hooks, ItemStack, KeyConflictContext, KeyRegistration,
    MenuClick, MenuRef, MenuSlotKind, MouseButton, Registrar, RegistrationContext,
    ScreenMouseScrolledPreEvent, ScreenView, SettingSpec, Tag,
};
use recipe::{Grid, Recipe, Recipes};
use rules::Rules;
use transfer::{Click, Mode, Slot, quick_move_sources, transfer};

const ID: &str = "item_scroller";
const MAX_PENDING: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sweep {
    Move(Mode),
    Drop(Mode),
}

enum Action {
    Transfer { slot: usize, pull: bool, mode: Mode, anchor: Option<ItemStack> },
    Drop { slot: usize, mode: Mode, anchor: ItemStack },
    Clicks { cursor: Option<ItemStack>, clicks: Vec<MenuClick>, expected: Vec<(usize, Option<ItemStack>)> },
    Sort(Vec<usize>),
    Trade(trading::Trade, Vec<usize>),
    Batch(VecDeque<(usize, ItemStack)>),
    Refill { inputs: Vec<usize>, player: Vec<usize>, wanted: Vec<Option<ItemStack>>, sets: u32 },
    Craft { grid: Grid, recipe: Recipe, remaining: u32, drop: bool },
}

struct Job {
    menu: MenuRef,
    action: Action,
    previous: Option<Vec<Option<ItemStack>>>,
    unchanged: u8,
}

struct Drag {
    menu: MenuRef,
    button: u8,
    visited: Vec<(usize, Sweep)>,
}

#[derive(Default)]
struct State {
    keys: keybinds::Editor,
    pending: VecDeque<Job>,
    drag: Option<Drag>,
    recipes: Recipes,
    recipe_save_error_until: u64,
    rules: Rules,
    menu_id: Option<String>,
    disabled: bool,
    drop_key: Option<String>,
    mass_craft: bool,
    close_config: bool,
    paused_config: bool,
    favorites: trading::Favorites,
    favorite_error_until: u64,
    trade_inputs: Option<(MenuRef, Vec<Option<ItemStack>>)>,
}

#[derive(Default)]
struct Scroller(Mutex<State>);

fn active_menu(host: &dyn ClientHost) -> Option<MenuRef> {
    host.local_player()?.container_menu(host)
}

fn contains([x, y, width, height]: [f32; 4], [mx, my]: [f32; 2]) -> bool {
    mx >= x && mx < x + width && my >= y && my < y + height
}

fn hit(host: &dyn ClientHost, menu: MenuRef, mouse: [f32; 2]) -> Option<usize> {
    menu.slot_boxes(host)?.iter().position(|rect| rect.is_some_and(|r| contains(r, mouse)))
}

fn personal(kinds: &[MenuSlotKind]) -> bool {
    kinds.contains(&MenuSlotKind::Player(40))
}

fn side(kind: MenuSlotKind, personal: bool) -> Option<bool> {
    match kind {
        MenuSlotKind::Player(i @ 0..=35) => Some(!personal || i >= 9),
        MenuSlotKind::Storage => Some(false),
        MenuSlotKind::CraftingGrid if !personal => Some(false),
        _ => None,
    }
}

fn capacity(host: &dyn ClientHost, stack: &ItemStack) -> u32 {
    if let Some(patch) = stack.components() {
        let patch = patch.as_nbt();
        if patch.contains_key(&"!minecraft:max_stack_size".into()) {
            return 1;
        }
        if let Some(Tag::Int(n)) = patch.get(&"minecraft:max_stack_size".into()) {
            return (*n).clamp(1, 99) as u32;
        }
    }
    host.max_stack_size(stack.item()).unwrap_or(1)
}

fn normalized(mut stack: ItemStack) -> ItemStack {
    stack.set_count(1).expect("one item is a valid stack count");
    stack
}

fn matching(a: &ItemStack, b: &ItemStack) -> bool {
    a.same_item(b) && a.same_components(b)
}

fn snapshot(host: &dyn ClientHost, menu: MenuRef, blocked: Option<&HashSet<usize>>) -> Option<Vec<Slot<ItemStack>>> {
    let kinds = menu.slot_kinds(host)?;
    let boxes = menu.slot_boxes(host)?;
    let personal = personal(&kinds);
    let mut slots = Vec::new();
    for (index, kind) in kinds.into_iter().enumerate() {
        let Some(player_side) = side(kind, personal) else { continue };
        if !boxes.get(index).is_some_and(Option::is_some) || blocked.is_some_and(|slots| slots.contains(&index)) {
            continue;
        }
        let item = menu.slot(host, index)?;
        let capacity = item.as_ref().map_or(99, |stack| capacity(host, stack));
        let count = item.as_ref().map_or(0, ItemStack::count);
        slots.push(Slot {
            id: index.try_into().ok()?, player_side, item: item.map(normalized), count, capacity,
        });
    }
    Some(slots)
}

fn pickup(slot: usize) -> MenuClick {
    MenuClick::Pickup { slot: Some(slot), button: MouseButton::Left }
}

fn right(slot: usize) -> MenuClick {
    MenuClick::Pickup { slot: Some(slot), button: MouseButton::Right }
}

fn outside() -> MenuClick {
    MenuClick::Pickup { slot: None, button: MouseButton::Left }
}

fn drop_key(host: &dyn ClientHost, key: &str) -> bool {
    let label = |id: &str| {
        host.text_string(&mod_api::Text::new(mod_api::text::TextContent::Keybind(id.into()))).ok()
    };
    label("key.drop").is_some_and(|drop| Some(drop) == label(key))
}

fn enabled(host: &dyn ClientHost, id: &str) -> bool {
    host.setting_bool(id).unwrap_or(true)
}

fn sweep(keys: &keybinds::Editor, host: &dyn ClientHost, button: u8, drop: bool) -> Option<Sweep> {
    let stack = keys.held(host, "stack");
    let matching = keys.held(host, "matching");
    let mode = match (stack, matching, button) {
        (true, _, 1) => Mode::LeaveOne,
        (true, true, 0) if !drop => Mode::Matching,
        (_, true, 0 | 1) => Mode::One,
        (true, false, 0) => Mode::Stack,
        _ => return None,
    };
    if drop {
        enabled(host, "sweep_drop").then_some(Sweep::Drop(mode))
    } else {
        Some(Sweep::Move(mode))
    }
}

fn storage_sortable(menu: Option<&str>) -> bool {
    matches!(menu, Some("minecraft:generic_3x3" | "minecraft:hopper" | "minecraft:shulker_box" | "minecraft:ender_chest"))
        || menu.is_some_and(|id| id.starts_with("minecraft:generic_9x"))
        || menu.is_some_and(|id| id.starts_with("horizon:mount"))
}

fn sort_slots(host: &dyn ClientHost, menu: MenuRef, screen: &ScreenView<'_>, hover: usize) -> Option<Vec<usize>> {
    let kinds = menu.slot_kinds(host)?;
    let allowed = match kinds.get(hover)? {
        MenuSlotKind::Storage if storage_sortable(screen.menu) => |k| matches!(k, MenuSlotKind::Storage),
        MenuSlotKind::Player(0..=8) => |k| matches!(k, MenuSlotKind::Player(0..=8)),
        MenuSlotKind::Player(9..=35) => |k| matches!(k, MenuSlotKind::Player(9..=35)),
        _ => return None,
    };
    let boxes = menu.slot_boxes(host)?;
    Some(kinds.into_iter().enumerate().filter_map(|(i, kind)|
        (allowed(kind) && boxes.get(i).is_some_and(Option::is_some)).then_some(i)).collect())
}

fn position_pull(host: &dyn ClientHost, menu: MenuRef, index: usize, up: bool) -> Option<bool> {
    let kinds = menu.slot_kinds(host)?;
    let boxes = menu.slot_boxes(host)?;
    let personal = personal(&kinds);
    let anchor = side(*kinds.get(index)?, personal)?;
    let y = boxes.get(index).copied().flatten()?[1];
    let other_y = kinds.iter().zip(&boxes).filter_map(|(&kind, rect)| {
        let target = side(kind, personal)?;
        let rect = (*rect)?;
        (target != anchor).then_some(rect[1])
    }).min_by(|a, b| (a - y).abs().total_cmp(&(b - y).abs()))?;
    if y == other_y { return None; }
    Some(up != (other_y < y))
}

impl State {
    fn open(&mut self, host: &dyn ClientHost, screen: &ScreenView<'_>) -> bool {
        let id = screen.menu.unwrap_or("inventory");
        if self.menu_id.as_deref() != Some(id) {
            self.rules = Rules::load(host);
            self.menu_id = Some(id.to_owned());
        }
        self.rules.menu_allowed(id)
    }

    fn allows_slot(&self, slot: usize) -> bool {
        !self.rules.blocked(self.menu_id.as_deref().unwrap_or("inventory"))
            .is_some_and(|blocked| blocked.contains(&slot))
    }

    fn grid(&self, host: &dyn ClientHost, menu: MenuRef) -> Option<Grid> {
        let id = self.menu_id.as_deref().unwrap_or("inventory");
        let mut grid = Grid::read(host, menu, id, self.rules.grids.get(id))?;
        if !self.allows_slot(grid.output) || grid.inputs.iter().any(|&slot| !self.allows_slot(slot)) {
            return None;
        }
        grid.player.retain(|&slot| self.allows_slot(slot));
        Some(grid)
    }

    fn drop_held(&self, host: &dyn ClientHost) -> bool {
        self.drop_key.as_deref().is_some_and(|key| host.is_key_down(key))
    }

    fn enqueue(&mut self, menu: MenuRef, action: Action) -> Flow {
        if self.pending.len() < MAX_PENDING {
            self.pending.push_back(Job { menu, action, previous: None, unchanged: 0 });
        }
        Flow::Cancel
    }

    fn clicks(&mut self, host: &dyn ClientHost, menu: MenuRef, cursor: Option<ItemStack>, clicks: Vec<MenuClick>) -> Flow {
        if clicks.is_empty() { return Flow::Continue; }
        let mut slots: Vec<_> = clicks.iter().filter_map(|click| match click {
            MenuClick::Pickup { slot, .. } => *slot,
            MenuClick::QuickMove { slot } | MenuClick::Throw { slot, .. } => Some(*slot),
            _ => None,
        }).collect();
        slots.sort_unstable();
        slots.dedup();
        if slots.iter().any(|&slot| !self.allows_slot(slot)) { return Flow::Continue; }
        let Some(expected) = slots.into_iter().map(|i| Some((i, menu.slot(host, i)?))).collect() else {
            return Flow::Continue;
        };
        self.enqueue(menu, Action::Clicks { cursor, clicks, expected })
    }

    fn transfer(&mut self, host: &dyn ClientHost, menu: MenuRef, slot: usize, pull: bool, mode: Mode) -> Flow {
        if !self.allows_slot(slot) { return Flow::Continue; }
        let Some(kinds) = menu.slot_kinds(host) else { return Flow::Continue };
        let personal = personal(&kinds);
        let Some(source) = kinds.get(slot).and_then(|&k| side(k, personal)) else { return Flow::Continue };
        if matches!(mode, Mode::All | Mode::Matching) && kinds.iter().enumerate().any(|(i, &kind)| {
            !self.allows_slot(i) && side(kind, personal) == Some(source == pull)
        }) {
            return Flow::Continue;
        }
        if !kinds.iter().any(|&kind| side(kind, personal) == Some(!source)) {
            return Flow::Continue;
        }
        let Some(anchor) = menu.slot(host, slot) else { return Flow::Continue };
        if anchor.is_none() && mode != Mode::All && (mode == Mode::Matching || !pull) {
            return Flow::Continue;
        }
        self.enqueue(menu, Action::Transfer { slot, pull, mode, anchor: anchor.map(normalized) })
    }

    fn sweep_slot(&mut self, host: &dyn ClientHost, menu: MenuRef, slot: usize, sweep: Sweep) -> Flow {
        if !self.allows_slot(slot) { return Flow::Continue; }
        if self.drag.as_ref().is_some_and(|drag| drag.visited.contains(&(slot, sweep))) {
            return Flow::Cancel;
        }
        let flow = match sweep {
            Sweep::Move(mode) => {
                let up = self.keys.held(host, "move_up");
                let down = self.keys.held(host, "move_down");
                let pull = if up || down { position_pull(host, menu, slot, up).unwrap_or(false) } else { false };
                self.transfer(host, menu, slot, pull, mode)
            }
            Sweep::Drop(mode) => {
                let eligible = menu.slot_kinds(host).and_then(|kinds| kinds.get(slot).copied())
                    .is_some_and(|kind| side(kind, false).is_some());
                if eligible && let Some(anchor) = menu.slot(host, slot).flatten() {
                    self.enqueue(menu, Action::Drop { slot, mode, anchor: normalized(anchor) })
                } else { Flow::Continue }
            }
        };
        if matches!(flow, Flow::Cancel) && let Some(drag) = &mut self.drag {
            drag.visited.push((slot, sweep));
        }
        flow
    }

    fn store(&mut self, host: &dyn ClientHost, menu: MenuRef, grid: &Grid) -> bool {
        let Some(recipe) = grid.remember(host, menu) else { return false };
        self.recipes.load(host);
        self.recipe_save_error_until = if self.recipes.store(host, recipe).is_err() {
            host.millis().saturating_add(5000)
        } else { 0 };
        true
    }

    fn selected_recipe(&mut self, host: &dyn ClientHost) -> Option<Recipe> {
        self.recipes.load(host);
        self.recipes.entries[self.recipes.selected].clone()
    }

    fn fill(&mut self, menu: MenuRef, grid: Grid, recipe: Recipe, sets: u32) -> Flow {
        let Some(recipe) = recipe.fit(grid.inputs.len()) else { return Flow::Continue };
        self.enqueue(menu, Action::Refill { inputs: grid.inputs, player: grid.player, wanted: recipe.inputs, sets })
    }

    fn craft(&mut self, menu: MenuRef, grid: Grid, recipe: Recipe, remaining: u32, drop: bool) -> Flow {
        let Some(recipe) = recipe.fit(grid.inputs.len()) else { return Flow::Continue };
        self.enqueue(menu, Action::Craft { grid, recipe, remaining, drop })
    }
}

fn furnace_stack_push(menu_id: &str, index: usize, pull: bool, mode: Mode) -> bool {
    matches!(menu_id, "minecraft:furnace" | "minecraft:blast_furnace" | "minecraft:smoker")
        && (3..39).contains(&index) && !pull && mode == Mode::Stack
}

fn transfer_clicks(
    host: &dyn ClientHost, menu: MenuRef, index: usize, pull: bool, mode: Mode, anchor: &Option<ItemStack>,
    blocked: Option<&HashSet<usize>>,
) -> Vec<MenuClick> {
    let Some(slots) = snapshot(host, menu, blocked) else { return Vec::new() };
    let Ok(hovered) = u16::try_from(index) else { return Vec::new() };
    if matches!(mode, Mode::All | Mode::Matching) {
        return quick_move_sources(&slots, hovered, pull, mode == Mode::Matching, anchor.as_ref())
            .into_iter().map(|slot| MenuClick::QuickMove { slot: usize::from(slot) }).collect();
    }
    if !slots.iter().any(|slot| slot.id == hovered && &slot.item == anchor) {
        return Vec::new();
    }
    transfer(&slots, hovered, pull, mode, true).into_iter().map(|click| match click {
        Click::Left(slot) => pickup(slot.into()),
        Click::Right(slot) => right(slot.into()),
    }).collect()
}

impl Job {
    fn run(&mut self, host: &mut dyn ClientHost, rules: &Rules, id: &str) -> bool {
        let menu = self.menu;
        if active_menu(host) != Some(menu) || menu.slot_boxes(host).is_none() {
            return false;
        }
        if let Action::Clicks { cursor, clicks, expected } = &self.action {
            if menu.carried(host).as_ref() == Some(cursor)
                && expected.iter().all(|(i, stack)| menu.slot(host, *i).as_ref() == Some(stack))
            {
                menu.click(host, clicks);
            }
            return false;
        }
        if menu.carried(host) != Some(None) {
            return false;
        }
        if matches!(self.action, Action::Trade(..) | Action::Sort(_) | Action::Refill { .. } | Action::Craft { .. }) {
            let Some(current) = menu.slot_count(host).and_then(|count| {
                (0..count).map(|slot| menu.slot(host, slot)).collect::<Option<Vec<_>>>()
            }) else { return false };
            if self.previous.as_ref() == Some(&current) {
                self.unchanged += 1;
                return self.unchanged < 8;
            }
            self.previous = Some(current);
            self.unchanged = 0;
        }
        match &mut self.action {
            Action::Clicks { .. } => unreachable!(),
            Action::Transfer { slot, pull, mode, anchor } => {
                if furnace_stack_push(id, *slot, *pull, *mode) {
                    // QuickMove uses this furnace's cooking recipes and fuel registry.
                    // Its fallback can also move between the hotbar and main inventory.
                    if rules.blocked(id).is_none_or(|slots| slots.iter().all(|&i| i == 2 || i >= 39))
                        && anchor.is_some()
                        && menu.slot(host, *slot).flatten().map(normalized) == *anchor
                    {
                        menu.click(host, &[MenuClick::QuickMove { slot: *slot }]);
                    }
                    return false;
                }
                let mut clicks = transfer_clicks(host, menu, *slot, *pull, *mode, anchor, rules.blocked(id));
                if !clicks.is_empty() {
                    if matches!(mode, Mode::All | Mode::Matching) && clicks.len() > 8 {
                        let remaining = clicks.split_off(8).into_iter().filter_map(|click| {
                            let MenuClick::QuickMove { slot } = click else { return None };
                            Some((slot, menu.slot(host, slot)??))
                        }).collect();
                        if menu.click(host, &clicks) {
                            self.action = Action::Batch(remaining);
                            return true;
                        }
                    } else {
                        menu.click(host, &clicks);
                    }
                }
                false
            }
            Action::Batch(remaining) => {
                let count = remaining.len().min(8);
                let batch: Vec<_> = remaining.drain(..count)
                    .filter(|(slot, stack)| menu.slot(host, *slot).flatten().as_ref() == Some(stack))
                    .map(|(slot, _)| MenuClick::QuickMove { slot }).collect();
                (batch.is_empty() || menu.click(host, &batch)) && !remaining.is_empty()
            }
            Action::Drop { slot, mode, anchor } => {
                let Some(stack) = menu.slot(host, *slot).flatten().filter(|s| matching(s, anchor)) else {
                    return false;
                };
                let clicks = match mode {
                    Mode::LeaveOne if stack.count() > 1 => vec![pickup(*slot), right(*slot), outside()],
                    Mode::LeaveOne => return false,
                    _ => vec![MenuClick::Throw { slot: *slot, whole_stack: *mode != Mode::One }],
                };
                menu.click(host, &clicks);
                false
            }
            Action::Trade(trade, player) => id == "minecraft:merchant" && trade.step(host, menu, player),
            Action::Sort(slots) => sort::step(host, menu, slots, rules),
            Action::Refill { inputs, player, wanted, sets } => {
                let clicks = recipe::refill(host, menu, inputs, player, wanted, *sets);
                !clicks.is_empty() && menu.click(host, &clicks)
            }
            Action::Craft { grid, recipe, remaining, drop } => {
                if *remaining == 0 { return false; }
                let grid_matches = grid.inputs.iter().zip(&recipe.inputs).all(|(&slot, wanted)| {
                    match (menu.slot(host, slot), wanted) {
                        (Some(None), None) => true,
                        (Some(Some(stack)), Some(wanted)) => matching(&stack, wanted),
                        _ => false,
                    }
                });
                let output = menu.slot(host, grid.output).flatten().filter(|out| {
                    grid_matches && matching(out, &recipe.output)
                });
                let clicks = if let Some(output) = output {
                    if *drop {
                        *remaining = remaining.saturating_sub(output.count());
                        vec![pickup(grid.output), outside()]
                    } else if output.count() <= *remaining {
                        *remaining -= output.count();
                        recipe::take_one(host, menu, grid, &output)
                    } else {
                        return false;
                    }
                } else {
                    recipe::refill(host, menu, &grid.inputs, &grid.player, &recipe.inputs, 1)
                };
                if clicks.is_empty() || !menu.click(host, &clicks) { return false; }
                *remaining > 0
            }
        }
    }
}

#[mod_api::horizon_mod]
impl Hooks for Scroller {
    fn register(&self, r: &mut dyn Registrar, _: &RegistrationContext) {
        r.register_key_mapping(KeyRegistration::new("open_config", "key.keyboard.i")
            .modifiers(mod_api::ClientModifiers::new(6))
            .label("key.item_scroller.open_config").context(KeyConflictContext::InGame));
        for (id, default) in [
            ("sweep", true), ("sweep_drop", true), ("reverse", false), ("reverse_one", false),
            ("reverse_stacks", false), ("position_scroll", false), ("scroll_one", true),
            ("scroll_stack", true), ("scroll_matching", true), ("scroll_all", true),
            ("upstream_scroll_all", false),
            ("cursor_place", true), ("cursor_drop", true), ("matching_drop", true),
            ("sort_enabled", true), ("crafting", true), ("save_recipes", true),
            ("store_on_fill", false), ("recipe_items", true), ("right_craft", true),
            ("villager_scroll", true), ("trade_shortcuts", true),
        ] {
            let _ = r.register_setting(SettingSpec::toggle(id, format!("item_scroller.setting.{id}"), default));
        }
        let _ = r.register_setting(SettingSpec::choice("sort_method", "item_scroller.setting.sort_method",
            ["id", "count", "registry", "name", "rarity"].map(|id| format!("item_scroller.sort.{id}")), 0));
    }

    fn screen_key_pressed_pre(&self, host: &dyn ClientHost, e: &mut mod_api::ScreenKeyPressedPreEvent<'_>) -> Flow {
        let mut state = self.0.lock().unwrap();
        if state.paused_config && e.screen.id == "paused" {
            if matches!(state.keys.press(host, e.key, e.modifiers, e.repeat), Flow::Continue)
                && !e.repeat
            {
                state.keys.close();
                state.paused_config = false;
            }
            return Flow::Cancel;
        }
        if host.mod_screen() == Some(ID) {
            return state.keys.press(host, e.key, e.modifiers, e.repeat);
        }
        if e.screen.id == "paused" && host.mod_key(ID, "open_config").is_some_and(|(key, mods)| {
            e.key == key && e.modifiers & 15 == mods.bits & 15
        }) {
            if !e.repeat {
                state.paused_config = true;
            }
            return Flow::Cancel;
        }
        if e.screen.id != "inventory" { return Flow::Continue; }
        state.keys.load(host);
        state.pending.retain(|job| !matches!(job.action, Action::Trade(..)));
        if state.keys.bound("toggle", e.key, e.modifiers) {
            if !e.repeat {
                state.disabled = !state.disabled;
                state.pending.clear();
                state.drag = None;
                state.mass_craft = false;
            }
            return Flow::Cancel;
        }
        if state.disabled || !state.open(host, e.screen) { return Flow::Continue; }
        let Some(menu) = active_menu(host) else { return Flow::Continue };
        let is_drop = drop_key(host, e.key);
        if is_drop {
            state.drop_key = Some(e.key.to_owned());
            if e.repeat && (state.drag.is_some() || state.keys.held(host, "drop_all") || state.keys.held(host, "drop_matching")) {
                return Flow::Cancel;
            }
        }
        let index = hit(host, menu, e.screen.mouse);
        if menu.carried(host) != Some(None) || e.repeat { return Flow::Continue; }

        if state.keys.bound("debug", e.key, e.modifiers) {
            let kinds = menu.slot_kinds(host);
            let kind = index.and_then(|i| kinds.as_ref()?.get(i));
            println!("[Item Scroller] menu={:?}, slot={index:?}, role={kind:?}", e.screen.menu);
            return Flow::Cancel;
        }
        if enabled(host, "sort_enabled") && state.keys.bound("sort", e.key, e.modifiers) {
            let Some(index) = index.filter(|&slot| state.allows_slot(slot)) else { return Flow::Continue };
            let Some(mut slots) = sort_slots(host, menu, e.screen, index) else { return Flow::Continue };
            slots.retain(|&slot| state.allows_slot(slot));
            return state.enqueue(menu, Action::Sort(slots));
        }
        if enabled(host, "crafting") {
            if let Some(grid) = state.grid(host, menu) {
                if state.keys.held(host, "recipes")
                    && let Some(number) = e.key.strip_prefix("key.keyboard.").and_then(|s| s.parse::<usize>().ok())
                    && (1..=9).contains(&number)
                {
                    state.recipes.selected = number - 1;
                    if index == Some(grid.output) {
                        if state.keys.held(host, "stack") { state.store(host, menu, &grid); }
                        else if let Some(recipe) = state.selected_recipe(host) { state.fill(menu, grid, recipe, 1); }
                    }
                    return Flow::Cancel;
                }
                if state.keys.bound("mass_toggle", e.key, e.modifiers) {
                    state.mass_craft = !state.mass_craft;
                    return Flow::Cancel;
                }
                if state.keys.bound("craft_all", e.key, e.modifiers) && let Some(recipe) = state.selected_recipe(host) {
                    return state.craft(menu, grid, recipe, 4096, false);
                }
            }
            let drop = state.keys.bound("drop_results", e.key, e.modifiers);
            if (drop || state.keys.bound("move_results", e.key, e.modifiers))
                && let Some(recipe) = state.selected_recipe(host)
                && let Some(kinds) = menu.slot_kinds(host)
            {
                let clicks = recipe::player_slots(&kinds).into_iter().filter(|&i| state.allows_slot(i)).filter(|&i| {
                    menu.slot(host, i).flatten().is_some_and(|s| matching(&s, &recipe.output))
                }).filter_map(|slot| {
                    if drop { Some(MenuClick::Throw { slot, whole_stack: true }) }
                    else if kinds.contains(&MenuSlotKind::Storage) { Some(MenuClick::QuickMove { slot }) }
                    else { None }
                }).collect();
                return state.clicks(host, menu, None, clicks);
            }
        }
        let all = state.keys.held(host, "drop_all");
        if (!all && !(enabled(host, "matching_drop") && state.keys.held(host, "drop_matching"))) || !is_drop {
            return Flow::Continue;
        }
        let Some(index) = index.filter(|&i| state.allows_slot(i)) else { return Flow::Continue };
        let Some(kinds) = menu.slot_kinds(host) else { return Flow::Continue };
        let Some(source) = kinds.get(index).and_then(|&k| side(k, false)) else { return Flow::Continue };
        let anchor = menu.slot(host, index).flatten();
        let clicks = kinds.into_iter().enumerate().filter(|&(slot, k)| state.allows_slot(slot) && side(k, false) == Some(source))
            .filter_map(|(i, _)| Some((i, menu.slot(host, i)??)))
            .filter(|(_, s)| all || anchor.as_ref().is_some_and(|a| a.same_item(s)))
            .map(|(slot, _)| MenuClick::Throw { slot, whole_stack: true }).collect();
        state.clicks(host, menu, None, clicks)
    }

    fn screen_mouse_button_pressed_pre(&self, host: &dyn ClientHost, e: &mut mod_api::ScreenMouseButtonPressedPreEvent<'_>) -> Flow {
        let mut state = self.0.lock().unwrap();
        if (state.paused_config && e.screen.id == "paused") || host.mod_screen() == Some(ID) {
            if e.button == 0 && state.keys.click(host, e.screen.mouse) {
                if state.paused_config {
                    state.keys.close();
                    state.paused_config = false;
                } else {
                    state.close_config = true;
                }
            }
            return Flow::Cancel;
        }
        if e.screen.id != "inventory" { return Flow::Continue; }
        state.keys.load(host);
        if state.disabled || !state.open(host, e.screen) { return Flow::Continue; }
        let Some(menu) = active_menu(host) else { return Flow::Continue };
        let index = hit(host, menu, e.screen.mouse);
        let Some(cursor) = menu.carried(host) else { return Flow::Continue };
        if e.screen.menu == Some("minecraft:merchant") {
            state.pending.retain(|job| !matches!(job.action, Action::Trade(..)));
            if index == Some(2) && cursor.is_none() && enabled(host, "trade_shortcuts")
                && (0..3).all(|slot| state.allows_slot(slot))
                && matches!(e.button, 1 | 2)
                && let Some(trade) = trading::Trade::read(host, menu)
            {
                if e.button == 2 {
                    if state.favorites.toggle(host, &trade).is_none() {
                        state.favorite_error_until = host.millis().saturating_add(5000);
                    }
                } else if let Some(kinds) = menu.slot_kinds(host) {
                    let player = recipe::player_slots(&kinds);
                    // QuickMove can send results to any player slot.
                    if player.iter().all(|&slot| state.allows_slot(slot)) {
                        return state.enqueue(menu, Action::Trade(trade, player));
                    }
                }
                return Flow::Cancel;
            }
        }
        if e.screen.menu == Some("minecraft:merchant") && index.is_none() {
            state.trade_inputs = None;
        }
        if cursor.is_none() && enabled(host, "crafting") && let Some(grid) = state.grid(host, menu) {
            if state.keys.held(host, "recipes") && let Some(i) = (0..recipe::COUNT).find(|&i| contains(recipe::panel_box(e.screen, i), e.screen.mouse)) {
                state.recipes.selected = i;
                if e.button == 2 {
                    return state.clicks(host, menu, None, recipe::clear(host, menu, &grid.inputs));
                }
                if let Some(recipe) = state.selected_recipe(host) {
                    let sets = if state.keys.held(host, "stack") { 64 } else { 1 };
                    return state.fill(menu, grid, recipe, sets);
                }
                return Flow::Cancel;
            }
            if index == Some(grid.output) && e.button == 2 {
                return if state.store(host, menu, &grid) { Flow::Cancel } else { Flow::Continue };
            }
            if index == Some(grid.output) && e.button == 1 && enabled(host, "right_craft")
                && let Some(recipe) = grid.remember(host, menu)
            {
                let limit = capacity(host, &recipe.output);
                let drop = state.drop_held(host);
                return state.craft(menu, grid, recipe, limit, drop);
            }
        }
        if let Some(cursor) = cursor {
            if e.button != 0 || !state.keys.held(host, "stack") { return Flow::Continue; }
            let Some(kinds) = menu.slot_kinds(host) else { return Flow::Continue };
            let mut clicks = Vec::new();
            if let Some(index) = index {
                if !enabled(host, "cursor_place") || menu.slot(host, index) != Some(None) { return Flow::Continue; }
                let personal = personal(&kinds);
                let Some(destination) = kinds.get(index).and_then(|&k| side(k, personal)) else { return Flow::Continue };
                clicks.push(pickup(index));
                clicks.extend(kinds.into_iter().enumerate().filter(|&(_, kind)| side(kind, personal) == Some(!destination))
                    .filter(|&(slot, _)| state.allows_slot(slot))
                    .filter(|&(slot, _)| menu.slot(host, slot).flatten().is_some_and(|s| matching(&s, &cursor)))
                    .map(|(slot, _)| MenuClick::QuickMove { slot }));
            } else if enabled(host, "cursor_drop") && !contains(e.screen.rect, e.screen.mouse) {
                clicks.push(outside());
                clicks.extend(kinds.into_iter().enumerate().filter(|&(_, kind)| side(kind, false).is_some())
                    .filter(|&(slot, _)| state.allows_slot(slot))
                    .filter(|&(slot, _)| menu.slot(host, slot).flatten().is_some_and(|s| matching(&s, &cursor)))
                    .map(|(slot, _)| MenuClick::Throw { slot, whole_stack: true }));
            }
            return state.clicks(host, menu, Some(cursor), clicks);
        }
        let Some(slot) = index else { return Flow::Continue };
        if e.button == 0 && state.keys.held(host, "deposit") {
            return state.transfer(host, menu, slot, false, Mode::All);
        }
        if e.button == 0 && state.keys.held(host, "matching_click") {
            return state.transfer(host, menu, slot, false, Mode::Matching);
        }
        let Some(sweep) = sweep(&state.keys, host, e.button as u8, state.drop_held(host)) else { return Flow::Continue };
        state.drag = Some(Drag { menu, button: e.button as u8, visited: Vec::new() });
        let flow = state.sweep_slot(host, menu, slot, sweep);
        if !matches!(flow, Flow::Cancel) { state.drag = None; }
        flow
    }

    fn screen_mouse_dragged_pre(&self, host: &dyn ClientHost, e: &mut mod_api::ScreenMouseDraggedPreEvent<'_>) -> Flow {
        let mut state = self.0.lock().unwrap();
        let Some(drag) = &state.drag else { return Flow::Continue };
        let menu = drag.menu;
        if e.button as u8 != drag.button || active_menu(host) != Some(menu) { return Flow::Continue; }
        if enabled(host, "sweep") && let Some(sweep) = sweep(&state.keys, host, drag.button, state.drop_held(host))
            && let Some(slot) = hit(host, menu, e.screen.mouse)
        {
            state.sweep_slot(host, menu, slot, sweep);
        }
        Flow::Cancel
    }

    fn screen_mouse_button_released_pre(&self, _: &dyn ClientHost, e: &mut mod_api::ScreenMouseButtonReleasedPreEvent<'_>) -> Flow {
        let mut state = self.0.lock().unwrap();
        if state.paused_config && e.screen.id == "paused" { return Flow::Cancel; }
        if state.drag.as_ref().is_some_and(|drag| drag.button == e.button as u8) {
            state.drag = None;
            Flow::Cancel
        } else { Flow::Continue }
    }

    fn screen_key_released_pre(&self, host: &dyn ClientHost, e: &mut mod_api::ScreenKeyReleasedPreEvent<'_>) -> Flow {
        let mut state = self.0.lock().unwrap();
        if (state.paused_config && e.screen.id == "paused") || host.mod_screen() == Some(ID) {
            state.keys.release(host, e.key);
        }
        Flow::Continue
    }

    fn screen_closing(&self, _: &dyn ClientHost, _: &mut mod_api::ScreenClosingEvent<'_>) {
        let mut state = self.0.lock().unwrap();
        state.keys.close();
        state.close_config = false;
        state.paused_config = false;
        state.drag = None;
        state.mass_craft = false;
        state.trade_inputs = None;
        state.drop_key = None;
        state.menu_id = None;
        state.pending.clear();
    }

    fn screen_mouse_scrolled_pre(&self, host: &dyn ClientHost, e: &mut ScreenMouseScrolledPreEvent<'_>) -> Flow {
        let dy = e.scroll[1];
        let mut state = self.0.lock().unwrap();
        if (state.paused_config && e.screen.id == "paused") || host.mod_screen() == Some(ID) {
            state.keys.scroll(dy);
            return Flow::Cancel;
        }
        if e.screen.id != "inventory" || !dy.is_finite() || dy == 0.0 { return Flow::Continue; }
        state.keys.load(host);
        state.pending.retain(|job| !matches!(job.action, Action::Trade(..)));
        if state.disabled || !state.open(host, e.screen) { return Flow::Continue; }
        let Some(menu) = active_menu(host) else { return Flow::Continue };
        if menu.carried(host) != Some(None) { return Flow::Continue; }
        let index = hit(host, menu, e.screen.mouse);
        let stack = state.keys.held(host, "stack");
        let matching = state.keys.held(host, "matching");
        let all = state.keys.held(host, "deposit");
        if enabled(host, "crafting") && let Some(grid) = state.grid(host, menu) {
            if state.keys.held(host, "recipes") {
                state.recipes.selected = (state.recipes.selected as isize - dy.signum() as isize)
                    .rem_euclid(recipe::COUNT as isize) as usize;
                return Flow::Cancel;
            }
            if index == Some(grid.output) {
                if dy > 0.0 {
                    if let Some(recipe) = grid.remember(host, menu) {
                        if all || (stack && matching) {
                            return state.craft(menu, grid, recipe, 4096, false);
                        }
                        if stack {
                            return state.clicks(host, menu, None, vec![MenuClick::QuickMove { slot: grid.output }]);
                        }
                        let clicks = recipe::take_one(host, menu, &grid, &recipe.output);
                        return state.clicks(host, menu, None, clicks);
                    }
                    if stack { return state.clicks(host, menu, None, recipe::clear(host, menu, &grid.inputs)); }
                } else {
                    if host.setting_bool("store_on_fill").unwrap_or(false) {
                        state.store(host, menu, &grid);
                    }
                    if let Some(recipe) = state.selected_recipe(host) {
                        return state.fill(menu, grid, recipe, if stack { 64 } else { 1 });
                    }
                }
                return Flow::Continue;
            }
            if let Some(slot) = index && grid.inputs.contains(&slot) && all && dy > 0.0 {
                return state.clicks(host, menu, None, recipe::clear(host, menu, &grid.inputs));
            }
        }
        if e.screen.menu == Some("minecraft:merchant") && enabled(host, "villager_scroll") && index == Some(2) && stack {
            let live = menu.slot(host, 2).flatten().is_some();
            if live {
                let inputs: Vec<_> = (0..2).map(|slot| menu.slot(host, slot).flatten()).collect();
                state.trade_inputs = Some((menu, inputs));
                if dy > 0.0 {
                    return state.clicks(host, menu, None, vec![MenuClick::QuickMove { slot: 2 }]);
                }
            } else if dy > 0.0 {
                return state.clicks(host, menu, None, recipe::clear(host, menu, &[0, 1]));
            }
            if let Some((remembered, wanted)) = &state.trade_inputs && *remembered == menu
                && state.allows_slot(0) && state.allows_slot(1) && state.allows_slot(2)
                && let Some(kinds) = menu.slot_kinds(host)
            {
                let wanted = wanted.clone();
                let player = recipe::player_slots(&kinds).into_iter().filter(|&slot| state.allows_slot(slot)).collect();
                return state.enqueue(menu, Action::Refill {
                    inputs: vec![0, 1], player, wanted, sets: 1,
                });
            }
            return Flow::Continue;
        }
        let Some(index) = index else { return Flow::Continue };
        let upstream_all = stack && matching && host.setting_bool("upstream_scroll_all").unwrap_or(false);
        let (mode, toggle) = if all || upstream_all { (Mode::All, "scroll_all") }
            else if matching { (Mode::Matching, "scroll_matching") }
            else if stack { (Mode::Stack, "scroll_stack") }
            else { (Mode::One, "scroll_one") };
        if !enabled(host, toggle) { return Flow::Continue; }
        let positioned = host.setting_bool("position_scroll").unwrap_or(false).then(|| position_pull(host, menu, index, dy > 0.0)).flatten();
        let reverse = host.setting_bool("reverse").unwrap_or(false)
            ^ host.setting_bool(if mode == Mode::One { "reverse_one" } else { "reverse_stacks" }).unwrap_or(false);
        let pull = positioned.unwrap_or(dy < 0.0) ^ reverse;
        state.transfer(host, menu, index, pull, mode)
    }

    fn screen_render_post(&self, host: &dyn ClientHost, e: &mut mod_api::ScreenRenderPostEvent<'_>) {
        let mut state = self.0.lock().unwrap();
        if (state.paused_config && e.screen.id == "paused") || host.mod_screen() == Some(ID) {
            state.keys.draw(host, e);
            return;
        }
        if e.screen.id != "inventory" {
            return;
        }
        state.keys.load(host);
        if e.screen.menu == Some("minecraft:merchant") {
            if state.disabled || !state.open(host, e.screen) || !enabled(host, "trade_shortcuts") { return; }
            if host.millis() < state.favorite_error_until {
                let message = host.translate("item_scroller.favorite_error").unwrap_or_else(|| "Couldn’t save trade favorites.".into());
                e.draw.text(&message, [5.0, 5.0], 0xffff8888, true);
            }
            if let Some(menu) = active_menu(host)
                && let Some(trade) = trading::Trade::read(host, menu)
                && state.favorites.contains(host, &trade)
                && let Some([x, y, ..]) = menu.slot_boxes(host).and_then(|boxes| boxes.get(2).copied().flatten())
            {
                e.draw.text("★", [x, y - 9.0], 0xffffd45a, true);
            }
            return;
        }
        if !enabled(host, "crafting") { return; }
        if host.millis() < state.recipe_save_error_until {
            let message = host.translate("item_scroller.recipe_save_failed")
                .unwrap_or_else(|| "Couldn’t save this recipe.".into());
            let x = (e.draw.size()[0] - e.draw.text_width(&message)) * 0.5;
            e.draw.text(&message, [x.max(0.0), 5.0], 0xffff8888, true);
        }
        if !state.keys.held(host, "recipes") { return; }
        if state.disabled || !state.open(host, e.screen)
            || active_menu(host).and_then(|menu| state.grid(host, menu)).is_none()
        {
            return;
        }
        state.recipes.load(host);
        for (index, recipe) in state.recipes.entries.iter().enumerate() {
            let bounds @ [x, y, _, _] = recipe::panel_box(e.screen, index);
            let selected = state.recipes.selected == index;
            e.draw.fill(bounds, if selected { 0xffe2c06d } else { 0xff828282 });
            e.draw.fill([x + 1.0, y + 1.0, 18.0, 18.0], 0xff252525);
            if enabled(host, "recipe_items") && let Some(recipe) = recipe {
                e.draw.item_stack(&recipe.output, [x + 2.0, y + 2.0], 16.0);
            } else {
                e.draw.text(&(index + 1).to_string(), [x + 3.0, y + 5.0], 0xffdddddd, true);
            }
        }
    }

    fn client_tick_post(&self, host: &mut dyn ClientHost, _: &mut ClientTickPostEvent) {
        if host.consume_click(ID, "open_config") {
            if host.client_session().client_server().is_some_and(|server| server.integrated_local) {
                let _ = host.chat_line(mod_api::Text::translate("item_scroller.keys.open_from_pause", Vec::new()));
            } else {
                host.mod_screen_open(ID);
            }
            return;
        }
        let mut state = self.0.lock().unwrap();
        if std::mem::take(&mut state.close_config) {
            drop(state);
            host.mod_screen_close(ID);
            return;
        }
        if state.disabled { return; }
        if enabled(host, "sweep") && let Some(drag) = &state.drag {
            let (menu, button) = (drag.menu, drag.button);
            if active_menu(host) == Some(menu) && let Some(sweep) = sweep(&state.keys, host, button, state.drop_held(host))
                && let Some(slot) = hit(host, menu, host.cursor())
            {
                state.sweep_slot(host, menu, slot, sweep);
            }
        }
        if let Some(mut job) = state.pending.pop_front() {
            let id = state.menu_id.as_deref().unwrap_or("inventory");
            if job.run(host, &state.rules, id) { state.pending.push_front(job); }
            return;
        }
        if enabled(host, "crafting") && (state.mass_craft || state.keys.held(host, "mass_craft") && !state.keys.held(host, "mass_toggle"))
            && let Some(menu) = active_menu(host)
            && menu.slot_boxes(host).is_some()
            && state.menu_id.as_deref().is_some_and(|id| state.rules.menu_allowed(id))
            && let Some(grid) = state.grid(host, menu)
            && let Some(recipe) = state.selected_recipe(host)
        {
            state.craft(menu, grid, recipe, 1, true);
        }
    }
}

#[cfg(test)]
mod furnace_tests {
    use super::*;

    #[test]
    fn whole_stack_push_uses_furnace_quick_move_from_inventory_and_hotbar() {
        for id in ["minecraft:furnace", "minecraft:blast_furnace", "minecraft:smoker"] {
            for slot in [3, 29, 30, 38] {
                assert!(furnace_stack_push(id, slot, false, Mode::Stack));
            }
            assert!(!furnace_stack_push(id, 1, false, Mode::Stack));
            assert!(!furnace_stack_push(id, 3, true, Mode::Stack));
            assert!(!furnace_stack_push(id, 3, false, Mode::LeaveOne));
        }
        assert!(!furnace_stack_push("minecraft:generic_9x3", 3, false, Mode::Stack));
    }
}
