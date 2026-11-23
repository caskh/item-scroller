use mod_api::{ClientHost, MenuRef, Tag};

use crate::{capacity, matching, pickup, rules::Rules};

/// Re-read the inventory before each transaction so the server's corrections
/// apply to the next step.
pub fn step(host: &mut dyn ClientHost, menu: MenuRef, slots: &[usize], rules: &Rules) -> bool {
    let Some(stacks) = slots.iter().map(|&i| menu.slot(host, i)).collect::<Option<Vec<_>>>() else {
        return false;
    };
    for (dest, stack) in stacks.iter().enumerate() {
        let Some(stack) = stack else { continue };
        let free = capacity(host, stack).saturating_sub(stack.count());
        if free == 0 {
            continue;
        }
        if let Some((offset, source)) = stacks[dest + 1..].iter().enumerate().find_map(|(i, item)| {
            item.as_ref().filter(|item| matching(item, stack)).map(|item| (i, item))
        }) {
            let from = slots[dest + 1 + offset];
            let mut clicks = vec![pickup(from), pickup(slots[dest])];
            if source.count() > free {
                clicks.push(pickup(from));
            }
            return menu.click(host, &clicks);
        }
    }

    let method = host.setting_choice("sort_method").unwrap_or(0);
    let rank: std::collections::HashMap<_, _> = if method == 2 {
        host.item_keys().into_iter().enumerate().map(|(rank, key)| (key, rank)).collect()
    } else {
        Default::default()
    };
    let mut order: Vec<_> = stacks.iter().enumerate().filter_map(|(index, stack)| {
        let stack = stack.as_ref()?;
        let id = stack.item().id(host);
        let components = stack.to_nbt(host).and_then(|mut data| {
            data.remove(&"components".into())?.to_network_bytes().ok()
        });
        let needs_name = method == 3 || !rules.first.is_empty() || !rules.last.is_empty();
        let name = needs_name.then(|| host.hover_name(stack));
        let priority = [&rules.first, &rules.last].map(|list| {
            name.as_ref().and_then(|name| list.iter().position(|value| value == name))
                .or_else(|| list.iter().position(|value| value == &id))
        });
        let priority = match priority {
            [Some(index), _] => (0, index),
            [None, Some(index)] => (2, index),
            [None, None] => (1, 0),
        };
        let rarity = (method == 4).then(|| {
            let patch = stack.components().map(|p| p.as_nbt());
            let value = patch.and_then(|p| p.get(&"minecraft:rarity".into()).cloned())
                .or_else(|| host.item_default_component(stack.item(), "minecraft:rarity"));
            match value {
                Some(Tag::String(v)) => match v.to_string_lossy().as_str() {
                    "epic" => 3,
                    "rare" => 2,
                    "uncommon" => 1,
                    _ => 0,
                },
                _ => 0,
            }
        });
        Some((index, stack, id, components, name, rarity, priority))
    }).collect();
    if order.len() != stacks.iter().flatten().count() {
        return false;
    }
    order.sort_by(|a, b| {
        let primary = match method {
            1 => b.1.count().cmp(&a.1.count()),
            2 => rank.get(&a.1.item()).cmp(&rank.get(&b.1.item())),
            3 => a.4.cmp(&b.4),
            4 => b.5.cmp(&a.5),
            _ => a.2.cmp(&b.2),
        };
        a.6.cmp(&b.6).then(primary).then_with(|| a.2.cmp(&b.2))
            .then_with(|| a.3.cmp(&b.3))
            .then_with(|| b.1.count().cmp(&a.1.count()))
            .then_with(|| a.0.cmp(&b.0))
    });
    let Some((dest, &(source, ..))) = order.iter().enumerate().find(|(i, row)| *i != row.0) else {
        return false;
    };
    let mut clicks = vec![pickup(slots[source]), pickup(slots[dest])];
    if stacks[dest].is_some() {
        clicks.push(pickup(slots[source]));
    }
    menu.click(host, &clicks)
}
