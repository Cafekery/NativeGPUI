# What Finder has that the demo does not

Taken from Finder 26.5's own menu bar, read through the accessibility API and diffed against the
demo's. Items Finder greys out when nothing is selected are still features; items the demo has in
a context menu but not in the menu bar are listed as wiring, not as features.

## Wiring: the demo has the action, the menu bar does not offer it

- [ ] File ▸ Open With, and Always Open With under ⌥
- [x] File ▸ Compress
- [x] File ▸ Share…
- [ ] File ▸ Add to Dock (⇧⌃⌘T)
- [ ] File ▸ Open in New Window and Close (⌥⌘O on a window that closes behind it)
- [x] Go ▸ Library, which Finder shows only while ⌥ is held
- [x] Go ▸ Select Startup Disk
- [x] Go ▸ Open Enclosing Folder in New Window (⌃⌘↑)

## Small features

- [x] File ▸ Find by Name… (⇧⌃⌘F): a search scoped to the name, which the criteria row can carry
- [x] File ▸ Get Summary Info (⌃⌘I): one window for a whole selection
- [ ] Edit ▸ Copy as Link (⌥⌃⌘C): Finder keeps it disabled on a local file and on an iCloud Drive
  file alike, so what it copies is unconfirmed
- [ ] Edit ▸ Paste Exactly (⇧⌥⌘V): the demo's copies already keep dates, permissions, extended
  attributes and ACLs; what Finder adds is the original's owner (unconfirmed), which takes admin
  rights
- [x] Edit ▸ Show Clipboard
- [x] Window ▸ Cycle Through Windows (⌘`)
- [x] Window ▸ Show Progress Window: the demo has a jobs popover, Finder has a window
- [ ] View ▸ Show Preview Options: Finder on the test Mac kept it disabled with the preview pane showing and a text file or an image selected over accessibility, so what its panel offers is unconfirmed
- [ ] View ▸ Customize Toolbar…, the one menu item the demo greys out
- [x] Tag swatches read as seven unlabelled groups to VoiceOver; Finder's are radio buttons named “Add “Red” label” and “Remove “Red” label”

## Features

- [x] Go ▸ Recents (⇧⌘F)
- [x] Go ▸ Connect to Server… (⌘K)
- [x] Go ▸ AirDrop (⇧⌘R), and the toolbar's AirDrop button, send the selection through AppKit's AirDrop sheet; Finder's own AirDrop window, where nearby people are listed before anything is chosen, has no public API
- [ ] Go ▸ Shared (⇧⌘S): Finder's view of iCloud items shared with you has no public API to show it
- [x] File ▸ New Smart Folder, and saved searches with it: Save in a search's scope bar writes a .savedSearch in Finder's own format to ~/Library/Saved Searches and adds it to the sidebar, and opening one, the demo's or Finder's, runs its query through Spotlight. The Save sheet asks only for the name, without Finder's tags, Where and Add To Sidebar controls
- [x] File ▸ Print (⌘P)
- [x] File ▸ Slideshow (⌥⌘Y)
- [ ] File ▸ Create/Copy iCloud Link, Manage Shared File…
- [ ] Quick Actions in the context menu: Rotate, Markup, Create PDF, and custom ones
- [x] The Services submenu

## ⌥ in context menus

Finder swaps some context-menu rows while ⌥ is held. These were read off Finder 26's own menus through the accessibility API, which lists each alternate with its modifier mask. The demo's context menus have no alternates at all; the crate's context-menu entries would need an alternate flag first, the way `MenuCommand::option_alternate` works in the menu bar.

- [ ] A file: Open With becomes Always Open With (the same app list, and the choice sticks for that kind of file)
- [x] The folder's background: Get Info becomes Show Inspector
- [ ] The folder's background: Paste Item becomes Paste Item Exactly, with ⌥⇧ held
- [x] A sidebar item: Open in New Tab becomes Open in New Window
- [ ] A selected file or folder, not yet read off Finder: Copy becomes Copy as Pathname, Quick Look becomes Slideshow, Move to Trash becomes Delete Immediately…, Get Info becomes Show Inspector. Finder on the test Mac refused to open an item's menu over accessibility after the first read, so these rest on memory of Finder, not on this probe

## Left alone

- Burn folders and Burn Disc: the hardware is gone.
- View ▸ Use Stacks: the desktop's, and the demo has no desktop.
- Eject One and Eject All: the demo ejects the selection, which is the same act.
- View ▸ Clean Up Selection: the demo's icon view is a grid in sort order with nothing placed by
  hand, so there is no stray icon to snap back.

## Sidebar surface

Read off Finder 27.0 in the same frame as the demo (1100x680 pt at 240,120, sidebar 216 pt, a 7-item
folder), Display P3 at 8 bits, mean R G B of strips with no glyphs in either app.

- List view, dark, behind the traffic lights (x 3..6.5 pt, y 8..44): Finder 41.0 41.6 41.0, the demo
  41.0 41.0 41.0. Over saturated wallpaper (window at 280,280): Finder 39.6 41.0 41.0, the demo 40.0
  41.0 41.0. Finder's carries a little more of the desktop picture's tint.
- List view, dark, under the column-header band (x 6, y 50..66): Finder 40.0 41.0 40.0, the demo 41.0
  41.0 41.0. Ending the demo's band at the toolbar's bottom instead measured 39.0 there, further off.
- Column view: Finder scrolls the earlier columns of the path under the sidebar, so their selected rows
  show through the glass at its right edge (x 207..210.5, y 56..76: 44.3 44.7 44.7 key, 46.2 non-key).
  The demo's column view starts at the current folder: 39.0 40.5 40.3.
- The "Favorites" header's accessibility frame is 198x21 at y=51 in Finder and 198x16 at y=55 in the
  demo. The glyphs land on the same pixels (x 16.5..65.5, y 58..67.5).
- Dark, the toolbar band over the list (x=600 pt, y 2..50): Finder 39 40 39, the demo 40 40 40.
- Light mode shows no lift in either app, in any view: behind the traffic lights, across the header band
  and down the rows both read 236.8..237.0 key and 242.0 non-key.
