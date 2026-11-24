use mod_api::{ClientHost, ConfigFileError, ConfigPath, ItemStack, MenuClick, MenuRef, Tag};

use crate::{normalized, recipe};

#[derive(Clone, PartialEq)]
pub struct Trade {
    inputs: [Option<ItemStack>; 2],
    output: ItemStack,
}

impl Trade {
    pub fn read(host: &dyn ClientHost, menu: MenuRef) -> Option<Self> {
        let output = menu.slot(host, 2)??;
        let inputs = [menu.slot(host, 0)?.map(normalized), menu.slot(host, 1)?.map(normalized)];
        inputs.iter().any(Option::is_some).then_some(Self { inputs, output })
    }

    fn record(&self, host: &dyn ClientHost) -> Option<Tag> {
        let mut stacks = Vec::new();
        for stack in self.inputs.iter().chain([&Some(self.output.clone())]) {
            stacks.push(Tag::Compound(match stack {
                Some(stack) => stack.to_nbt(host)?,
                None => Default::default(),
            }));
        }
        Some(Tag::List(stacks))
    }

    pub fn step(&self, host: &mut dyn ClientHost, menu: MenuRef, player: &[usize]) -> bool {
        if let Some(current) = Self::read(host, menu) {
            return current == *self && menu.click(host, &[MenuClick::QuickMove { slot: 2 }]);
        }
        // A changed payment type can indicate that the player selected another offer.
        for (slot, wanted) in self.inputs.iter().enumerate() {
            match menu.slot(host, slot) {
                Some(Some(stack)) if !wanted.as_ref().is_some_and(|w| crate::matching(w, &stack)) => return false,
                None => return false,
                _ => {}
            }
        }
        let clicks = recipe::refill(host, menu, &[0, 1], player, &self.inputs, 99);
        !clicks.is_empty() && menu.click(host, &clicks)
    }
}

#[derive(Default)]
pub struct Favorites(Option<Vec<Tag>>);

impl Favorites {
    fn load(&mut self, host: &dyn ClientHost) -> Option<&mut Vec<Tag>> {
        if self.0.is_none() {
            let path = ConfigPath::new("trade_favorites.nbt").ok()?;
            self.0 = Some(match host.data_files().read(&path) {
                Ok(bytes) => match Tag::from_network_bytes(&bytes, Some(4 << 20)).ok()? {
                    (Tag::List(items), end) if end == bytes.len() => items,
                    _ => return None,
                },
                Err(ConfigFileError::NotFound) => Vec::new(),
                Err(_) => return None,
            });
        }
        self.0.as_mut()
    }

    pub fn contains(&mut self, host: &dyn ClientHost, trade: &Trade) -> bool {
        self.load(host).is_some_and(|items| !items.is_empty()
            && trade.record(host).is_some_and(|record| items.contains(&record)))
    }

    pub fn toggle(&mut self, host: &dyn ClientHost, trade: &Trade) -> Option<()> {
        let record = trade.record(host)?;
        let items = self.load(host)?;
        let mut updated = items.clone();
        if let Some(i) = updated.iter().position(|entry| entry == &record) {
            updated.remove(i);
        } else {
            updated.push(record);
        }
        let bytes = Tag::List(updated.clone()).to_network_bytes().ok()?;
        Tag::from_network_bytes(&bytes, Some(4 << 20)).ok()?;
        host.data_files().write(&ConfigPath::new("trade_favorites.nbt").ok()?, &bytes).ok()?;
        *items = updated;
        Some(())
    }
}
