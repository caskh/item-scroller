//! Inventory transfer planning for a client-side Horizon WebAssembly mod.
//! The caller supplies ordinary storage slots in menu order and an empty cursor.
//! A stack key includes item components so different potions or enchanted items stay separate.

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    One,
    Stack,
    LeaveOne,
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

/// Pick menu slots for a batch of vanilla quick-move clicks. The saved key
/// protects queued matching transfers when the hovered stack changes.
pub fn quick_move_sources<K: Eq>(
    slots: &[Slot<K>],
    hovered: u16,
    pull: bool,
    matching: bool,
    anchor: Option<&K>,
) -> Vec<u16> {
    let Some(hover) = slots.iter().find(|slot| slot.id == hovered) else {
        return Vec::new();
    };
    let source_side = hover.player_side != pull;
    if !slots.iter().any(|slot| slot.player_side != source_side) {
        return Vec::new();
    }
    if matching && (anchor.is_none() || (hover.item.as_ref() != anchor && (pull || hover.item.is_some()))) {
        return Vec::new();
    }

    slots.iter()
        .filter(|slot| {
            slot.player_side == source_side
                && slot.count > 0
                && (!matching || slot.item.as_ref() == anchor)
        })
        .map(|slot| slot.id)
        .collect()
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

        if mode == Mode::LeaveOne && source.count < 2 {
            continue;
        }
        let mut remaining = source.count - u32::from(mode == Mode::LeaveOne);
        // Merge compatible stacks before starting empty slots.
        for empty in [false, true] {
            for dest in slots.iter().filter(|slot| slot.player_side != source_side) {
                if dest.item.is_none() != empty
                    || dest.item.as_ref().is_some_and(|existing| existing != key)
                    || (pull && matches!(mode, Mode::One | Mode::Stack) && dest.id != hovered) {
                    continue;
                }
                let space = dest.capacity.min(source.capacity).saturating_sub(dest.count);
                if space == 0 { continue; }
                if clicks.is_empty() {
                    clicks.push(Click::Left(source.id));
                    if mode == Mode::LeaveOne {
                        clicks.push(Click::Right(source.id));
                    }
                }
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
    fn leave_one_returns_any_overflow_to_the_source() {
        let slots = [slot(0, false, 5), slot(1, true, 63)];
        assert_eq!(
            transfer(&slots, 0, false, Mode::LeaveOne, true),
            vec![Click::Left(0), Click::Right(0), Click::Left(1), Click::Left(0)]
        );
        let slots = [slot(0, false, 1), slot(1, true, 0)];
        assert!(transfer(&slots, 0, false, Mode::LeaveOne, true).is_empty());
    }

    #[test]
    fn leave_one_fills_multiple_destinations_without_taking_the_last_item() {
        let slots = [slot(0, false, 5), slot(1, true, 62), slot(2, true, 0)];
        assert_eq!(
            transfer(&slots, 0, false, Mode::LeaveOne, true),
            vec![Click::Left(0), Click::Right(0), Click::Left(1), Click::Left(2)]
        );
    }

    #[test]
    fn matching_quick_move_uses_hovered_side_to_push_and_other_side_to_pull() {
        let slots = [
            slot(0, false, 5),
            slot(1, false, 2),
            Slot { item: Some(2), ..slot(2, false, 3) },
            slot(3, true, 4),
            slot(4, true, 0),
        ];
        assert_eq!(quick_move_sources(&slots, 0, false, true, Some(&1)), [0, 1]);
        assert_eq!(quick_move_sources(&slots, 0, true, true, Some(&1)), [3]);
        assert_eq!(quick_move_sources(&slots, 3, false, true, Some(&1)), [3]);
        assert_eq!(quick_move_sources(&slots, 3, true, true, Some(&1)), [0, 1]);
        assert!(quick_move_sources(&slots, 4, false, true, None).is_empty());
        assert!(quick_move_sources(&slots, 2, false, true, Some(&1)).is_empty());
    }

    #[test]
    fn move_all_uses_the_same_direction_and_allows_an_empty_hovered_slot() {
        let slots = [
            slot(0, false, 5),
            Slot { item: Some(2), ..slot(1, false, 3) },
            slot(2, false, 0),
            slot(3, true, 4),
        ];
        assert_eq!(quick_move_sources(&slots, 2, false, false, None), [0, 1]);
        assert_eq!(quick_move_sources(&slots, 2, true, false, None), [3]);
    }

    #[test]
    fn full_destination_or_occupied_cursor_leaves_inventory_alone() {
        let slots = [slot(0, false, 5), slot(1, true, 64)];
        assert!(transfer(&slots, 0, false, Mode::Stack, true).is_empty());
        assert!(transfer(&slots, 1, false, Mode::One, false).is_empty());
    }
}
