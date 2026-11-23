use std::collections::{HashMap, HashSet};

use mod_api::{ClientHost, ConfigPath};

#[derive(Default)]
pub struct Rules {
    menus: HashSet<String>,
    slots: HashMap<String, HashSet<usize>>,
    pub grids: HashMap<String, (usize, Vec<usize>)>,
    pub first: Vec<String>,
    pub last: Vec<String>,
}

impl Rules {
    pub fn load(host: &dyn ClientHost) -> Self {
        let path = ConfigPath::new("item_scroller/rules.txt").unwrap();
        host.config_files().read(&path).ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .map_or_else(Self::default, |text| Self::parse(&text))
    }

    fn parse(text: &str) -> Self {
        let mut rules = Self::default();
        for line in text.lines().map(str::trim) {
            let Some((command, value)) = line.split_once(char::is_whitespace) else {
                continue;
            };
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            match command {
                "menu" => { rules.menus.insert(value.to_owned()); }
                "slot" => {
                    let Some((menu, slot)) = value.split_once(char::is_whitespace) else { continue };
                    if let Ok(slot) = slot.trim().parse() {
                        rules.slots.entry(menu.to_owned()).or_default().insert(slot);
                    }
                }
                "grid" => {
                    let mut parts = value.split_whitespace();
                    let (Some(menu), Some(output), Some(range), None) =
                        (parts.next(), parts.next(), parts.next(), parts.next()) else { continue };
                    let Some((first, last)) = range.split_once('-') else { continue };
                    let (Ok(output), Ok(first), Ok(last)) =
                        (output.parse::<usize>(), first.parse::<usize>(), last.parse::<usize>()) else { continue };
                    if matches!(last.checked_sub(first), Some(3 | 8)) && !(first..=last).contains(&output) {
                        rules.grids.insert(menu.to_owned(), (output, (first..=last).collect()));
                    }
                }
                "first" => rules.first.push(value.to_owned()),
                "last" => rules.last.push(value.to_owned()),
                _ => {}
            }
        }
        rules
    }

    pub fn menu_allowed(&self, menu: &str) -> bool {
        !self.menus.contains(menu)
    }

    pub fn blocked(&self, menu: &str) -> Option<&HashSet<usize>> {
        self.slots.get(menu)
    }
}
