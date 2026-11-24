<img src="icon.png" alt="" width="80" height="80">

# Item Scroller for Horizon

Move items between inventories with the mouse wheel, drag across slots to transfer stacks, or press **R** to sort a chest. Save up to 18 crafting recipes and refill a crafting grid with ingredients from your inventory.

This is caskh’s client-side Rust port of Item Scroller for Horizon. The original Minecraft Java mod was created by masa. This port follows Sakura-Ryoko’s Java 26.3 branch. [Java version used for the port](https://github.com/sakura-ryoko/itemscroller/tree/26.3) · [masa’s original repository](https://github.com/maruohon/itemscroller)

## Installation

1. Download the [Horizon mod package](https://github.com/caskh/item-scroller/raw/refs/heads/main/item_scroller.hmod).
2. Place `item_scroller.hmod` in your active Horizon profile’s `horizon-mods` folder and enable Item Scroller.
3. In **Mods → Item Scroller → Config**, turn on **Enable gameplay shortcuts** to use inventory, crafting, and trading shortcuts. They start disabled.

Some multiplayer servers may disallow these shortcuts or flag them with anticheat. Check the server rules before enabling them.

**Compatibility** · Horizon mod API 0.2 · Client-side WebAssembly · English, Russian, Spanish, Korean, Dutch, Ukrainian, Simplified Chinese, and Traditional Chinese

## Inventory

| Gesture | Action |
| --- | --- |
| Wheel over a slot | Push one item on scroll up, or pull one on scroll down |
| Shift + wheel | Push or pull a stack |
| Ctrl + wheel | Push or pull matching stacks |
| Shift + Alt + wheel or left click | Move all items across inventories |
| Alt + left click | Move all matching stacks from the hovered inventory |
| Shift + left or right drag | Move entire stacks with left drag, or leave one item in each source slot with right drag |
| Ctrl + left or right drag | Move one item from each visited slot |
| Ctrl + Shift + left drag | Move all matching stacks from each visited slot |
| W or S + a move drag | Move toward the inventory above or below the hovered slot |
| Shift + left click with a stack on the cursor | Place it in an empty slot and move matching stacks there from the opposite inventory, or drop matching stacks by clicking outside the window |
| Ctrl or Shift + Drop key + drag | Drop one item with Ctrl, entire stacks with Shift + left drag, or all but one with Shift + right drag |
| Ctrl + Shift + Drop key | Drop all stacks of the hovered item type on that side |
| X + Drop key | Drop all ordinary stacks on the hovered side |
| R over an inventory | Merge matching stacks and sort the hovered inventory |
| Ctrl + S | Toggle Item Scroller for the session |
| Ctrl + Alt + Shift + I | Log the menu ID and hovered slot role |

For drop drags, hold the Drop key while the pointer is off the slots, then hold the drag modifier and start dragging. The vanilla Drop key can be rebound. The normal player inventory transfers between the main inventory and hotbar. Sorting treats those two regions separately.

In single-player, press **Esc** to pause, then press **Ctrl + Alt + I** to open Item Scroller’s key binds. Closing the editor returns to the pause menu. On multiplayer servers, press **Ctrl + Alt + I** during play. Click the current binding and press a key combination, or click **Options** to choose a key and modifiers, reset the binding, or clear it. Backspace clears a binding during capture. **Open Item Scroller config** is the only Item Scroller entry in Horizon’s **Controls**. Action shortcuts start with the mod defaults in the new editor.

**Matching items (scroll or drag)** applies to wheel transfers and drag gestures. **Matching items (left-click)** applies to left-click and pushes matching stacks from the hovered inventory. In **Mods → Item Scroller → Config**, you can change scroll directions, enable position-aware scrolling, select a sorting method, and toggle individual features. The Ctrl + Shift + wheel option switches to the upstream move-all gesture. By default, that wheel gesture still moves matching items.

## Crafting and trading

- Middle-click a crafting result to save its exact ingredient layout in the selected memory slot. Hold **S** to show the 18 slots. Scroll or press **1–9** to select a slot, or click a slot to select it and refill the grid. Shift-click a saved slot to fill multiple sets. Middle-click a memory slot to clear the grid.
- Scroll up over the crafting result to take one result, hold **Shift** to quick-move results, or hold the move-all modifier to refill and craft repeatedly. Scroll down to refill from the selected recipe. Shift + scroll up over an empty result clears the grid. Right-click a result to craft up to a stack into the inventory.
- **Shift + C** crafts the selected recipe repeatedly. **Ctrl + C** crafts and drops while held, and **Ctrl + Shift + C** toggles that behavior. **Shift + M** moves matching results to open storage. **Shift + Alt + Q** drops matching results from the player inventory.
- Select an offer and right-click its result to repeat that trade with the available payment items. Middle-click a payable result to add or remove a profile-wide favorite. A star above the result marks a favorite. Favorites match the payment item types and exact result, including components, across villagers.
- Shift + scroll up over a villager trade result takes the result or clears the payment slots if the result is empty. Shift + scroll down can refill the most recently observed valid trade on that screen.

Recipe memory is saved per Horizon profile by default. Horizon supplies menu layouts and handles each transaction through normal game clicks.

## Menu and sorting rules

The optional profile file `config/item_scroller/rules.txt` takes one rule per line. Rules are read when the inventory screen opens. Use the debug hotkey to find the menu ID and menu-order slot index. Use `inventory` for the personal inventory.

```text
menu minecraft:brewing_stand
slot minecraft:generic_9x3 0
grid minecraft:crafting 0 1-9
first minecraft:diamond
last Custom named stack
```

`menu` disables Item Scroller in that menu. `slot` excludes an individual slot from direct mod clicks. `grid` identifies the result slot and a contiguous 2×2 or 3×3 input grid for a menu whose slots Horizon exposes. `first` and `last` set sorting priorities by item ID or display name.

## Differences from the Java version

Individual villager favorites, offer-list reordering, trading all favorites in one action, trade-list scroll restoration, and creative inventory interactions require client APIs the pinned Horizon SDK does not expose. Java-specific integrations, per-world recipe files, recipe-book-protocol mass crafting, and sorting by vanilla creative category are also outside this release.

## Build

Run `cargo horizon-mod build --wasm` with the pinned Horizon SDK and Rust 1.99.1. The SDK is cached in `.horizon/`. The build updates `mod.wasm` and the manifest digest and writes the package to `target/horizon-mod/item_scroller.hmod`.

## Credits and license

- **masa** created Item Scroller for Minecraft Java Edition. [Original project](https://github.com/maruohon/itemscroller)
- **Sakura-Ryoko** maintains the Java fork used as the reference for this port. [Java 26.3 source](https://github.com/sakura-ryoko/itemscroller/tree/26.3)
- **caskh** maintains the Horizon port. For Horizon bugs, include your Horizon version, the container you opened, and the gesture you used. [Horizon issues](https://github.com/caskh/item-scroller/issues)

Distributed under the GNU Lesser General Public License v3. The Horizon package includes the Rust source and both license texts. [LGPL v3](LICENSE.txt) · [GPL v3](COPYING.txt)
