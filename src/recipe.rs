use mod_api::{ClientHost, Compound, ConfigPath, ItemStack, MenuClick, MenuRef, MenuSlotKind, ScreenView, Tag};

use crate::{matching, pickup, right};

pub const COUNT: usize = 18;
const FILE: &str = "recipes.nbt";

#[derive(Clone, Debug, PartialEq)]
pub struct Recipe {
    pub output: ItemStack,
    pub inputs: Vec<Option<ItemStack>>,
}

#[derive(Default)]
pub struct Recipes {
    pub entries: [Option<Recipe>; COUNT],
    pub selected: usize,
    loaded: bool,
    dirty: [bool; COUNT],
}

const READ_QUOTA: u64 = 4 << 20;

fn decode_records(bytes: &[u8]) -> Option<Compound> {
    let (Tag::Compound(records), end) = Tag::from_network_bytes(bytes, Some(READ_QUOTA)).ok()? else {
        return None;
    };
    (end == bytes.len()).then_some(records)
}

fn read_records(host: &dyn ClientHost, path: &ConfigPath) -> Option<Compound> {
    match host.data_files().read(path) {
        Ok(bytes) => decode_records(&bytes),
        Err(mod_api::ConfigFileError::NotFound) => Some(Compound::new()),
        Err(_) => None,
    }
}

fn updated_file(mut records: Compound, updates: Vec<(usize, Tag)>) -> Option<Vec<u8>> {
    for (index, record) in updates {
        records.insert(index.to_string().as_str().into(), record);
    }
    let bytes = Tag::Compound(records).to_network_bytes().ok()?;
    decode_records(&bytes)?;
    Some(bytes)
}

impl Recipe {
    fn record(&self, host: &dyn ClientHost) -> Option<Tag> {
        let cells = self.inputs.iter().map(|stack| {
            let cell = match stack {
                Some(stack) => stack.to_nbt(host)?,
                None => Compound::new(),
            };
            Some(Tag::Compound(cell))
        }).collect::<Option<Vec<_>>>()?;
        Some(Tag::compound([
            ("output", Tag::Compound(self.output.to_nbt(host)?)),
            ("cells", Tag::List(cells)),
        ]))
    }
}

impl Recipes {
    pub fn load(&mut self, host: &dyn ClientHost) {
        if self.loaded || !host.setting_bool("save_recipes").unwrap_or(true) {
            return;
        }
        let path = ConfigPath::new(FILE).unwrap();
        let Some(data) = read_records(host, &path) else { return };
        self.loaded = true;
        for (index, entry) in self.entries.iter_mut().enumerate() {
            if entry.is_some() {
                continue;
            }
            let Some(Tag::Compound(record)) = data.get(&index.to_string().as_str().into()) else { continue };
            let (Some(Tag::Compound(output)), Some(Tag::List(cells))) =
                (record.get(&"output".into()), record.get(&"cells".into())) else { continue };
            let Some(output) = ItemStack::from_nbt(host, output) else { continue };
            if !matches!(cells.len(), 4 | 9) {
                continue;
            }
            let inputs = cells.iter().map(|cell| match cell {
                Tag::Compound(cell) if cell.is_empty() => Some(None),
                Tag::Compound(cell) => {
                    let mut stack = ItemStack::from_nbt(host, cell)?;
                    stack.set_count(1).ok()?;
                    Some(Some(stack))
                }
                _ => None,
            }).collect::<Option<Vec<_>>>();
            if let Some(inputs) = inputs {
                *entry = Some(Recipe { output, inputs });
            }
        }
    }

    pub fn store(&mut self, host: &dyn ClientHost, recipe: Recipe) {
        self.entries[self.selected] = Some(recipe);
        self.dirty[self.selected] = true;
        if !host.setting_bool("save_recipes").unwrap_or(true) {
            return;
        }
        let path = ConfigPath::new(FILE).unwrap();
        let Some(records) = read_records(host, &path) else { return };
        let updates = self.dirty.iter().enumerate().filter(|(_, dirty)| **dirty)
            .map(|(i, _)| Some((i, self.entries[i].as_ref()?.record(host)?)))
            .collect::<Option<Vec<_>>>();
        if let Some(bytes) = updates.and_then(|updates| updated_file(records, updates))
            && host.data_files().write(&path, &bytes).is_ok()
        {
            self.dirty.fill(false);
        }
    }
}

pub struct Grid {
    pub output: usize,
    pub inputs: Vec<usize>,
    pub player: Vec<usize>,
}

impl Grid {
    pub fn read(
        host: &dyn ClientHost, menu: MenuRef, menu_id: &str, layout: Option<&(usize, Vec<usize>)>,
    ) -> Option<Self> {
        if menu_id == "minecraft:merchant" {
            return None;
        }
        let kinds = menu.slot_kinds(host)?;
        let (output, inputs) = if let Some((output, inputs)) = layout {
            let boxes = menu.slot_boxes(host)?;
            if std::iter::once(output).chain(inputs).any(|&slot| {
                !boxes.get(slot).is_some_and(Option::is_some)
                    || kinds.get(slot).is_none_or(|kind| matches!(kind, MenuSlotKind::Player(_) | MenuSlotKind::Equipment))
            }) {
                return None;
            }
            (*output, inputs.clone())
        } else {
            let inputs: Vec<_> = kinds.iter().enumerate().filter_map(|(i, kind)|
                (*kind == MenuSlotKind::CraftingGrid).then_some(i)).collect();
            // Multiple outputs leave the grid's result ambiguous.
            let mut outputs = kinds.iter().enumerate().filter_map(|(i, kind)|
                (*kind == MenuSlotKind::Result).then_some(i));
            let output = outputs.next()?;
            if outputs.next().is_some() || !matches!(inputs.len(), 4 | 9) {
                return None;
            }
            (output, inputs)
        };
        Some(Self { output, inputs, player: player_slots(&kinds) })
    }

    pub fn remember(&self, host: &dyn ClientHost, menu: MenuRef) -> Option<Recipe> {
        let output = menu.slot(host, self.output)??;
        let inputs = self.inputs.iter().map(|&slot| {
            let mut stack = menu.slot(host, slot)?;
            if let Some(stack) = &mut stack {
                stack.set_count(1).ok()?;
            }
            Some(stack)
        }).collect::<Option<Vec<_>>>()?;
        inputs.iter().any(Option::is_some).then_some(Recipe { output, inputs })
    }
}

pub fn player_slots(kinds: &[MenuSlotKind]) -> Vec<usize> {
    kinds.iter().enumerate().filter_map(|(i, kind)|
        matches!(kind, MenuSlotKind::Player(0..=35)).then_some(i)).collect()
}

pub fn panel_box(screen: &ScreenView<'_>, index: usize) -> [f32; 4] {
    let [x, y, width, _] = screen.rect;
    let left = if x >= 66.0 { x - 64.0 } else { x + width - 58.0 };
    [left + (index % 3) as f32 * 20.0, y + (index / 3) as f32 * 20.0, 20.0, 20.0]
}

pub fn clear(host: &dyn ClientHost, menu: MenuRef, inputs: &[usize]) -> Vec<MenuClick> {
    inputs.iter().filter(|&&slot| menu.slot(host, slot).is_some_and(|s| s.is_some()))
        .map(|&slot| MenuClick::QuickMove { slot }).collect()
}

/// Fill one cell per tick. If a previous transaction did not take effect the
/// caller waits for the server instead of applying more cursor clicks.
pub fn refill(
    host: &dyn ClientHost,
    menu: MenuRef,
    inputs: &[usize],
    player: &[usize],
    wanted: &[Option<ItemStack>],
    sets: u32,
) -> Vec<MenuClick> {
    if inputs.len() != wanted.len() {
        return Vec::new();
    }
    for (&slot, wanted) in inputs.iter().zip(wanted) {
        if let Some(current) = menu.slot(host, slot).flatten()
            && !wanted.as_ref().is_some_and(|wanted| matching(wanted, &current))
        {
            return vec![MenuClick::QuickMove { slot }];
        }
    }
    let wanted_counts = wanted;
    for (&dest, wanted) in inputs.iter().zip(wanted_counts) {
        let Some(wanted) = wanted else { continue };
        let count = menu.slot(host, dest).flatten().map_or(0, |stack| stack.count());
        let per_set: u32 = wanted_counts.iter().flatten().filter(|item| matching(item, wanted))
            .map(ItemStack::count).sum();
        let available: u32 = inputs.iter().chain(player).filter_map(|&slot| menu.slot(host, slot).flatten())
            .filter(|item| matching(item, wanted)).map(|item| item.count()).sum();
        let goal = wanted.count().saturating_mul(sets.min(available / per_set))
            .min(crate::capacity(host, wanted));
        if count >= goal {
            continue;
        }
        let Some((source, stack)) = player.iter().find_map(|&slot| {
            let stack = menu.slot(host, slot)??;
            matching(wanted, &stack).then_some((slot, stack))
        }) else { continue };
        let amount = (goal - count).min(stack.count());
        if amount == stack.count() {
            return vec![pickup(source), pickup(dest)];
        }
        let mut clicks = vec![pickup(source)];
        clicks.extend(std::iter::repeat_n(right(dest), amount.min(8) as usize));
        clicks.push(pickup(source));
        return clicks;
    }
    Vec::new()
}

impl Recipe {
    pub fn fit(mut self, cells: usize) -> Option<Self> {
        if self.inputs.len() == cells {
            return Some(self);
        }
        let (old, new) = match (self.inputs.len(), cells) {
            (4, 9) => (2, 3),
            (9, 4) => (3, 2),
            _ => return None,
        };
        let mut inputs = vec![None; cells];
        for (index, stack) in self.inputs.into_iter().enumerate() {
            let Some(stack) = stack else { continue };
            let (x, y) = (index % old, index / old);
            if x >= new || y >= new {
                return None;
            }
            inputs[y * new + x] = Some(stack);
        }
        self.inputs = inputs;
        Some(self)
    }
}

/// The result slot can consume a recipe on pickup. Find a destination with
/// room for the entire result before taking it.
pub fn take_one(host: &dyn ClientHost, menu: MenuRef, grid: &Grid, out: &ItemStack) -> Vec<MenuClick> {
    let max = crate::capacity(host, out);
    let target = grid.player.iter().copied().find(|&slot| {
        menu.slot(host, slot).flatten().is_some_and(|s| {
            matching(&s, out) && max.saturating_sub(s.count()) >= out.count()
        })
    }).or_else(|| grid.player.iter().copied().find(|&slot| menu.slot(host, slot) == Some(None)));
    target.map_or_else(Vec::new, |slot| vec![pickup(grid.output), pickup(slot)])
}

#[cfg(test)]
mod persistence_tests {
    use super::*;

    #[test]
    fn saving_one_slot_preserves_unresolved_records() {
        let missing = Tag::compound([("output", Tag::compound([("id", Tag::String("absent:item".into()))]))]);
        let mut records = Compound::new();
        records.insert("0".into(), missing.clone());
        let replacement = Tag::compound([("test", Tag::Int(42))]);
        let bytes = updated_file(records, vec![(1, replacement.clone())]).unwrap();
        let saved = decode_records(&bytes).unwrap();
        assert_eq!(saved.get(&"0".into()), Some(&missing));
        assert_eq!(saved.get(&"1".into()), Some(&replacement));
    }

    #[test]
    fn reject_updates_outside_reader_quota_and_trailing_bytes() {
        let oversized = Tag::ByteArray(vec![0; READ_QUOTA as usize]);
        assert!(updated_file(Compound::new(), vec![(0, oversized)]).is_none());
        let mut bytes = updated_file(Compound::new(), vec![]).unwrap();
        bytes.push(0);
        assert!(decode_records(&bytes).is_none());
    }
}
