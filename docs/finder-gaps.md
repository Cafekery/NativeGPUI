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
- [x] The view switcher's segments read “icon view”, “list view”, “column view”, “gallery view” to
  VoiceOver, as Finder's, and its collapsed popup “list view” (its value stays “as List”).
## Icons and thumbnails

Read off Finder 27 on the test Mac beside the demo, same window, folder, view and icon size.

- [x] A folder the user cannot open (mode 000, 111, 444, or an ACL that denies list and search) wears Finder's red no-entry badge. The demo takes the user's access from the listing it already makes (`ATTR_CMN_USERACCESS` in `getattrlistbulk`, which the file system answers from its catalogue without opening the folder, so Desktop, Documents and Downloads are not touched) and draws such a folder with `iconForFile:`, which carries the badge.
- [ ] Like Finder, the demo shows a folder's first frame with its icons, but it waits at most 60 ms for them: an icon IconServices has never rendered (an app just installed, a size never drawn) lands a frame or more after the listing. Warm, /Applications in list and icon view had 0 ms of fill after the listing in 5 of 5 runs.
- [ ] A folder with a custom icon that scrolls into view shows the plain folder for a frame or two before its own icon (Adobe's folders in /Applications: 1 frame in a 3 s scroll through); Finder has it at once. The demo learns of a custom icon when the row is first drawn, or for a listing's first 64 rows, while it reads the listing.
- [ ] Whether a 32 pt list row previews documents was not read off Finder: its AppleScript view options ignored the size change. The demo previews pictures and movies only in list rows, as Finder does at 16 pt, and documents in icon, column and gallery views.
## Behaviour against Finder 27 on the test Mac

- [x] A symlink to an app (and an app itself) is a leaf in column view: no chevron, no “Contents”
  column, the preview column beside it, the window titled by the folder that holds it, the path
  bar ending at it; a double-click launches it.
- [x] Show Preview is kept as Finder keeps it: one setting for icon and list views
  (`ShowPreviewPane`), one for gallery (`GalleryViewOptions.ShowPreviewPane`) and one for the
  column view's preview column. Watched on the user's Mac: hiding it in list or icon view hides
  it in the other and leaves gallery and columns; hiding it in gallery or columns leaves the other
  three; a reopened window keeps each (the demo: a relaunch). Finder holds the column setting for
  every window and folder without rewriting `ColumnViewOptions.ShowPreview` while it runs.
- [ ] Icon view keeps the columns of the first frame that shows a folder's items, whatever the pane
  or the window does next (a narrower window scrolls sideways), as Finder's free arrangement does.
  On the user's Mac both lay the 17-item fixture out in 5 columns with the pane shown (label centres
  282–730 pt) and keep 5 through a hidden pane and windows of 900, 1300 and 1100 pt; laid out with
  the pane hidden both keep 7 and cut them at the pane. Left: Finder shows the icons beneath its
  pane's glass (88 over an icon, 38 around it; the demo's canvas stops at the pane), places a new
  folder's icons in name order where the demo uses the list's sort, and keeps each icon's place
  across visits; the demo lays a folder out afresh each time it opens.
- [x] Column view with an unreadable folder selected: as in Finder, its row has no chevron, nothing
  opens, the preview column shows the badged folder and the path bar ends at it. Finder's kind line
  reads “Folder - Zero bytes”, the demo's “Folder”.
- [x] Column view sorts by `ColumnViewOptions.ArrangeBy` (dnam: name) and gallery view by
  `GalleryViewSettings.arrangeBy`, each in the direction Finder first sorts that key, not by the
  list's sort; the demo sorts the rows before it shows the view, and keeps each view's own key.
- [x] Sort By: Name ⌃⌥⌘1, Kind ⌃⌥⌘2, Date Last Opened ⌃⌥⌘3, Date Added ⌃⌥⌘4, Date Modified ⌃⌥⌘5,
  Date Created, Size ⌃⌥⌘6, Tags ⌃⌥⌘7; Clean Up By: Name, Kind, Date Modified, Date Created, Size,
  Tags (⌥⌘). Read off Finder 27 with lists of a 7-item fixture: Date Last Opened is Spotlight's
  last-used date, which it makes up from the modification date where it has not indexed the item;
  Tags sort by the first tag's name; an item a column says nothing for (no tag, no version) lists
  last whichever way the column sorts; the Tags column shows the names ("Work, Blue"), Version a
  bundle's short version, both blank for nothing.
- [ ] Comments: Finder shows the comment kept in the folder's .DS_Store (`cmmt`) and not the
  `kMDItemFinderComment` attribute; the demo reads no .DS_Store record for it, so the column stays
  off. Shared By and Last Modified By: Finder shows neither column outside iCloud Drive, even when
  its settings ask for them; the demo offers neither.
- [ ] The context menu's Open on an unreadable folder does nothing; Finder's was not measured.
- [x] A window opens with its preview pane already at full width; only Show/Hide Preview (⇧⌘P)
  slides it, over about 230 ms each way.
- [x] Move to Trash (⌘⌫) and Delete Immediately… select the row just below the cursor (the row
  last clicked, ⌘-clicked or reached with ⇧-arrow) when that row stays, and nothing when it goes
  too or there is none. Measured in list, icon and column views: one row, a ⇧-range either way,
  ⌘-clicked rows, an expanded folder whose first child is the row below.
- [x] In the Trash, File ▸ Move to Trash reads Put Back (⌘⌫), and an item's menu offers Put Back,
  Delete Immediately… and Empty Trash. Put Back returns each item to the folder its Trash
  `.DS_Store` records, selects nothing in the Trash, and selects the items in a window showing
  that folder, opening one if none does.
- [x] Sounds, recorded off the audio output: Move to Trash and its redo play
  `dock/drag to trash.aif`; copy, move, duplicate, compress, Put Back and the undo of a trash or a
  duplicate play `system/Volume Mount.aif`; rename, new folder, make alias, their undos, Delete
  Immediately, mount and eject play nothing.
- [ ] Empty Trash's sound was not measured, as that empties the user's Trash; the demo plays
  `finder/empty trash.aif`. Finder's undo of Duplicate deletes the copy; the demo moves it to the
  Trash.
## Preview pane, measured against Finder on macOS 27

Finder 27 (26A428), dark and light, window 1100x680, sidebar 216, pane 241, list view.

- [ ] After a window resize AppKit leaves the demo's inspector 240.5 wide, its divider at x 859.0
  where Finder's is at 858.5; the card itself sits where Finder's does.
- [ ] GPUI shapes the system font without AppKit's per-size tracking: 11 pt values run 1.5–3.5 pt
  narrower than Finder's, 13–15 pt headings and names 1–2 pt wider.
- [ ] The rim around a picture: Finder's darkens a full point at the top and bottom edges, the
  demo's half a point, and on the right edge the demo's reads 8 levels lighter.
- [ ] Calculator.app: Finder's Quick Look draws the bundle's asset catalogue as a grid ("Showing 47
  of 275 assets"); the card shows the app's icon.
- [ ] A text file: Finder's Quick Look draws the text in a rounded, monospaced page 221x291; the
  card shows the document's thumbnail.
- [ ] Finder's kind line weighs a folder ("Folder - 6 bytes", "Folder - Zero bytes"); the card
  says "Folder" until the index has weighed it, and never says "Zero bytes".
- [ ] Finder keeps a second pane width for gallery view (`PreviewPaneGalleryWidth`, 240 on the test
  Mac); the demo has one width for every view.
- [ ] The rows under Show More are Finder's per-kind Preview Options (for a PNG here only "Content
  created"); the demo's list is fixed.
- [x] Light appearance: Finder's vibrant text darkens the glass by a fixed depth (headings and
  values 178.5, row titles 102, kind 51.5), and the glass lightens from 237 to 242 when the window
  resigns key; the card follows both, within a level key and non-key. The name is a flat 70% black
  (Finder 71 key, 73 non-key).
- [ ] After a live appearance switch Finder's kind line turns secondary (116 dark, 134 light) until
  its window is reopened; the card keeps the tertiary a window opens with (77 dark, 185 light).
- [x] The tags well: Finder fills it on the user's Mac (32 dark, 255 light, x 869–1090, y 549–582,
  "Add Tags…" at x 870, y 552–564.5, ink 87 dark, 189 light) and draws no rule under the tags row,
  key and non-key, for a picture and a text file; the card matches within a level, its placeholder
  1 pt wider. On the mini (26A5416b) Finder drew a rule there instead.
- [ ] Show Preview after Hide Preview brings Finder's pane back 288 wide in list and gallery view,
  at any window width (900–1300), selection or earlier width (241 and 240 set by the divider), and
  Finder saves 288; the demo's comes back at the width it had (241).
- [ ] Once the content runs into the actions band (a window 600 tall), Finder rules the band off
  across the pane, x 859–1100 (y 529.5: 68 dark, 214 light; y 530: 63, 230); the card's rule has
  the same rows and inks but spans x 862–1094, since its window keeps clear of the divider's grab
  and the window's resize edge.
- [ ] The quick actions' Rotate Left symbol sits 0.5 pt left of Finder's (x 897–911 against
  897.5–911.5); its row, Markup and More… match (626–641, 627–641).
- [x] Column view's preview column has no glass under it, and Finder draws its text in plain label
  colours there (dark: name and values 221, kind, titles and action labels 154, rules 54 on the
  column's 32; light: 39, 128, 230 on 255); the card uses them in column view.
- [ ] In column view the card's text sits 0.5 pt above Finder's (name 380.5 against 381.0, the
  same for every row down to "Add Tags…"), and its quick actions 3 pt above (symbols 595–610
  against 598–613, labels 622.5–633 against 625.5–636), Photo.png at 1100x680.
## Path bar and status bar

Measured against Finder 27.0 (26A428) on a 2x display, both windows at 240,120 1100x680 pt in list
view, points from the window's top-left, colours as sRGB 0–255. Re-measured on the integrated build,
dark and light: the edge rows (75, 56 dark; 221, 240 light) and every crumb's name start and
chevron (Δ 0.0) match; what is left:

- [ ] Finder's list runs on beneath the path bar, which lets a heavily blurred trace of it through:
  with a selected row under the bar the fill reads 40,42,43 in dark and 252,253,255 in light (255).
  The demo's list stops at the bar, so its bar stays 41,41,41 / 255.
- [ ] A crumb name cut to the shared width fades out over its last ~9 pt in Finder (140 → 32 across
  x 564–573 for "nativegpui-wave" capped at 66 pt); the demo's label clips it at the same x, so
  three capped names end 0.5 pt later.
- [ ] The startup-disk crumb's icon ink is 12 × 13.5 pt against Finder's 11 × 12.5 pt, on the same
  centre. Folder crumb icons match in dark and run 0.5 pt wider a side in light.
- [x] Like Finder, the demo paints the next alternating row into the 10 pt between the last row and
  the path bar (y 642–652, 42,42,42 in dark), including right after a scroll that ran past the end.
- [x] In column view the trail ends at a lone selected item that opens no column (an app, a symlink
  to one, a folder the user can't list), as Finder's does.
- [ ] Fills are a level off on one channel: the path bar 41,41,41 against Finder's 41,41,40 (40,41,40
  in a 3-item or empty folder), its bottom rim 62,62,62 against 61,62,61, the status bar 39,40,40
  against 39,40,39 with its divider 29,29,29 against 29,30,29, and the light status divider's first
  row 241 against 242.

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
- The sidebar's tint comes from where the window sits on the desktop picture, one tint for the whole
  window (AppKit's own window-background chameleon, in both apps), never from what lies behind the
  window: grey, magenta and a copy of the wallpaper behind it read identically. Over the yellow half of
  the desktop both read 40 40 37 across the rows, key and non-key, and the selection pill 71.3 71.5
  67.4 (Finder) against 70.4 71.2 67.4 (the demo); over the warm corner both read 38 38 38. A second
  `CAChameleonLayer` the demo once laid under its rows took the blue channel 1–2 levels low and was
  removed.
- Ink, dark: Favorites icons, labels (255 key, 147 non-key) and the eject key ink (105.5) match Finder.
  Locations icons are `systemGrayColor` while key (181 181 186 in both) and AppKit's inactive tint
  otherwise (144 in both): a label colour on a vibrant image view draws far darker than as text
  (secondary 94, tertiary 32). The non-key eject reads 72.8 against Finder's 70.5.
- Favorites and Locations come from Finder's lists through `sharedfilelistd`: macOS refuses an app
  (not a shell) the list files themselves, which left the demo on its built-in rows. Tags stay the 7
  standard ones where Finder lists its 14: Finder keeps them in its iCloud key-value store, whose
  database an app may not open either.
- A mounted server is one row named by its host, as in Finder, with CoreTypes' `SidebarDisplay`;
  Finder 27 draws a flat box with a rule and a dot, not yet compared glyph for glyph. The row opens the
  first share mounted from that host, where Finder shows the host's shares, and its eject unmounts that
  share only.
- A cloud storage row (a File Provider domain) shows `externaldrive.badge.icloud`; Finder shows the
  provider's own icon.
- Finder's Computer and Remote Disc rows are not shown; Network always is.
- The demo reads Finder's lists and never writes them, so Remove from Sidebar and reordering of a row
  that comes from them last until the next launch.
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

- [ ] Rows: the demo now takes Finder's list text and icon sizes and Finder's own row rule,
  max(icon, ceil(ascent + descent)) + 4 (20 at 13 pt text, 21 at 14, 36 at 32 pt icons); only 20
  (mini) and 21 (this Mac) were read off Finder's windows.
- [ ] Header rule, dark: Finder 1pt #414241 with 0.5pt #303030 beneath; the demo 1pt #3d3d3d.
- [ ] Kind column: Finder shortens a kind to the fullest of its own names for it that fits
  (`KIND_FORMATTER_<n>_<m>` in Finder's strings: "Plain Text Document" → "Plain Text", "PDF
  document" → "PDF Document", "QuickTime movie" → "QT movie", "Pages document" → "Pages" at 121 pt).
  The demo reads the same table and fits 20 pt inside the column: 17 kinds matched on the mini;
  Finder's exact slack lies between 100.87 and 102.95 pt of text for a 121 pt column, not swept.
- [x] Date column: one style for the whole column, as Finder; see the user's Mac below.
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
  38 in 76, which moves the title 1pt right; AirDrop is 44 wide, the demo's 40. With nothing
  selected the demo greys Share, Tags ("Edit Tags"; "Add Tags" with a selection) and AirDrop as
  Finder does, greys Share and AirDrop while any chosen item is a folder the user can't list (Tags
  stays on), and shows AirDrop only when Finder's toolbar has it. The demo's Progress item has no
  Finder counterpart.
- [ ] Get Info, Connect to Server, New Folder, rename, conflicts, the tag editor, the tab bar and
  the progress window were captured from Finder 27 but not yet compared with the demo's.
- [ ] Go to Folder: the demo's panel sits where Finder's default one does on the user's Mac
  (460×183, centred on the window, 102 pt below its top); Finder reopens it at the size it was last
  resized to (`NSWindow Frame GoToSheet`, 854×345 there). A path longer than the field wraps to a
  second line in the demo, and for a folder the user can't list Finder lists nothing under the
  field where the demo lists its path.
- Measured on the user's Mac (textSize 14, iconSize 16, "Show all filename extensions" on, so both
  apps show full names): rows 21 pt and stripes below the last row on the same 21 pt pitch, header
  labels within 0.5 pt, titles in the same glyph positions. Differences left:
  - Date column: Finder shows one style for the whole column, whatever the rows hold (a 17-row
    folder and two one-file folders switched at the same widths): long from 229 pt (225 is medium),
    medium from 183 (181 is short), short from 148 (147 is the date alone, today's row its time).
    The demo now picks one style from two samples (yesterday at 10 PM, six days ago at 10 AM) with
    16 pt to spare, short styles in monospaced digits, which lands on those widths at 14 pt text;
    that the samples are Finder's own was inferred from the thresholds on one day, not read from it.
  - A kind too long for the column: both cut in the middle now, at different places ("Comm…t
    (.csv)" in Finder, "Comma S….csv)" in the demo).
  - The window title's ink is 233 in Finder and 223 in the demo (same glyphs, dark, key).
  - Header rule, dark: Finder 65,66,65 for a point then 48 for half a point, the demo 61 for a point.
  - Toolbar: Finder greys Share, Tags and AirDrop whenever nothing is selected, as the demo does,
    and a click below the last row empties the selection in both. A window Finder opens in the
    user's gallery style has its first item (by name) selected, which list view keeps: that was
    the "enabled in a new window" reading.
## Capability, speed and reliability

Measured against Finder 27.0 (26A428) on the same Mac, on a fixture tree of hostile names,
symlinks, permission cases, 100 000- and 500 000-item folders and a WebDAV share made to stop
answering.

- A folder Finder can't open (no read or search permission, or an ACL denying list) gets Finder's
  own words, “The folder “X” can’t be opened because you don’t have permission to see its
  contents.”, in an app-modal critical alert centred on the screen (260×208, caution icon), and
  the window stays where it was with the folder selected. The demo shows the same alert, run from
  a task so its modal loop does not re-enter GPUI's borrow of the app. As in Finder, only a
  double-click shows it: File ▸ Open, Open in New Tab and Open in New Window are off and ⌘↓ does
  nothing while every chosen item is such a folder, Share and AirDrop are off while any is, its list
  row has no disclosure triangle and its column row no chevron, and Go to Folder on one leaves its
  sheet up and says nothing. On the user's Mac both alerts are 260×208 at 993,343, layer 8, with
  the same text and button frames; the alert's badge is the demo's own app icon where Finder's is
  Finder's. “The operation can’t be completed because the original item for “X” can’t be found.”
  on a dangling symlink is still a sheet on the window.
- An operation the user's permissions refuse (copying a file they can't read, pasting into a
  folder they can't write): Finder offers to do it as an administrator, “Finder wants to copy
  “noread.txt”. Touch ID or enter your password to continue with “Finder”.”, and for a folder it
  can't read it makes the empty destination first and then asks “You may need to enter the name
  and password for an administrator on this computer to change the item named “noread”.” (Stop,
  Continue). The demo has no privileged helper to act as an administrator, so it reports the
  failure in a sheet instead, in the system's words, which for an unreadable file name the
  destination: “noread.txt” couldn’t be copied because you don’t have permission to access
  “drop2”.
- A link dragged from a browser: Finder saves `<page title>.webloc` (an XML property list with one
  `URL` key, type `ilht`, creator `MACS`, extension hidden). The demo's window accepts the drop
  (the source is told Copy) and saves nothing: GPUI's drop hands it the address without the title.
- Dragged text: Finder saves `<first words>.textClipping`; GPUI does not take text drags, so the
  demo refuses them (the source is told None).
- The demo does not watch folders on a network share: one call into a share that stops answering
  would hold the watcher and every other folder it watches. A folder there refreshes when it is
  opened and after the demo's own operations. Whether Finder shows a change made on the server as
  it happens was not measured.
- A folder on a share that stops answering stays empty while the window stays responsive (largest
  gap on the main thread 33 ms across the 4.6 s the share was stopped); nothing says why. What
  Finder shows while a share times out was not measured.
- Renaming with a new extension or a leading dot happens without a question. Finder's questions
  were not captured: typing into Finder's name field by synthetic keys did not register in three
  attempts.
- When a folder above the one shown is renamed, the demo follows to the new path; Finder's window
  title still read the old path 2.5 s later.
- Names follow LaunchServices' display name wherever an item can lose part of it (Finder's
  hide-extension flag, any `.app`, `.localized`), which on the mini matched Finder for all 40 names
  probed, quirks included ("Up.APP" shows as "Up.APP.app"). Search results come from the index,
  which keeps no hide-extension flag, so a flagged document there keeps its extension. A rename
  field shows the whole name where Finder's shows the displayed one.
