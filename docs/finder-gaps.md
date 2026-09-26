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

- [x] Edit ▸ Search by Name… (⇧⌃⌘F): a search scoped to the name, which the criteria row can carry
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

## Toolbar: where the search field shrinks and collapses

Read over accessibility from Finder 27.0 on the test Mac and from the demo, same folder, list view,
preview pane at 241, window width stepped 1pt around every transition, at sidebar 300, 216, 158 and
hidden. Every toolbar item's x and width, the search field's width and state, the view switcher's
state and the overflow order now match Finder at every width both sampled (about 600 rows). Finder's
rule: the title keeps a quarter of the window width; the search field gives way first (329 → 164,
then its 40pt button), then the view switcher (164 → 50pt popup), then the title (to 160), the last
space and, last, items into the overflow menu. Every threshold moves 4/3pt per point of sidebar.

- [ ] The demo's sidebar opens at 192pt, a new Finder window's at 216, so at default widths the demo
  starts shrinking the field at 1333 (Finder 1365), collapses it at 1113 (Finder 1145) and collapses
  the view switcher at 947 (Finder 979). Finder's sidebar stops at 158; the demo's at 120.
- [ ] The demo's preview pane is 304–366pt wide, Finder's 241, so the demo's window stops shrinking
  63–125pt earlier and never reaches Finder's narrowest toolbar states (title at 160, overflow).
- [ ] After one resize that re-expands a collapsed search field (773 → 1400 at sidebar 216), Finder
  keeps the item 4pt wider (333, title 372) until the next resize; the demo settles at 329 and 376.
- [ ] Growing 1pt at a time from Finder's minimum width, the demo read a 44pt search button and a
  title 2–4pt short at 781, 783 and 785 (Finder: 40pt, title 160–164), on a 200ms settle.
- [ ] The view switcher's segments read “as Icons”, “as List”, “as Columns”, “as Gallery” to
  VoiceOver; Finder's read “icon view”, “list view”, “column view”, “gallery view”, and its collapsed
  popup “list view”.
## Icons and thumbnails

Read off Finder 27 on the test Mac beside the demo, same window, folder, view and icon size.

- [ ] A folder the user cannot open (mode 000, 111, 444, or an ACL that denies list and search) wears Finder's red no-entry badge; the demo draws a plain folder. `iconForFile:` on the path returns the badged icon, but knowing which folders need it takes an access check per folder, which on the home listing reaches Desktop, Documents and Downloads before any grant.
- [ ] Like Finder, the demo shows a folder's first frame with its icons, but it waits at most 60 ms for them: an icon IconServices has never rendered (an app just installed, a size never drawn) lands a frame or more after the listing. Warm, /Applications in list and icon view had 0 ms of fill after the listing in 5 of 5 runs.
- [ ] A folder with a custom icon that scrolls into view shows the plain folder for a frame or two before its own icon (Adobe's folders in /Applications: 1 frame in a 3 s scroll through); Finder has it at once. The demo learns of a custom icon when the row is first drawn, or for a listing's first 64 rows, while it reads the listing.
- [ ] Whether a 32 pt list row previews documents was not read off Finder: its AppleScript view options ignored the size change. The demo previews pictures and movies only in list rows, as Finder does at 16 pt, and documents in icon, column and gallery views.

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
## Launch

Measured on macOS 27.0 (26A428) against a WebDAV share mounted from this Mac whose server was
stopped after the mount had settled.

- A pin of the demo's own (not a Finder favourite) inside a share that stopped answering keeps
  the whole sidebar empty until the share times out: the sidebar is built off the main thread, so
  the window and the listing no longer wait, but its pinned-folder check stats the pin. A
  `stat` into the stopped share took 84,907 ms before the server was resumed.
- A symbolic link in the opened folder that points into a stopped share holds that folder's
  listing: each link is followed to learn whether it opens as a folder. The same follow into the
  stopped share took 9,959 ms, ending only when the server was resumed.
- A share that stops answering while macOS is still reading it right after it mounts stalls
  `coreservicesd`, and with it every process's first file-property call: `NSApplication.shared`
  waited 15,440 ms inside `connectToCoreServicesD` before any of the demo's code ran.
- A Favorites or Locations item names the path its bookmark stored, as Finder's list last wrote
  it; an item whose folder has since moved or gone is not followed or dropped at launch.
## macOS 27 audit

Finder 27.0 (26A428) against the demo on one 2x display, dark and light, both windows at 240,120
1100x680 in list view on the same folder, sidebar 216 and preview pane 241 wide, nothing selected.
Traffic lights, corner radius, window shadow and title type measure identical.

- [ ] Rows: Finder is 21pt at 16pt icons and 14pt text, as the demo now is; the pitch at 32pt icons is
  unmeasured, the demo's is 37.
- [ ] Header rule, dark: Finder 1pt #414241 with 0.5pt #303030 beneath; the demo 1pt #3d3d3d.
- [ ] Kind column: Finder 27 reads "Plain Text", "RTF Document", "PDF Document", "ZIP archive", "JSON",
  "Markdown", "HTML text"; the demo "Plain Text Document", "Rich Text Document", "PDF document", "ZIP
  archive", "JSON Document", "Markdown Document", "HTML document". `LSCopyKindStringForTypeInfo` and
  `NSURLLocalizedTypeDescriptionKey` give Finder's "JSON", "Markdown" and "HTML text", `kMDItemKind`
  its "ZIP archive"; no public API gives "Plain Text", "RTF Document" or "PDF Document".
- [ ] Column view with nothing selected: Finder shows the ancestor columns with the current folder
  selected in its parent's; the demo shows only the current folder's column.
- [ ] Menu enabling: with nothing selected Finder disables New Folder with Selection, Rename, Compress,
  Duplicate, Make Alias, Share…, Show Original, Move to Trash, Eject, Print, Cut, Copy, Deselect
  All, Back and Forward without history, Clean Up in list view and Show Progress Window with no
  operation running. The demo now disables its own selection rows the same way; Back and Forward,
  Clean Up and Show Progress Window stay enabled.
- [ ] Menu titles name their target: Quick Look “Fixture”, Slideshow “Fixture”, Copy “Fixture” as
  Pathname, New Folder with “Notes.txt”, Copy “Notes.txt”, Paste Item. The demo's titles are fixed.
- [ ] File menu order, Finder: New Finder Window, New Folder, New Folder with Selection, New Smart
  Folder, New Tab, Open, Open With, Close Window (⌥ Close All), Get Info, Rename, Compress, Duplicate,
  Make Alias, Quick Look, Share…, Manage Shared Folder…, Show Original, Add to Sidebar (⌥ Add to
  Dock), Move to Trash, Eject, the tag swatches, Print. The demo adds New Text File, puts New Tab,
  Close and Close All last, and has no Open With, Add to Dock, Manage Shared Folder… or tag row. ⌥⌘O is Open and Close Window in Finder, Open in New Window in the demo.
- [ ] Edit menu: Finder puts Show Clipboard, Search (⌘F) and Search by Name… (⇧⌃⌘F) above Writing
  Tools, AutoFill, Start Dictation and Emoji & Symbols, and has no Paste and Match Style or Delete;
  the demo keeps AppKit's Paste and Match Style and Delete.
- [ ] View menu, Finder: Use Groups before Sort By; Sort By adds Date Last Opened (⌃⌥⌘3), Date Added
  (⌃⌥⌘4), Date Created and Tags (⌃⌥⌘7); Clean Up Selection; after Hide Sidebar: Hide Preview, Hide
  Toolbar, Hide Path Bar, Show Status Bar, Customize Toolbar…, Show View Options, Show Preview
  Options. The demo's Show Preview and Show Path Bar do not flip their titles and it lists Show
  Hidden Files (⇧⌘.), which Finder's menu does not.
- [ ] App menu: Settings… (⌘,), and the ⌥ rows Empty Trash and Quit and Keep Windows (⌥⌘Q).
- [ ] Go menu: Shared (⇧⌘S), Library under ⌥, the user's own locations, and Recent Folders above Go
  to Folder…; the demo has Recent Folders last. Help: Finder lists Mac User Guide and Tips for Your
  Mac, the demo "Found'er Help" (⌘?).
- [ ] Toolbar: Finder's back and forward segments are 36 and 37 wide in a 75pt group, the demo's 38 and
  38 in 76, which moves the title 1pt right; AirDrop is 44 wide, the demo's 40. With no history and
  nothing selected Finder disables back, forward, AirDrop, Share and Edit Tags; the demo leaves them
  enabled. The demo's Progress item has no Finder counterpart and narrows the title from 365 to 324.
- [ ] Get Info, Go to Folder, Connect to Server, New Folder, rename,
  conflicts, the tag editor, the tab bar and the progress window were captured from Finder 27 but
  not yet compared with the demo's.
