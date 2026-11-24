use std::collections::BTreeMap;

use mod_api::{ClientHost, ConfigPath, DrawContext, Flow, ScreenRenderPostEvent};

use crate::contains;

const ACTIONS: &[(&str, &str, u32)] = &[
    ("stack", "left.shift", 0), ("matching", "left.control", 0),
    ("deposit", "left.alt", 1), ("drop_matching", "left.control", 1),
    ("drop_all", "x", 0), ("matching_click", "left.alt", 0),
    ("move_up", "w", 0), ("move_down", "s", 0), ("toggle", "s", 2),
    ("sort", "r", 0), ("recipes", "s", 0), ("craft_all", "c", 1),
    ("mass_craft", "c", 2), ("mass_toggle", "c", 3),
    ("move_results", "m", 1), ("drop_results", "q", 5), ("debug", "i", 7),
];
const PREFIX: &str = "key.keyboard.";
const FILE: &str = "keybinds.txt";
const MODIFIERS: &[(u32, &str)] = &[(2, "control"), (4, "alt"), (1, "shift"), (8, "win")];

fn modifier(key: &str) -> u32 {
    let Some((side, name)) = key.split_once('.') else { return 0 };
    if !matches!(side, "left" | "right") { return 0; }
    MODIFIERS.iter().find(|(_, candidate)| *candidate == name).map_or(0, |(bit, _)| *bit)
}

fn label(host: &dyn ClientHost, part: &str, fallback: &str) -> String {
    host.translate(&format!("item_scroller.keys.{part}")).unwrap_or_else(|| fallback.into())
}

fn action_label(host: &dyn ClientHost, action: &str) -> String {
    let key = if action == "drop_matching" { "item_scroller.config.drop_matching".into() }
        else { format!("key.item_scroller.{action}") };
    host.translate(&key).unwrap_or_else(|| action.into())
}

fn key_label(host: &dyn ClientHost, key: &str) -> String {
    host.translate(&format!("{PREFIX}{key}")).unwrap_or_else(|| key.replace('.', " ").to_uppercase())
}

fn display(host: &dyn ClientHost, key: &str, mods: u32) -> String {
    if key.is_empty() { return label(host, "unbound", "Unbound"); }
    let mut parts: Vec<_> = MODIFIERS.iter().filter(|(bit, _)| mods & bit != 0)
        .map(|(_, name)| match *name { "control" => "Ctrl", "alt" => "Alt", "shift" => "Shift", _ => "Super" }.to_owned()).collect();
    parts.push(key_label(host, key));
    parts.join(" + ")
}

#[derive(Clone, Copy, Default)]
enum Page {
    #[default]
    Actions,
    Capture(usize),
    Options(usize, u32),
}

enum Command {
    Capture(usize),
    Options(usize),
    Modifier(usize, u32),
    Set(usize, String, u32),
    Back,
    Done,
}

struct Button([f32; 4], String, Command);

#[derive(Default)]
pub struct Editor {
    bindings: BTreeMap<String, (String, u32)>,
    loaded: bool,
    page: Page,
    offset: usize,
    max_offset: usize,
    actions_offset: usize,
    pending_modifier: Option<(String, u32)>,
    buttons: Vec<Button>,
    save_failed: bool,
}

impl Editor {
    pub fn load(&mut self, host: &dyn ClientHost) {
        if self.loaded { return; }
        self.loaded = true;
        let path = ConfigPath::new(FILE).unwrap();
        match host.data_files().read(&path) {
            Ok(bytes) => {
                let Ok(text) = String::from_utf8(bytes) else { self.save_failed = true; return };
                for line in text.lines() {
                    let mut fields = line.split('\t');
                    if let (Some(action), Some(key), Some(mods), None) =
                        (fields.next(), fields.next(), fields.next(), fields.next())
                        && ACTIONS.iter().any(|(candidate, ..)| *candidate == action)
                        && let Ok(mods @ 0..=15) = mods.parse()
                    {
                        self.bindings.insert(action.into(), (key.into(), mods));
                    }
                }
            }
            Err(mod_api::ConfigFileError::NotFound) => {}
            Err(_) => self.save_failed = true,
        }
    }

    fn binding(&self, action: &str) -> (&str, u32) {
        self.bindings.get(action).map(|(key, mods)| (key.as_str(), *mods)).unwrap_or_else(|| {
            let &(_, key, mods) = ACTIONS.iter().find(|(id, ..)| *id == action).unwrap();
            (key, mods)
        })
    }

    pub fn held(&self, host: &dyn ClientHost, action: &str) -> bool {
        let (key, required) = self.binding(action);
        if key.is_empty() { return false; }
        let mut mods = host.modifiers().bits;
        for (bit, name) in MODIFIERS {
            if mods & bit == 0 && ["left", "right"].iter().any(|side| {
                host.is_key_down(&format!("{PREFIX}{side}.{name}"))
            }) { mods |= bit; }
        }
        let down = key.starts_with("left.") && mods & modifier(key) != 0;
        (down || host.is_key_down(&format!("{PREFIX}{key}"))) && mods & required == required
    }

    pub fn bound(&self, action: &str, key: &str, modifiers: u32) -> bool {
        let (mapped, required) = self.binding(action);
        !mapped.is_empty() && key.strip_prefix(PREFIX) == Some(mapped)
            && modifiers & 15 & !modifier(mapped) == required
    }

    fn set(&mut self, host: &dyn ClientHost, index: usize, key: String, mods: u32) {
        let mut updated = self.bindings.clone();
        updated.insert(ACTIONS[index].0.into(), (key, mods));
        let text: String = updated.iter().map(|(action, (key, mods))| format!("{action}\t{key}\t{mods}\n")).collect();
        let path = ConfigPath::new(FILE).unwrap();
        self.save_failed = host.data_files().write(&path, text.as_bytes()).is_err();
        if !self.save_failed { self.bindings = updated; }
        self.back();
    }

    fn back(&mut self) {
        self.page = Page::Actions;
        self.offset = self.actions_offset;
        self.pending_modifier = None;
        self.buttons.clear();
    }

    pub fn close(&mut self) {
        self.actions_offset = 0;
        self.back();
        self.save_failed = false;
    }

    pub fn press(&mut self, host: &dyn ClientHost, key: &str, mods: u32, repeat: bool) -> Flow {
        let Some(key) = key.strip_prefix(PREFIX) else { return Flow::Cancel };
        if key == "escape" {
            if matches!(self.page, Page::Actions) { return Flow::Continue; }
            self.back();
        } else if let Page::Capture(index) = self.page && !repeat && key != "unknown" {
            let bit = modifier(key);
            let mods = mods & 15 & !bit;
            if bit != 0 {
                self.pending_modifier = Some((key.into(), mods));
            } else {
                let unbind = key == "backspace" && mods == 0;
                self.set(host, index, if unbind { String::new() } else { key.into() }, mods);
            }
        }
        Flow::Cancel
    }

    pub fn release(&mut self, host: &dyn ClientHost, key: &str) {
        if let Page::Capture(index) = self.page
            && self.pending_modifier.as_ref().is_some_and(|(pending, _)| key.strip_prefix(PREFIX) == Some(pending))
            && let Some((key, mods)) = self.pending_modifier.take()
        {
            self.set(host, index, key, mods);
        }
    }

    pub fn click(&mut self, host: &dyn ClientHost, point: [f32; 2]) -> bool {
        let Some(index) = self.buttons.iter().position(|button| contains(button.0, point)) else { return false };
        let Button(_, _, command) = self.buttons.swap_remove(index);
        match command {
            Command::Capture(index) => {
                self.actions_offset = self.offset;
                self.page = Page::Capture(index);
                self.buttons.clear();
            }
            Command::Options(index) => {
                self.actions_offset = self.offset;
                self.offset = 0;
                self.page = Page::Options(index, self.binding(ACTIONS[index].0).1);
                self.buttons.clear();
            }
            Command::Modifier(index, mods) => {
                self.page = Page::Options(index, mods);
                self.buttons.clear();
            }
            Command::Set(index, key, mods) => self.set(host, index, key, mods),
            Command::Back => self.back(),
            Command::Done => return true,
        }
        false
    }

    pub fn scroll(&mut self, delta: f64) {
        if matches!(self.page, Page::Capture(_)) || !delta.is_finite() { return; }
        self.offset = self.offset.saturating_add_signed(-delta.signum() as isize).min(self.max_offset);
        self.buttons.clear();
    }

    pub fn draw(&mut self, host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
        self.load(host);
        self.buttons.clear();
        let draw = &mut *event.draw;
        let [width, height] = draw.size();
        let panel = (width - 20.0).clamp(120.0, 430.0);
        let x = (width - panel) / 2.0;
        let available = ((height - 80.0) / 20.0).floor().max(1.0) as usize;
        let background = if event.screen.id == "paused" { 0xff101218 } else { 0xd5101218 };
        draw.fill([0.0, 0.0, width, height], background);
        let title = label(host, "title", "Item Scroller Key Binds");
        draw.text(&title, [x, 14.0], 0xffffffff, true);
        match self.page {
            Page::Actions => {
                self.max_offset = ACTIONS.len().saturating_sub(available);
                self.offset = self.offset.min(self.max_offset);
                for (row, (index, &(action, ..))) in ACTIONS.iter().enumerate().skip(self.offset).take(available).enumerate() {
                    let y = 34.0 + row as f32 * 20.0;
                    let name = action_label(host, action);
                    let (key, mods) = self.binding(action);
                    let options_width = (draw.text_width(&label(host, "options", "Options")) + 12.0).max(48.0);
                    let key_width = panel * 0.33;
                    let name_width = panel - options_width - key_width - 10.0;
                    draw.text(&fit(draw, &name, name_width), [x, y + 5.0], 0xffffffff, true);
                    self.buttons.push(Button([x + name_width + 4.0, y, key_width, 18.0],
                        display(host, key, mods), Command::Capture(index)));
                    self.buttons.push(Button([x + panel - options_width, y, options_width, 18.0],
                        label(host, "options", "Options"), Command::Options(index)));
                }
                self.buttons.push(Button([x + panel - 62.0, height - 32.0, 62.0, 18.0],
                    host.translate("gui.done").unwrap_or_else(|| "Done".into()), Command::Done));
                if self.max_offset > 0 {
                    let first = self.offset + 1;
                    let last = (self.offset + available).min(ACTIONS.len());
                    draw.text(&format!("{first}–{last} / {}", ACTIONS.len()), [x, height - 31.0], 0xffcccccc, true);
                }
            }
            Page::Capture(index) => {
                draw.text(&action_label(host, ACTIONS[index].0), [x, 36.0], 0xffffffff, true);
                for (i, (part, fallback)) in [
                    ("capture", "Press a key or key combination"),
                    ("clear", "Backspace to unbind"),
                    ("cancel", "Esc to cancel"),
                ].into_iter().enumerate() {
                    draw.text(&label(host, part, fallback), [x, 56.0 + i as f32 * 15.0], 0xffdedede, true);
                }
            }
            Page::Options(index, mods) => {
                draw.text(&action_label(host, ACTIONS[index].0), [x, 30.0], 0xffffffff, true);
                let button_width = panel / 4.0;
                for (i, &(bit, name)) in MODIFIERS.iter().enumerate() {
                    let name = key_label(host, &format!("left.{name}"));
                    let text = if mods & bit != 0 { format!("[{name}]") } else { name };
                    self.buttons.push(Button([x + i as f32 * button_width, 44.0, button_width - 3.0, 18.0],
                        text, Command::Modifier(index, mods ^ bit)));
                }
                let (_, default_key, default_mods) = ACTIONS[index];
                let mut choices = vec![
                    (label(host, "reset", "Reset to default"), default_key.into(), default_mods),
                    (label(host, "unbound", "Unbound"), String::new(), 0),
                ];
                let names = ('a'..='z').chain('0'..='9').map(|c| c.to_string())
                    .chain((1..=12).map(|n| format!("f{n}")))
                    .chain(["space", "tab", "enter", "backspace", "left", "right", "up", "down",
                        "left.shift", "right.shift", "left.control", "right.control", "left.alt", "right.alt"]
                        .map(String::from));
                choices.extend(names.map(|key| {
                    let mods = mods & !modifier(&key);
                    (display(host, &key, mods), key, mods)
                }));
                let rows = ((height - 108.0) / 20.0).floor().max(1.0) as usize;
                self.max_offset = choices.len().div_ceil(2).saturating_sub(rows);
                self.offset = self.offset.min(self.max_offset);
                let half = panel / 2.0;
                for (i, (text, key, mods)) in choices.into_iter().skip(self.offset * 2).take(rows * 2).enumerate() {
                    let y = 66.0 + (i / 2) as f32 * 20.0;
                    self.buttons.push(Button([x + (i % 2) as f32 * half, y, half - 3.0, 18.0],
                        text, Command::Set(index, key, mods)));
                }
                self.buttons.push(Button([x, height - 25.0, half - 3.0, 18.0], label(host, "back", "Back"), Command::Back));
            }
        }
        for Button(rect @ [bx, by, bw, _], text, _) in &self.buttons {
            draw.fill(*rect, if contains(*rect, event.screen.mouse) { 0xff464e63 } else { 0xff2d3444 });
            let text = fit(draw, text, bw - 6.0);
            draw.text(&text, [bx + (bw - draw.text_width(&text)) / 2.0, by + 5.0], 0xffffffff, true);
        }
        if self.save_failed {
            draw.text(&label(host, "save_error", "Couldn’t save key bindings."), [x, height - 12.0], 0xffff8888, true);
        }
    }
}

fn fit(draw: &dyn DrawContext, text: &str, width: f32) -> String {
    if draw.text_width(text) <= width { return text.into(); }
    for (i, _) in text.char_indices().rev() {
        let shortened = format!("{}…", &text[..i]);
        if draw.text_width(&shortened) <= width { return shortened; }
    }
    String::new()
}
