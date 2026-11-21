mod transfer;

use std::{collections::VecDeque, sync::Mutex};
use mod_api::{ClientHost, ClientTickPostEvent, Flow, Hooks, ItemStack, MenuClick,
    MenuRef, MenuSlotKind, MouseButton, ScreenMouseScrolledPreEvent, Tag, Registrar, RegistrationContext, KeyRegistration, KeyConflictContext, SettingSpec};
use transfer::{Click, Mode, Slot, transfer};

struct Gesture {
    menu: MenuRef,
    hovered: u16,
    pull: bool,
    mode: Mode,
    anchor: Option<ItemStack>,
}

#[derive(Default)]
struct Scroller {
    drops: Mutex<Option<(MenuRef, Vec<(usize, ItemStack)>)>>,
    bulk: Mutex<Option<(MenuRef, VecDeque<usize>)>>,
    drag_menu: Mutex<Option<MenuRef>>,
    sweep_item: Mutex<Option<(MenuRef, usize, ItemStack)>>,
    last_hover: Mutex<Option<(MenuRef, usize, bool)>>,
    pending: Mutex<VecDeque<Gesture>>,
}

fn side(kind: &MenuSlotKind, personal: bool) -> Option<bool> {
    match kind {
        MenuSlotKind::Player(i @ 0..=35) => Some(!personal || *i >= 9),
        MenuSlotKind::Storage => Some(false),
        _ => None,
    }
}

fn personal(kinds: &[MenuSlotKind]) -> bool {
    kinds.iter().any(|k| matches!(k, MenuSlotKind::Player(40)))
}

fn hit(host: &dyn ClientHost, menu: MenuRef, [x,y]: [f32;2]) -> Option<usize> {
    menu.slot_boxes(host)?.iter().position(|r| r.is_some_and(|[l,t,w,h]| x>=l && x<l+w && y>=t && y<t+h))
}

fn snapshot(host: &dyn ClientHost, menu: MenuRef) -> Option<Vec<Slot<ItemStack>>> {
    let mut slots = Vec::new();
    let kinds = menu.slot_kinds(host)?;
    let personal = personal(&kinds);
    for (index,kind) in kinds.iter().enumerate() {
        let Some(player_side) = side(kind,personal) else { continue };
        let mut item = menu.slot(host, index)?;
        let (mut capacity, mut count) = (99, 0);
        if let Some(stack) = &mut item {
            capacity = host.max_stack_size(stack.item())?;
            if let Some(patch) = stack.components() {
                let patch = patch.as_nbt();
                if patch.get(&"!minecraft:max_stack_size".into()).is_some() {
                    capacity = 1;
                } else if let Some(Tag::Int(n)) = patch.get(&"minecraft:max_stack_size".into()) {
                    capacity = (*n).clamp(1, 99) as u32;
                }
            }
            count = stack.count();
            stack.set_count(1).ok()?;
        }
        slots.push(Slot { id: index.try_into().ok()?, player_side, item, count, capacity });
    }
    Some(slots)
}

const BINDINGS: &[(&str,&str,u32)] = &[
    ("item_scroller.bind.0","key.keyboard.",0),
    ("item_scroller.bind.1","key.keyboard.m",0),
    ("item_scroller.bind.2","key.keyboard.x",0),
    ("item_scroller.bind.3","key.keyboard.left.shift",0),
    ("item_scroller.bind.4","key.keyboard.left.control",0),
    ("item_scroller.bind.5","key.keyboard.left.alt",0),
    ("item_scroller.bind.6","key.keyboard.left.alt",1),
    ("item_scroller.bind.7","key.keyboard.left.control",1),
    ("item_scroller.bind.8","key.keyboard.a",0),
    ("item_scroller.bind.9","key.keyboard.b",0),
    ("item_scroller.bind.10","key.keyboard.c",0),
    ("item_scroller.bind.11","key.keyboard.d",0),
    ("item_scroller.bind.12","key.keyboard.e",0),
    ("item_scroller.bind.13","key.keyboard.f",0),
    ("item_scroller.bind.14","key.keyboard.g",0),
    ("item_scroller.bind.15","key.keyboard.h",0),
    ("item_scroller.bind.16","key.keyboard.i",0),
    ("item_scroller.bind.17","key.keyboard.j",0),
    ("item_scroller.bind.18","key.keyboard.k",0),
    ("item_scroller.bind.19","key.keyboard.l",0),
    ("item_scroller.bind.20","key.keyboard.n",0),
    ("item_scroller.bind.21","key.keyboard.o",0),
    ("item_scroller.bind.22","key.keyboard.p",0),
    ("item_scroller.bind.23","key.keyboard.q",0),
    ("item_scroller.bind.24","key.keyboard.r",0),
    ("item_scroller.bind.25","key.keyboard.s",0),
    ("item_scroller.bind.26","key.keyboard.t",0),
    ("item_scroller.bind.27","key.keyboard.u",0),
    ("item_scroller.bind.28","key.keyboard.v",0),
    ("item_scroller.bind.29","key.keyboard.w",0),
    ("item_scroller.bind.30","key.keyboard.y",0),
    ("item_scroller.bind.31","key.keyboard.z",0),
];

fn held(host: &dyn ClientHost, action: &str) -> bool {
    let choice = host.setting_choice(action).unwrap_or(0) as usize;
    let configured;
    let (key, required) = if choice == 0 {
        if action == "drop_matching" {
            ("key.keyboard.left.control",1)
        } else {
            let Some((key, mods)) = host.mod_key("item_scroller",action) else { return false };
            configured = key;
            (configured.as_str(), mods.bits)
        }
    } else {
        let Some(&(_,key,mods)) = BINDINGS.get(choice) else { return false };
        (key,mods)
    };
    let mut bits = host.modifiers().bits;
    for (mask,left,right) in [(1,"key.keyboard.left.shift","key.keyboard.right.shift"),(2,"key.keyboard.left.control","key.keyboard.right.control"),(4,"key.keyboard.left.alt","key.keyboard.right.alt")] {
        if bits & mask == 0 && (host.is_key_down(left) || host.is_key_down(right)) { bits |= mask; }
    }
    let down = match key {
        "key.keyboard.left.shift" => bits & 1 != 0,
        "key.keyboard.left.control" => bits & 2 != 0,
        "key.keyboard.left.alt" => bits & 4 != 0,
        _ => host.is_key_down(key),
    };
    down && bits & required == required
}

impl Scroller {
    fn queue_slot(&self, host: &dyn ClientHost, menu: MenuRef, mouse: [f32;2]) -> Flow {
        let [mx,my] = mouse;
        let Some(index) = menu.slot_boxes(host).and_then(|boxes| boxes.iter().position(|r|
            r.is_some_and(|[x,y,w,h]| mx>=x && mx<x+w && my>=y && my<y+h))) else { return Flow::Continue };
        let Some(kinds) = menu.slot_kinds(host) else { return Flow::Continue };
        if (!personal(&kinds) && !kinds.contains(&MenuSlotKind::Storage)) || !matches!(kinds.get(index), Some(MenuSlotKind::Storage | MenuSlotKind::Player(0..=35))) { return Flow::Continue; }
        let matching = held(host,"matching");
        if *self.last_hover.lock().unwrap() == Some((menu,index,matching)) { return Flow::Cancel; }
        *self.last_hover.lock().unwrap() = Some((menu,index,matching));
        if let Some(mut item) = menu.slot(host,index).flatten() {
            let _ = item.set_count(1);
            *self.sweep_item.lock().unwrap() = Some((menu,index,item.clone()));
            self.pending.lock().unwrap().push_back(Gesture {menu, hovered:index as u16, pull:false, mode:if matching {Mode::Matching} else {Mode::Stack}, anchor:Some(item)});
        }
        Flow::Cancel
    }
}

#[mod_api::horizon_mod]
impl Hooks for Scroller {
    fn register(&self, r: &mut dyn Registrar, _: &RegistrationContext) {
        for (id,key,label) in [("stack","key.keyboard.left.shift","key.item_scroller.stack"),("matching","key.keyboard.left.control","key.item_scroller.matching")] {
            r.register_key_mapping(KeyRegistration::new(id,key).label(label).context(KeyConflictContext::Gui));
        }
        r.register_key_mapping(KeyRegistration::new("deposit","key.keyboard.left.alt")
            .modifiers(mod_api::ClientModifiers::new(1)).label("key.item_scroller.deposit").context(KeyConflictContext::Gui));
        r.register_key_mapping(KeyRegistration::new("drop_all","key.keyboard.x").label("key.item_scroller.drop_all").context(KeyConflictContext::Gui));
        for id in ["stack","matching","deposit","drop_matching","drop_all"] {
            let _ = r.register_setting(SettingSpec::choice(id,format!("item_scroller.config.{id}"),BINDINGS.iter().map(|b| b.0),0));
        }
        for (id,label,default) in [("sweep","item_scroller.setting.sweep",true),("reverse","item_scroller.setting.reverse",false)] {
            let _ = r.register_setting(SettingSpec::toggle(id,label,default));
        }
    }

    fn screen_key_pressed_pre(&self, host: &dyn ClientHost, e: &mut mod_api::ScreenKeyPressedPreEvent<'_>) -> Flow {
        if e.screen.id != "inventory" { return Flow::Continue; }
        let all = held(host,"drop_all");
        if !all && !held(host,"drop_matching") { return Flow::Continue; }
        let label = |key: &str| host.text_string(&mod_api::Text::new(mod_api::text::TextContent::Keybind(key.into()))).ok();
        let Some(drop_key) = label("key.drop") else { return Flow::Continue };
        if Some(drop_key) != label(e.key) { return Flow::Continue; }
        if e.repeat { return Flow::Cancel; }
        let Some(menu) = host.local_player().and_then(|p| p.container_menu(host)) else { return Flow::Continue };
        if menu.carried(host) != Some(None) { return Flow::Continue; }
        let Some(index) = hit(host,menu,e.screen.mouse) else { return Flow::Continue };
        let Some(kinds) = menu.slot_kinds(host) else { return Flow::Continue };
        let Some(source) = kinds.get(index).and_then(|k| side(k,false)) else { return Flow::Continue };
        let anchor = menu.slot(host,index).flatten();
        if !all && anchor.is_none() { return Flow::Continue; }
        let stacks = kinds.iter().enumerate().filter(|(_,k)| side(k,false)==Some(source))
            .filter_map(|(i,_)| Some((i,menu.slot(host,i)??)))
            .filter(|(_,s)| all || anchor.as_ref().is_some_and(|a| a.same_item(s))).collect();
        *self.drops.lock().unwrap() = Some((menu,stacks));
        Flow::Cancel
    }

    fn screen_mouse_button_pressed_pre(&self, host: &dyn ClientHost, e: &mut mod_api::ScreenMouseButtonPressedPreEvent<'_>) -> Flow {
        if e.button != 0 || e.screen.id != "inventory" {
            return Flow::Continue;
        }
        let Some(menu) = host.local_player().and_then(|p| p.container_menu(host)) else { return Flow::Continue };
        if menu.carried(host) != Some(None) { return Flow::Continue; }
        if held(host,"deposit") {
            let Some(kinds) = menu.slot_kinds(host) else { return Flow::Continue };
            if !personal(&kinds) && !kinds.contains(&MenuSlotKind::Storage) { return Flow::Continue; }
            let player_source = {
                let [x,y] = e.screen.mouse;
                let Some(slot) = menu.slot_boxes(host).and_then(|boxes| boxes.iter().position(|r|
                    r.is_some_and(|[l,t,w,h]| x>=l && x<l+w && y>=t && y<t+h))) else { return Flow::Continue };
                let Some(source) = kinds.get(slot).and_then(|k| side(k,personal(&kinds))) else { return Flow::Continue };
                source
            };
            let personal = personal(&kinds);
            let sources = kinds.iter().enumerate().filter(|(_,kind)| side(kind,personal)==Some(player_source)).map(|(i,_)| i).collect();
            *self.bulk.lock().unwrap() = Some((menu,sources));
            return Flow::Cancel;
        }
        if !(held(host,"stack") || held(host,"matching")) { return Flow::Continue; }
        self.last_hover.lock().unwrap().take();
        let flow = self.queue_slot(host, menu, e.screen.mouse);
        if matches!(flow, Flow::Cancel) { *self.drag_menu.lock().unwrap() = Some(menu); }
        flow
    }

    fn screen_mouse_dragged_pre(&self, host: &dyn ClientHost, e: &mut mod_api::ScreenMouseDraggedPreEvent<'_>) -> Flow {
        let menu = *self.drag_menu.lock().unwrap();
        if e.button == 0 && host.setting_bool("sweep").unwrap_or(true) && (held(host,"stack") || held(host,"matching")) && let Some(menu) = menu {
            return self.queue_slot(host,menu,e.screen.mouse);
        }
        Flow::Continue
    }

    fn screen_mouse_button_released_pre(&self, _: &dyn ClientHost, e: &mut mod_api::ScreenMouseButtonReleasedPreEvent<'_>) -> Flow {
        if e.button == 0 && self.drag_menu.lock().unwrap().take().is_some() { Flow::Cancel } else { Flow::Continue }
    }

    fn screen_closing(&self, _: &dyn ClientHost, _: &mut mod_api::ScreenClosingEvent<'_>) {
        self.drag_menu.lock().unwrap().take();
        self.drops.lock().unwrap().take();
        self.bulk.lock().unwrap().take();
        self.pending.lock().unwrap().clear();
    }

    fn screen_mouse_scrolled_pre(&self, host: &dyn ClientHost, e: &mut ScreenMouseScrolledPreEvent<'_>) -> Flow {
        let dy = e.scroll[1];
        if e.screen.id != "inventory" || !dy.is_finite() || dy == 0.0 {
            return Flow::Continue;
        }
        let Some(menu) = host.local_player().and_then(|p| p.container_menu(host)) else { return Flow::Continue };
        let [mx, my] = e.screen.mouse;
        let Some(index) = menu.slot_boxes(host).and_then(|boxes| boxes.iter().position(|rect|
            rect.is_some_and(|[x,y,w,h]| mx >= x && my >= y && mx < x+w && my < y+h))) else { return Flow::Continue };
        let Some(slots) = snapshot(host, menu) else { return Flow::Continue };
        let Some(anchor) = slots.iter().find(|s| usize::from(s.id) == index) else { return Flow::Continue };
        let mode = if held(host,"deposit") { Mode::All } else if held(host,"matching") { Mode::Matching } else if held(host,"stack") { Mode::Stack } else { Mode::One };
        let pull = (dy < 0.0) != host.setting_bool("reverse").unwrap_or(false);
        if transfer(&slots, anchor.id, pull, mode, menu.carried(host) == Some(None)).is_empty() {
            return Flow::Continue;
        }
        let mut pending = self.pending.lock().unwrap();
        if pending.len() < 16 {
            pending.push_back(Gesture { menu, hovered: anchor.id, pull, mode, anchor: anchor.item.clone() });
        }
        Flow::Cancel
    }

    fn client_tick_post(&self, host: &mut dyn ClientHost, _: &mut ClientTickPostEvent) {
        let drops = self.drops.lock().unwrap().take();
        if let Some((menu,stacks)) = drops {
            if menu.slot_boxes(host).is_some() && menu.carried(host)==Some(None) {
                let clicks:Vec<_> = stacks.into_iter().filter(|(i,s)| menu.slot(host,*i).flatten().as_ref()==Some(s))
                    .map(|(slot,_)| MenuClick::Throw {slot,whole_stack:true}).collect();
                if !clicks.is_empty() {menu.click(host,&clicks);}
            }
            return;
        }
        let bulk = self.bulk.lock().unwrap().take();
        if let Some((menu, mut sources)) = bulk {
            if menu.carried(host) == Some(None) && menu.slot_boxes(host).is_some() {
                let batch = sources.len().min(8);
                let moves: Vec<_> = sources.drain(..batch).filter(|i| menu.slot(host,*i).flatten().is_some())
                    .map(|slot| MenuClick::QuickMove {slot}).collect();
                if (moves.is_empty() || menu.click(host,&moves)) && !sources.is_empty() {
                    *self.bulk.lock().unwrap() = Some((menu,sources));
                }
            }
            return;
        }
        let queued = self.pending.lock().unwrap().pop_front();
        let active_drag = *self.drag_menu.lock().unwrap();
        let sweep = active_drag.is_some() && host.setting_bool("sweep").unwrap_or(true) && (held(host,"stack") || held(host,"matching"));
        let hovered = sweep.then(|| {
            let menu = host.local_player()?.container_menu(host)?;
            if active_drag != Some(menu) { return None; }
            let [mx,my] = host.cursor();
            let index = menu.slot_boxes(host)?.iter().position(|r| r.is_some_and(|[x,y,w,h]| mx>=x && mx<x+w && my>=y && my<y+h))?;
            Some((menu,index,held(host,"matching")))
        }).flatten();
        if hovered.is_none() { self.sweep_item.lock().unwrap().take(); }
        let changed = {
            let mut last = self.last_hover.lock().unwrap();
            let changed = *last != hovered;
            *last = hovered;
            changed
        };
        let g = if let Some(queued) = queued { queued } else {
            let Some((menu,index,matching)) = hovered.filter(|_| changed) else { return };
            if menu.carried(host) != Some(None) { return }
            let current = menu.slot(host,index).flatten();
            let previous = self.sweep_item.lock().unwrap().take();
            let Some(mut item) = current.or_else(|| previous.filter(|(m,i,_)| matching && *m==menu && *i==index).map(|(_,_,item)| item)) else { return };
            let _ = item.set_count(1);
            *self.sweep_item.lock().unwrap() = Some((menu,index,item.clone()));
            Gesture { menu, hovered:index as u16, pull:false, mode:if matching {Mode::Matching} else {Mode::Stack}, anchor:Some(item) }
        };
        if host.local_player().and_then(|p| p.container_menu(host)) != Some(g.menu)
            || g.menu.slot_boxes(host).is_none() || g.menu.carried(host) != Some(None) { return; }
        let Some(slots) = snapshot(host, g.menu) else { return };
        let Some(anchor) = slots.iter().find(|s| s.id == g.hovered && (s.item == g.anchor || (g.mode == Mode::Matching && s.item.is_none() && !g.pull))) else { return };
        if !slots.iter().any(|s| s.player_side != anchor.player_side) { return; }
        let clicks: Vec<_> = match g.mode {
            Mode::All | Mode::Matching => slots.iter()
                .filter(|s| s.player_side == (anchor.player_side != g.pull) && s.count > 0
                    && (g.mode == Mode::All || s.item == g.anchor))
                .map(|s| MenuClick::QuickMove { slot: usize::from(s.id) }).collect(),
            _ => transfer(&slots, g.hovered, g.pull, g.mode, true).into_iter().map(|click| {
                let (id, button) = match click {
                    Click::Left(id) => (id, MouseButton::Left),
                    Click::Right(id) => (id, MouseButton::Right),
                };
                MenuClick::Pickup { slot: Some(usize::from(id)), button }
            }).collect(),
        };
        if !clicks.is_empty() { g.menu.click(host, &clicks); }
    }
}
