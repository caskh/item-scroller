use std::{cmp::Ordering, collections::HashMap};

use mod_api::{ClientHost, ItemStack, MenuClick, MenuRef, MenuSlotKind, SwapWith, Tag};

use crate::{capacity, creative_categories::ITEMS, matching, pickup, rules::Rules};

pub const METHODS: [&str; 9] = [
    "id", "count", "registry", "name", "rarity",
    "category_name", "category_count", "category_registry", "category_rarity",
];
const CATEGORIES: [&str; 12] = [
    "building_blocks", "colored_blocks", "natural_blocks", "functional_blocks",
    "redstone_blocks", "tools_and_utilities", "combat", "food_and_drinks",
    "ingredients", "spawn_eggs", "op_blocks", "other",
];

fn category(id: &str) -> &'static str {
    let Some(path) = id.strip_prefix("minecraft:") else { return "other" };
    ITEMS.binary_search_by_key(&path, |&(item, _)| item)
        .map_or("other", |index| CATEGORIES[ITEMS[index].1])
}

fn category_rank(id: &str, order: &[String]) -> (bool, usize) {
    let category = category(id);
    if let Some(index) = order.iter().position(|entry| entry == category) {
        return (false, index);
    }
    (true, CATEGORIES.iter().position(|&name| name == category).unwrap_or(CATEGORIES.len()))
}

fn shulker_box(id: &str) -> bool {
    let Some(path) = id.strip_prefix("minecraft:") else { return false };
    path == "shulker_box" || path.ends_with("_shulker_box")
}

fn component(host: &dyn ClientHost, stack: &ItemStack, id: &str) -> Option<Tag> {
    if let Some(patch) = stack.components() {
        let patch = patch.as_nbt();
        if patch.contains_key(&format!("!{id}").as_str().into()) { return None; }
        if let Some(value) = patch.get(&id.into()) { return Some(value.clone()); }
    }
    host.item_default_component(stack.item(), id)
}

fn contents(host: &dyn ClientHost, stack: &ItemStack, id: &str) -> Option<Vec<Tag>> {
    match component(host, stack, id) {
        Some(Tag::List(items)) => Some(items),
        _ => None,
    }
}

fn bundle_items(host: &dyn ClientHost, stack: &ItemStack) -> Option<Vec<Tag>> {
    contents(host, stack, "minecraft:bundle_contents")
}

fn bundle_weight(host: &dyn ClientHost, items: &[Tag]) -> f64 {
    items.iter().filter_map(|entry| {
        let Tag::Compound(nbt) = entry else { return None };
        let stack = ItemStack::from_nbt(host, nbt)?;
        let weight = if let Some(nested) = bundle_items(host, &stack) {
            1.0 / 16.0 + bundle_weight(host, &nested)
        } else if contents(host, &stack, "minecraft:bees").is_some_and(|bees| !bees.is_empty()) {
            1.0
        } else {
            1.0 / f64::from(capacity(host, &stack).max(1))
        };
        Some(weight * f64::from(stack.count()))
    }).sum()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Container {
    None,
    Bundle,
    Shulker,
}

struct SortKey {
    container: Container,
    group: u8,
    fullness: f64,
}

fn container_key(host: &dyn ClientHost, id: &str, stack: &ItemStack) -> SortKey {
    if shulker_box(id) {
        let occupied = contents(host, stack, "minecraft:container").map_or(0, |items| items.len());
        let fullness = occupied as f64;
        return SortKey {
            container: Container::Shulker,
            group: if host.setting_bool("sort_boxes_last").unwrap_or(true) { 2 } else { 0 },
            fullness: if host.setting_bool("sort_boxes_reverse") == Some(true) { -fullness } else { fullness },
        };
    }
    if let Some(items) = bundle_items(host, stack) {
        let fullness = bundle_weight(host, &items);
        return SortKey {
            container: Container::Bundle,
            group: u8::from(host.setting_bool("sort_bundles_last").unwrap_or(true)),
            fullness: if host.setting_bool("sort_bundles_reverse") == Some(true) { -fullness } else { fullness },
        };
    }
    SortKey { container: Container::None, group: 0, fullness: 0.0 }
}

struct Row<'a> {
    index: usize,
    stack: &'a ItemStack,
    id: String,
    components: Option<Vec<u8>>,
    name: Option<String>,
    rarity: u8,
    priority: (u8, usize),
    category: (bool, usize),
    container: SortKey,
}

fn swap_bundle(
    host: &mut dyn ClientHost, menu: MenuRef, id: &str, rules: &Rules, source: usize, dest: usize,
) -> bool {
    let Some(kinds) = menu.slot_kinds(host) else { return false };
    let hotbar = kinds.into_iter().enumerate().filter_map(|(slot, kind)| {
        let MenuSlotKind::Player(index @ 0..=8) = kind else { return None };
        (!rules.blocked(id).is_some_and(|slots| slots.contains(&slot))).then_some((slot, index as u8))
    }).collect::<Vec<_>>();
    let swap = |slot, index| MenuClick::Swap { slot, with: SwapWith::Hotbar(index) };
    for &(slot, index) in &hotbar {
        if slot == source { return menu.click(host, &[swap(dest, index)]); }
        if slot == dest { return menu.click(host, &[swap(source, index)]); }
    }
    // Pickup on an occupied bundle or with a bundle on the cursor can insert
    // items into it. Swap via a hotbar slot and restore that slot in the batch.
    let scratch = hotbar.into_iter().filter_map(|(slot, index)| {
        let item = menu.slot(host, slot)?;
        if item.as_ref().is_some_and(|stack| {
            stack.count() > capacity(host, stack)
                || (id == "minecraft:shulker_box" && shulker_box(&stack.item().id(host)))
        }) { return None; }
        Some((item.is_some(), index))
    }).min();
    let Some((_, index)) = scratch else { return false };
    menu.click(host, &[swap(source, index), swap(dest, index), swap(source, index)])
}

pub struct Sort {
    pub slots: Vec<usize>,
    missed_box_merges: Vec<ItemStack>,
    box_probe: Option<ItemStack>,
}

impl Sort {
    pub fn new(slots: Vec<usize>) -> Self {
        Self { slots, missed_box_merges: Vec::new(), box_probe: None }
    }

    pub fn changed(&mut self) {
        self.box_probe = None;
    }

    pub fn retry_after_timeout(&mut self) -> bool {
        let Some(stack) = self.box_probe.take() else { return false };
        self.missed_box_merges.push(stack);
        true
    }

    /// Re-read the inventory before each transaction so server corrections
    /// apply to the next step.
    pub fn step(&mut self, host: &mut dyn ClientHost, menu: MenuRef, id: &str, rules: &Rules) -> bool {
        let slots = &self.slots;
        let Some(stacks) = slots.iter().map(|&i| menu.slot(host, i)).collect::<Option<Vec<_>>>() else {
            return false;
        };
        let assume_boxes_stack = host.setting_bool("sort_assume_empty_boxes") == Some(true);
        for (dest, stack) in stacks.iter().enumerate() {
            let Some(stack) = stack else { continue };
            if bundle_items(host, stack).is_some() { continue; }

            let free = capacity(host, stack).saturating_sub(stack.count());
            let probe = free == 0 && assume_boxes_stack && stack.count() < 64
                && shulker_box(&stack.item().id(host))
                && component(host, stack, "minecraft:container")
                    .is_none_or(|tag| matches!(tag, Tag::List(items) if items.is_empty()))
                && !self.missed_box_merges.iter().any(|failed| matching(failed, stack));
            if free == 0 && !probe { continue; }

            if let Some((offset, source)) = stacks[dest + 1..].iter().enumerate().find_map(|(i, item)| {
                item.as_ref().filter(|item| matching(item, stack) && (!probe || item.count() == 1))
                    .map(|item| (i, item))
            }) {
                let from = slots[dest + 1 + offset];
                let mut clicks = vec![pickup(from), pickup(slots[dest])];
                if probe || source.count() > free {
                    // Return any remainder to its original slot if the server refuses the merge.
                    clicks.push(pickup(from));
                }
                if probe { self.box_probe = Some(stack.clone()); }
                return menu.click(host, &clicks);
            }
        }

        let method = host.setting_choice("sort_method").unwrap_or(0);
        let by_category = (5..=8).contains(&method);
        let secondary = match method { 5 => 3, 6 => 1, 7 => 2, 8 => 4, other => other };
        let rank: HashMap<_, _> = if secondary == 2 {
            host.item_keys().into_iter().enumerate().map(|(rank, key)| (key, rank)).collect()
        } else {
            HashMap::new()
        };
        let needs_name = secondary == 3 || !rules.first.is_empty() || !rules.last.is_empty();
        let mut order: Vec<_> = stacks.iter().enumerate().filter_map(|(index, stack)| {
            let stack = stack.as_ref()?;
            let id = stack.item().id(host);
            let components = stack.to_nbt(host).and_then(|mut data| {
                data.remove(&"components".into())?.to_network_bytes().ok()
            });
            let name = needs_name.then(|| host.hover_name(stack));
            let [first, last] = [&rules.first, &rules.last].map(|list| {
                name.as_ref().and_then(|name| list.iter().position(|value| value == name))
                    .or_else(|| list.iter().position(|value| value == &id))
            });
            let priority = match (first, last) {
                (Some(index), _) => (0, index),
                (None, Some(index)) => (2, index),
                (None, None) => (1, 0),
            };
            let rarity = if secondary == 4 {
                match component(host, stack, "minecraft:rarity") {
                    Some(Tag::String(value)) => match value.to_string_lossy().as_str() {
                        "epic" => 3, "rare" => 2, "uncommon" => 1, _ => 0,
                    },
                    _ => 0,
                }
            } else { 0 };
            let category = if by_category { category_rank(&id, &rules.category_order) } else { (false, 0) };
            let container = container_key(host, &id, stack);
            Some(Row { index, stack, id, components, name, rarity, priority, category, container })
        }).collect();
        order.sort_by(|a, b| {
            let primary = || match secondary {
                1 => b.stack.count().cmp(&a.stack.count()),
                2 => rank.get(&a.stack.item()).cmp(&rank.get(&b.stack.item())),
                3 => a.name.cmp(&b.name),
                4 => b.rarity.cmp(&a.rarity),
                _ => a.id.cmp(&b.id),
            };
            a.priority.cmp(&b.priority)
                .then_with(|| a.container.group.cmp(&b.container.group))
                .then_with(|| if a.container.group != 0 {
                    a.container.fullness.total_cmp(&b.container.fullness)
                } else { Ordering::Equal })
                .then_with(|| a.category.cmp(&b.category))
                .then_with(primary)
                .then_with(|| a.id.cmp(&b.id))
                .then_with(|| a.container.fullness.total_cmp(&b.container.fullness))
                .then_with(|| a.components.cmp(&b.components))
                .then_with(|| b.stack.count().cmp(&a.stack.count()))
                .then_with(|| a.index.cmp(&b.index))
        });
        let Some((dest, row)) = order.iter().enumerate().find(|(i, row)| *i != row.index) else {
            return false;
        };
        let source = slots[row.index];
        let target = slots[dest];
        let mut clicks = vec![pickup(source), pickup(target)];
        if let Some(target_stack) = &stacks[dest] {
            if row.container.container == Container::Bundle || bundle_items(host, target_stack).is_some() {
                return swap_bundle(host, menu, id, rules, source, target);
            }
            clicks.push(pickup(source));
        }
        menu.click(host, &clicks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exported_categories_use_first_membership_and_custom_order() {
        assert_eq!(category("minecraft:oak_log"), "building_blocks");
        assert_eq!(category("minecraft:apple"), "food_and_drinks");
        assert_eq!(category("example:apple"), "other");
        assert!(category_rank("minecraft:oak_log", &[]) < category_rank("minecraft:apple", &[]));
        let order = ["food_and_drinks".into()];
        assert!(category_rank("minecraft:apple", &order) < category_rank("minecraft:oak_log", &order));
    }
}
