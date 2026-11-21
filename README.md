# Item Scroller for Horizon

Hover a storage or player inventory slot in an open container. Scroll up to push items to the other inventory or down to pull them toward the hovered side. A plain scroll moves one item. Shift moves a stack. Ctrl or Ctrl+Shift transfers matching stacks. Hold Shift and the left mouse button and sweep across slots to push each stack, or hold Ctrl+Shift and the left mouse button to push matching stacks from each newly hovered slot. Shift+Alt+wheel transfers all item types. Rebind the three actions in Controls. The mod config includes hover transfer and reverse scrolling toggles. The cursor must be empty. Wheel events are queued and processed one per client tick. The mod rechecks the menu, hovered stack and cursor before submitting clicks.

This implementation targets ordinary storage transfers. Equipment, crafting outputs, trading, creative tabs and recipe automation are outside its scope. The mod submits ordinary client menu clicks. The server decides whether each click succeeds.

Install the ready-to-use [item_scroller.hmod](item_scroller.hmod) in the active Horizon profile's `horizon-mods` folder and enable it in Options → Mods. No local build is needed to play. Developers can rebuild it with the pinned Horizon SDK and `cargo horizon-mod build --wasm`; the SDK is downloaded into `.horizon/` and is not stored in this repository.

Hold Shift+Alt and left-click a chest slot to take all items, or a player inventory slot to deposit all items. Rebind both directions with Move all items in Controls.

In the survival inventory, transfers run between the main inventory and the hotbar. Ctrl+Shift plus your current Drop Item key throws every stack of the hovered item type from that inventory. Hold the Drop all modifier and press the current Drop Item key to throw all ordinary stacks on the hovered side and can be changed under Drop all inventory in Controls. In the player inventory both drop actions include the hotbar and main inventory. Matching by item type includes named, damaged and enchanted variants.

Set the action keys in Mods → Item Scroller → Config. Use Controls / default follows the Controls binding, or Ctrl+Shift for Drop matching. Drop all defaults to X plus the vanilla Drop Item key.
