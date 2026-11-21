//! Inventory transfer planning for a client-side Horizon WebAssembly mod.
//! The caller supplies ordinary storage slots in menu order and an empty cursor.
//! A stack key includes item components so different potions or enchanted items stay separate.

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    One,
    Stack,
    Matching,
    All,
}

pub struct Slot<K> {
    pub id: u16,
    pub player_side: bool,
    pub item: Option<K>,
    pub count: u32,
    pub capacity: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Click {
    Left(u16),
    Right(u16),
}

/// Plan one source stack at a time. Re-read the menu after each completed sequence.
/// The adapter filters slots with special pickup or placement rules before calling.
pub fn transfer<K: Eq>(
    slots: &[Slot<K>],
    hovered: u16,
    pull: bool,
    mode: Mode,
    cursor_empty: bool,
) -> Vec<Click> {
    let mut clicks = Vec::new();
    let Some(anchor) = slots.iter().find(|slot| slot.id == hovered) else {
        return clicks;
    };
    if !cursor_empty {
        return clicks;
    }

    let source_side = anchor.player_side != pull;
    for source in slots.iter().filter(|slot| slot.player_side == source_side && slot.count > 0) {
        let Some(key) = source.item.as_ref() else { continue };
        if mode != Mode::All {
            if !pull && mode != Mode::Matching && source.id != hovered {
                continue;
            }
            if anchor.item.as_ref().is_some_and(|wanted| wanted != key)
                || (mode == Mode::Matching && anchor.item.is_none()) {
                continue;
            }
        }

        let mut remaining = source.count;
        // Merge compatible stacks before starting empty slots.
        for empty in [false, true] {
            for dest in slots.iter().filter(|slot| slot.player_side != source_side) {
                if dest.item.is_none() != empty
                    || dest.item.as_ref().is_some_and(|existing| existing != key)
                    || (pull && matches!(mode, Mode::One | Mode::Stack) && dest.id != hovered) {
                    continue;
                }
                let space = dest.capacity.saturating_sub(dest.count);
                if space == 0 { continue; }
                if clicks.is_empty() { clicks.push(Click::Left(source.id)); }
                if mode == Mode::One {
                    clicks.push(Click::Right(dest.id));
                    if remaining > 1 { clicks.push(Click::Left(source.id)); }
                    return clicks;
                }
                clicks.push(Click::Left(dest.id));
                remaining = remaining.saturating_sub(space);
                if remaining == 0 { return clicks; }
            }
        }
        if !clicks.is_empty() {
            clicks.push(Click::Left(source.id));
            return clicks;
        }
    }
    clicks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(id: u16, player_side: bool, count: u32) -> Slot<u8> {
        Slot { id, player_side, item: (count > 0).then_some(1), count, capacity: 64 }
    }

    #[test]
    fn push_one_returns_remainder() {
        let slots = [slot(0, false, 5), slot(1, true, 0)];
        assert_eq!(transfer(&slots, 0, false, Mode::One, true),
                   vec![Click::Left(0), Click::Right(1), Click::Left(0)]);
    }

    #[test]
    fn pull_stack_stops_at_anchor_capacity() {
        let slots = [slot(0, false, 5), slot(1, true, 63)];
        assert_eq!(transfer(&slots, 1, true, Mode::Stack, true),
                   vec![Click::Left(0), Click::Left(1), Click::Left(0)]);
    }

    #[test]
    fn full_destination_or_occupied_cursor_leaves_inventory_alone() {
        let slots = [slot(0, false, 5), slot(1, true, 64)];
        assert!(transfer(&slots, 0, false, Mode::Stack, true).is_empty());
        assert!(transfer(&slots, 1, false, Mode::One, false).is_empty());
    }
}
