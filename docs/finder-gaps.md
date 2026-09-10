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
- [ ] File ▸ New Smart Folder, and saved searches with it
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
