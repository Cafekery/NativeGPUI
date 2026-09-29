# What Finder has that the demo does not

Taken from Finder 26.5's own menu bar, read through the accessibility API and diffed against the
demo's. Items Finder greys out when nothing is selected are still features; items the demo has in
a context menu but not in the menu bar are listed as wiring, not as features.

## Wiring: the demo has the action, the menu bar does not offer it

- [x] File ▸ Open With, and Always Open With under ⌥: LaunchServices' apps for the whole selection,
  the default first above a rule, then App Store… and Other…
- [x] File ▸ Compress
- [x] File ▸ Share…
- [x] File ▸ Add to Dock (⇧⌃⌘T), the ⇧ alternate of Add to Sidebar
- [x] File ▸ Open and Close Window (⌥⌘O), Open's ⌥ alternate; it reads Open in New Window and Close
  while only folders are chosen
- [x] File ▸ Eject One (⌃⌘E) and Eject All (⌥⌘E), Eject's alternates
- [x] Go ▸ Library, which Finder shows only while ⌥ is held
- [x] Go ▸ Select Startup Disk
- [x] Go ▸ Open Enclosing Folder in New Window (⌃⌘↑)

## Small features

- [x] Edit ▸ Search by Name… (⇧⌃⌘F): a search scoped to the name, which the criteria row can carry
- [x] File ▸ Get Summary Info (⌃⌘I): one window for a whole selection
- [ ] Edit ▸ Copy as Link (⌃⌥⌘C), Copy's ⌥ alternate: the row is there and always off. Finder kept
  it off on a local file and on an iCloud Drive file alike, and its binary ties
  `validateCopyAsLink:` to CloudSharingUI's CopyLink, so what it copies, and when, is unconfirmed
- [x] Edit ▸ Paste Exactly (⇧⌥⌘V), and Paste Item Exactly in the context menus: a copy that then
  gives each item its original's owner and group, or the group alone where the process may not
  give the item away. A plain copy takes the destination folder's group
- [x] Edit ▸ Show Clipboard
- [x] Window ▸ Cycle Through Windows (⌘`)
- [x] Window ▸ Show Progress Window: a Progress window of the running operations; the row reads
  Hide Progress Window while it is open, and is off while nothing runs and it is closed
- [x] View ▸ Show Preview Options: Finder's floating 240 pt panel "Preview Options - <kind>" (Image, Text, PDF document, Movie, Folder read off Finder 27): Tags, Dates, the kind's groups and Show Quick Actions; the demo draws the same rows and keeps the choices per kind
- [x] View ▸ Customize Toolbar…: AppKit's palette, over a toolbar that starts from Finder's own
  arrangement (`NSToolbar Configuration Browser`)
- [x] Tag swatches read as seven unlabelled groups to VoiceOver; Finder's are radio buttons named “Add “Red” label” and “Remove “Red” label”

## Features

- [x] Go ▸ Recents (⇧⌘F)
- [x] Go ▸ Connect to Server… (⌘K)
- [x] Go ▸ AirDrop (⇧⌘R) opens an AirDrop location in the window, titled AirDrop as Finder's;
  items dropped on it, and the toolbar's AirDrop button, send through AppKit's AirDrop sheet
- [ ] The AirDrop location lists nothing. Finder 27 shows DeviceDiscoveryUI's SwiftUI view, not
  its AirDropInfoView.nib, and no `airdrop` symbol: top left, 16 pt in, the Contacts me-card photo
  (48 pt, round) beside “Use AirDrop as” (13 pt) over the name (15 pt semibold); bottom left, 16 pt
  in, “AirDrop lets you share instantly with people nearby.” (12 pt) over “Allow me to be
  discovered by: ” (12 pt) and a 114×20 pop-up of No One, Contacts Only and Everyone
  (`DiscoverableMode` in `com.apple.sharingd`), its bottom 12 pt above the status bar. No path bar;
  the view switcher, Group and Action are off. Setting the mode needs sharingd's private
  entitlement `com.apple.private.airdrop.settings`; System Settings' pane is
  `x-apple.systempreferences:com.apple.AirDrop-Handoff-Settings.extension`
- [x] Go ▸ Computer (⇧⌘C) and Network (⇧⌘K) open in the window, titled by the computer's name and
  “Network” as Finder's, nothing selected on arrival. Computer lists the mounted volumes and a
  Network row; Network lists the computers sharingd's browser finds (the list Finder shows), each
  with its model's icon, and opening one connects over SMB, else AFP, else screen sharing
- [x] Enclosing Folder (⌘↑) from a volume's root opens Computer with the volume selected, and
  ⌃⌘↑ opens it in a new window, as Finder's do; Select Startup Disk shows Computer with the
  startup disk selected
- [x] Column view rooted at Computer keeps Computer's column first however deep the selection
  goes; a folder reached any other way starts its columns at its volume
- [x] The path bar and the toolbar's Path menu know the locations, as Finder's: a folder's path
  starts at its volume (“Macintosh HD › Users › …”, no computer crumb); at Computer the lone crumb
  is the computer's name with its icon, at Network “Network”, in Shared and Recents nothing until
  an item is chosen, and AirDrop has no path bar. A chosen volume shows alone. The Path menu lists a
  location's own chain (Network, then the computer) and its computer row opens Computer
- [x] Go ▸ Shared (⇧⌘S), after iCloud Drive as in Finder's menu: the items Spotlight marks as shared
  (`kMDItemIsShared == 1`)
- [ ] Shared lists its items without Finder's Shared By column and grouping. Finder's Shared By
  (`shareOwner`, 200 pt, after Date Last Opened) shows the owner's name (“William Jones” from the
  share's name components, else `kMDItemOwnerName`: “Guillaume C.”); Group By Shared By heads each
  group “Shared By <owner>” (“Shared By Me” for the owner, `kMDItemSharedItemCurrentUserRole`),
  groups by name ascending. With neither on, the name cell carries “Shared by WJ” (the abbreviated
  name). Its View Options and Sort By list Shared By and Last Modified By
- [x] File ▸ New Smart Folder, and saved searches with it: Save in a search's scope bar writes a .savedSearch in Finder's own format to ~/Library/Saved Searches and adds it to the sidebar, and opening one, the demo's or Finder's, runs its query through Spotlight. The Save sheet asks only for the name, without Finder's tags, Where and Add To Sidebar controls
- [x] File ▸ Print (⌘P)
- [x] File ▸ Slideshow (⌥⌘Y)
- [ ] File ▸ Create/Copy iCloud Link and Manage Shared File…/Folder…: no API was found whose effect
  could be checked
- [x] Quick Actions, in the item menu and the preview pane, as Finder offers them: Rotate Left (⌥
  Rotate Right) for pictures and movies, Markup for one picture or PDF, Trim for one movie, Create
  PDF for one picture or several pictures and PDFs, Convert Image and Remove Background for
  pictures, then Customize…. The pane shows the first two and More…, its first button reading
  Rotate Right while ⌥ is held. Create PDF, Convert Image and Remove Background start a rename of
  what they make, as Finder's do; Rotate only selects
- [x] Trim opens a trimmer, then Finder's question, “Do you want to save the changes made to the
  document “X”?”, with New Clip, Replace and Cancel in Finder's order. Replace wrote the same file
  as Finder's (a 2 s clip cut to 1.5 s, 13,140 bytes); New Clip saves a new file through a save
  sheet with name, Where and Tags. Neither app can undo it
- [ ] Quick Actions list none of the user's own (Shortcuts, Automator); Markup, which opens Quick
  Look's Markup mode, is only in the build with the `private-api` feature
- [ ] AVKit's trimmer is a window titled by the file, 370×310, with Trim (off until a handle moves)
  and Cancel; Finder's is titled Quick Look, 370×283, with Done and Revert. Convert Image's window
  reads as a dialog (`AXDialog`) where Finder's is a standard window
- [x] App menu ▸ Settings… (⌘,), between About and Empty Trash: Finder's General, Tags, Sidebar and
  Advanced panes in one window 377 wide, 497, 591, 828 and 392 tall, the last pane remembered. Each
  choice is kept in the demo's own defaults under Finder's key and read before Finder's, so
  Finder's settings are never written
- [ ] Settings has no Sync Desktop & Documents folders, Suggest file names or Tint folders based on
  tags (a system-wide setting no app can override for itself), and only stores Remove items from
  the Trash after 30 days and the desktop's settings. Its tag list holds only the sidebar's tags
  where Finder's has every tag, favourites can't be dragged into its strip, and its panes switch
  by a segmented control where Finder's toolbar has four buttons
- [x] Quit and Keep Windows (⌥⌘Q): the next launch reopens the windows and tabs that were open;
  after a plain Quit only one window opens
- [x] The Services submenu

## ⌥ in context menus

Finder swaps some context-menu rows while ⌥ is held. Read off Finder 27's menus for a file, a
folder, two items, the folder's background and a sidebar item through the accessibility API,
which lists each alternate with its modifier mask; the demo's menus carry the same alternates.

- [x] Open With becomes Always Open With (the same app list, and the choice sticks for that kind of
  file)
- [x] Get Info becomes Show Inspector, in every menu that has it; with several items chosen, Get
  Summary Info sits under ⌃
- [x] Quick Look becomes Slideshow “X” (Slideshow 2 Items), Copy becomes Copy “X” as Pathname (Copy
  2 Items as Pathnames), and Duplicate becomes Duplicate Exactly under ⌥⇧
- [x] Compress has a ⌥ twin (the same Compress “X” for one item, Compress 2 Items for two) that
  makes the same plain archive: Finder 27's wrote an unencrypted zip and asked nothing
- [x] A folder and the folder's background offer Paste Item, which becomes Paste Item Exactly under
  ⌥⇧; no other menu offers it, and none has a ⌥ Move Item Here
- [x] A folder or a sidebar item: Open in New Tab becomes Open in New Window
- [x] Move to Trash has no alternate
- [ ] Finder's item menu has Show Preview Options after the tags; the demo's has not

## Toolbar: where the search field shrinks and collapses

Read over accessibility from Finder 27.0 on the test Mac and from the demo, same folder, list view,
preview pane at 241, window width stepped 1pt around every transition, at sidebar 300, 216, 158 and
hidden. Every toolbar item's x and width, the search field's width and state, the view switcher's
state and the overflow order now match Finder at every width both sampled (about 600 rows). Finder's
rule: the title keeps a quarter of the window width; the search field gives way first (329 → 164,
then its 40pt button), then the view switcher (164 → 50pt popup), then the title (to 160), the last
space and, last, items into the overflow menu. Every threshold moves 4/3pt per point of sidebar.

- [x] The demo's sidebar opens at 216 and stops at 158, as a new Finder window's, and its preview
  pane takes Finder's saved width (`PreviewPaneWidth`, 241 here, and `PreviewPaneGalleryWidth` in
  gallery view), 288 when there is none, down to 240. At those defaults Finder starts shrinking
  the field at 1365, collapses it at 1145 and collapses the view switcher at 979.
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
- [x] Icon view keeps each icon where it stands, as Finder's free arrangement does: a folder's
  icons take Finder's recorded places (`Iloc`, the centre of each image), then those moved here,
  and the rest fill the first free slots in name order; Finder writes a folder's places only once
  one of them changes, so an untouched folder is laid out afresh on each visit. Icon size, grid
  spacing, text size and label position are per folder, from Finder's `icvp` record or the View
  Options panel, the status bar slider and ⌘+/⌘−; changing one moves each icon to the same place
  in the new grid. Finder's grid: an icon framed at 9/8 of its size, cells as wide as that frame
  plus 10 or the label box (which grows with the grid spacing in steps of the text's line height),
  10 pt margins, rows filled across, or down the window's height with labels on the right. Kept
  sorted, rows spread evenly across the window 4 pt in from each side (columns packed down it
  with labels on the right). Dragging moves an icon by the pointer's travel, or under Snap to
  Grid to the nearest free slot (Finder: alpha dropped at 300,150 went to 402,270, as here).
  Items dropped in from another window or app land where they are let go, or in the nearest free
  slot. Finder lays the first icons of a new window out for its default window (998 pt wide here:
  4 columns in a window widened to 1100, overflowing one narrowed to 800) unless it built the
  window on the spot (the first after its settings change, or one opened right after another: 5);
  the demo does the same with its own default window, and lays its launch window out for its own
  width. Left: Finder's drop from its own windows lands 10 pt left and 3 pt above the drag image,
  and under Snap to Grid sets it in the chosen cell's top left corner (24 pt left, 4 pt up of its
  slot); the demo keeps dropped icons on their slots and cannot learn where another app's drag
  image was grabbed, so it centres the icon on the pointer.
- [ ] Finder draws icon view's icons on beneath its preview pane's glass; the demo's stop at the
  pane.
- [x] Column view with an unreadable folder selected: as in Finder, its row has no chevron, nothing
  opens, the preview column shows the badged folder and the path bar ends at it. Finder's kind line
  reads “Folder - Zero bytes”, the demo's “Folder”.
- [x] Column view sorts by `ColumnViewOptions.ArrangeBy` (dnam: name) and gallery view by
  `GalleryViewSettings.arrangeBy`, each in the direction Finder first sorts that key, not by the
  list's sort; the demo sorts the rows before it shows the view, and keeps each view's own key.
  Finder's Sort By in column and gallery view lists the same eight keys, ✓ on that view's key.
- [x] Switching to column view with an item selected deep in a long path: Finder shows the last
  column at once. The demo laid the path's columns out as their folders were read and then slid
  sideways to the end (0 to -1201 pt over about 190 ms on a 14-column path); it now shows the end
  in every frame, and only a column the selection opens slides in.
- [x] Sort By: Name ⌃⌥⌘1, Kind ⌃⌥⌘2, Date Last Opened ⌃⌥⌘3, Date Added ⌃⌥⌘4, Date Modified ⌃⌥⌘5,
  Date Created, Size ⌃⌥⌘6, Tags ⌃⌥⌘7; Clean Up By: Name, Kind, Date Modified, Date Created, Size,
  Tags (⌥⌘). Read off Finder 27 with lists of a 7-item fixture: Date Last Opened is Spotlight's
  last-used date, which it makes up from the modification date where it has not indexed the item;
  Tags sort by the first tag's name; an item a column says nothing for (no tag, no version) lists
  last whichever way the column sorts; the Tags column shows the names ("Work, Blue"), Version a
  bundle's short version, both blank for nothing.
- [x] Comments: Finder shows the comment its folder's .DS_Store keeps (`cmmt`, which Finder's
  `set comment` writes along with the `kMDItemFinderComment` attribute), never the attribute alone:
  an item with only the attribute shows blank. The column is 300 pt, its text set in as Kind's,
  and it sorts by the comment with blanks first ascending and last descending, ties by name (read
  off Finder 27 over AX: Apple, Elder, Fig, "alpha note", "beta note", "cmmt only"). The demo
  reads `cmmt` with the other extras, off the main thread, and offers the column in the header
  menu and View Options, which lists Finder's Show Columns in its order.
- [ ] Shared By, Last Modified By and View Options' iCloud Status: Finder offers them only in
  iCloud Drive and shows neither column elsewhere, even when its settings ask for them; the demo
  lists them disabled everywhere.
- [x] Column widths, read over AX in a Finder window at eight widths (14 pt text, 16 pt icons):
  Name takes what the other columns leave while the list fills the window, down to its least
  (198); below that the list overflows and every width holds until the window outgrows the
  columns again. The last column draws 6 pt wider than Finder keeps it (Kind 121 last, 115
  otherwise; Comments 306 of 300; Date Added 187 of 181), and a list that overflows as it opens
  shows Name at 206 at least (Finder's kept Name plus 6 when that is wider: 250 opened at 256).
  The demo matches at equal list widths; it keeps its own widths, not Finder's.
- [ ] In the same window the demo's list runs 4 pt wider than Finder's; the widths above were read
  with the demo's window 4 pt narrower.
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
  `finder/empty trash.aif`.
- [x] Undo of Duplicate deletes the copy outright, as Finder's does.
- [x] ⌘I with nothing chosen shows Info for the folder shown, and a selection opens one Info window
  per item however many there are, as in Finder. Get Summary Info (⌃⌘I) opens an item's own Info
  window for one item and one “Multiple Item Info” window, headed “N items”, for several.
- [x] ⌘I with nothing chosen at Computer, Network and AirDrop opens “Jace’s MacBook Pro Info”,
  “Network Info” and “AirDrop Info”, as Finder's: the computer's processor and memory, the others'
  kind (Neighborhood, AirDrop). In Recents and Shared Finder turns Get Info and Get Summary Info off
  until something is chosen, and so does the demo.
- [x] ⌘N and ⌘T open what Settings' “New Finder windows show” names (`NewWindowTarget`,
  `NewWindowTargetPath`), not the folder shown.
- [x] Make Alias and New Folder with Selection start renaming what they make, as Finder's do.
- [x] ⌥↑ and ⌥↓ select the first and last item in list view, as in Finder.
- [ ] In column view ⌥↓ still moves one row; Finder's ⌥↑ and ⌥↓ were not read in column, icon or
  gallery view.
- [x] Go ▸ Utilities (⇧⌘U) opens /Applications/Utilities.
- [ ] Finder shows /System/Applications' apps inside /Applications and its Utilities; the demo lists
  each folder as it is on disk.
- [x] Quick Look (⌘Y or Space) with nothing selected previews the folder shown, and follows the
  selection once there is one; File reads Quick Look “<folder>” and Slideshow “<folder>”, then
  Close Quick Look and Slideshow while the panel is up.
- [ ] Escape out of the ⌥⌘Y slideshow leaves the demo's Quick Look panel up in a window, where
  Finder's closes; while that panel is key the Window menu's tab rows read on, where Finder's
  stay off.
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
- [x] Finder's kind line weighs a folder ("Folder - 6 bytes", "Folder - Zero bytes"); the card
  weighs one the index has not, and says "Folder" only until it has.
- [x] Finder keeps a second pane width for gallery view (`PreviewPaneGalleryWidth`, 240 on the test
  Mac), and a window switched into or out of gallery view takes that view's width; so does the demo.
- [x] The rows under Show More are Finder's per-kind Preview Options (for a PNG here only "Content
  created"); the demo's follow the same options.
- [x] Media values read as Finder's: durations rounded to the second (00:02, 01:15, 01:02:05),
  sample rates in kHz (44.1 kHz, 48 kHz), channels Mono, Stereo or a count, dimensions and
  resolution with a hair space either side of × (320 × 240), codecs in the reverse of Spotlight's
  order (MPEG-4 AAC, H.264), colour space RGB.
- [ ] Finder's Preview Options panel takes key as it opens and shows its title in its title bar;
  the demo's never takes key and hides its title. Its tables for audio, applications and every
  other kind were not read off Finder.
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
- [x] Show Preview after Hide Preview brings Finder's pane back 288 wide in list and gallery view,
  at any window width (900–1300), selection or earlier width (241 and 240 set by the divider), and
  Finder saves 288 for that view; the demo does the same.
- [ ] Once the content runs into the actions band (a window 600 tall), Finder rules the band off
  across the pane, x 859–1100 (y 529.5: 68 dark, 214 light; y 530: 63, 230); the card's rule has
  the same rows and inks but spans x 862–1094, since its window keeps clear of the divider's grab
  and the window's resize edge.
- [ ] The quick actions' Rotate Left symbol sits 0.5 pt left of Finder's (x 897–911 against
  897.5–911.5); its row, Markup and More… match (626–641, 627–641).
- [ ] The pane's quick action buttons have no tooltips; Finder's first button's help reads Rotate
  Right while ⌥ is held.
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
- Favorites, Locations and Tags come from Finder's lists through `sharedfilelistd`: macOS refuses
  an app (not a shell) the list files themselves, which left the demo on its built-in rows. Tags
  are the ones Finder's sidebar lists, in its order, pinned and recent alike; the menus' swatches
  are Finder's favourite tags (`FavoriteTagNames`), the 7 standard ones when it has none.
- A mounted server is one row named by its host, as in Finder, with CoreTypes' `SidebarDisplay`;
  Finder 27 draws a flat box with a rule and a dot, not yet compared glyph for glyph. The row opens the
  first share mounted from that host, where Finder shows the host's shares, and its eject unmounts that
  share only.
- A cloud storage row (a File Provider domain) shows `externaldrive.badge.icloud` and the folder's
  name (S3Mount-NoahBucket); Finder shows the provider's own icon and name (Noah Bucket), which no
  public API gives.
- Finder's Computer and Remote Disc rows are not shown; Network is, unless Settings turns off
  Bonjour computers.
- Remove from Sidebar and reordering of a Favorites or Locations row last across launches in the
  demo's own list; Finder's lists are read, never written.
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
- [x] Column view with nothing selected: as in Finder, the ancestor columns show with the current
  folder selected in its parent's.
- [x] Menu enabling: with nothing selected Finder disables New Folder with Selection, Rename,
  Compress, Duplicate, Make Alias, Share…, Show Original, Move to Trash, Eject, Print, Cut, Copy,
  Deselect All, Back and Forward without history, Clean Up in list view and Show Progress Window
  with no operation running; so does the demo. Clean Up and Clean Up By need icon view without a
  sort, and the Window menu's tab rows stay off until the window has tabs.
- [ ] Sort By's None and Snap to Grid: Finder's menu has them only in icon view, the demo's shows
  them greyed in the other views. AppKit's Remove Window from Set sits above Cycle Through Windows,
  where Finder's order needs AppKit's private `_cycleWindows:`.
- [x] Menu titles name their target: Quick Look “Fixture”, Slideshow “Fixture”, Copy “Fixture” as
  Pathname, New Folder with “Notes.txt”, Copy “Notes.txt”, Paste Item; Show turns to Hide on
  Preview, Path Bar, Status Bar, Inspector, View Options, Preview Options and Progress Window.
- [ ] With a folder chosen Finder's tag row ends in Customize Folder…; the demo's stays Tags…, having
  no folder customisation.
- [x] File menu, in Finder's order with its alternates: New Finder Window, New Folder, New Folder
  with Selection, New Smart Folder, New Tab, Open (⌃ Open in New Tab, ⌥ Open and Close Window ⌥⌘O),
  Open With (⌥ Always Open With), Close Window (⌥ Close All), Get Info (⌥ Show Inspector, ⌃ Get
  Summary Info), Rename, Compress, Duplicate (⌥⇧ Duplicate Exactly), Make Alias, Quick Look (⌥
  Slideshow), Share…, Show Original, Add to Sidebar (⇧ Add to Dock), Move to Trash (⌥ Delete
  Immediately…), Eject, the tag row (the favourite tags' swatches and Tags… in one row), Print.
  Finder's Manage Shared Folder… is missing.
- [x] Edit menu, as Finder's: Undo, Redo, Cut, Copy (⌥ Copy as Pathname, ⌃⌥ Copy as Link), Paste
  (⌥ Move Items Here, ⇧⌥ Paste Exactly), Select All (⌥ Deselect All), Show Clipboard, Search (⌃
  Search by Name…), then AppKit's Writing Tools, AutoFill, Start Dictation and Emoji & Symbols,
  with no Paste and Match Style or Delete. While a text field has the keyboard the rows act on its
  text (Undo Typing, Paste); otherwise Cut stays off.
- [ ] Move Items Here turns on only for a copy made in the demo or items carrying Finder's
  `com.apple.finder.noderef`: a rule taken from one reading (a file copied elsewhere left it off)
  and Finder's binary.
- [x] View menu, as Finder's: Use Groups before Sort By; Sort By with None ⌃⌥⌘0 and Snap to Grid,
  then Name, Kind, Date Last Opened, Date Added, Date Modified, Date Created, Size and Tags; Clean
  Up, ⌥ Clean Up Selection, Clean Up By; after the sidebar: Preview, Toolbar, Path Bar, Status Bar,
  Customize Toolbar…, View Options, Preview Options. Show Hidden Files (⇧⌘.) is a key, not a row,
  as in Finder.
- [x] App menu: Settings… (⌘,), Empty Trash… with ⌥ Empty Trash, and Quit and Keep Windows (⌥⌘Q).
- [x] Go menu: Shared (⇧⌘S) after iCloud Drive, Library under ⌥, the user's own cloud locations,
  and Recent Folders above Go to Folder…. Help: Mac User Guide and Tips for Your Mac, as Finder's.
- [ ] Toolbar: Finder's back and forward segments are 36 and 37 wide in a 75pt group, the demo's 38 and
  38 in 76, which moves the title 1pt right; AirDrop is 44 wide, the demo's 40. With nothing
  selected the demo greys Share, Tags ("Edit Tags"; "Add Tags" with a selection) and AirDrop as
  Finder does, greys Share and AirDrop while any chosen item is a folder the user can't list (Tags
  stays on), and shows AirDrop only when Finder's toolbar has it. The demo's Progress item has no
  Finder counterpart.
- [ ] Get Info, New Folder, rename, conflicts, the tag editor, the tab bar and the progress window
  were captured from Finder 27 but not yet compared with the demo's.
- [x] Go to Folder: a sheet hung 102 pt below the window's top, as Finder's, between 434×183 and
  854×345, reopened at its last size under Finder's own autosave name (`NSWindow Frame
  GoToSheet`, Finder's size until the demo has saved one) and holding the last path gone to.
  AppKit on macOS 27 centres every sheet unless the sheet answers the private `sheetBehavior` with
  3 and the window places it through `window:willPositionSheet:usingRect:`; both have to go on the
  class the window reports, not its KVO subclass, or AppKit never asks.
- [ ] Go to Folder: a path longer than the field wraps to a second line in the demo, and for a
  folder the user can't list Finder lists nothing under the field where the demo lists its path.
- [x] Connect to Server: its own window at Finder's saved frame (`FXConnectToBounds`), 355×234 at
  least, zoom off, as Finder's: the last server (`FXConnectToLastURL`) in a combo box of recent
  servers, Favorite Servers with + and −, an action menu with Clear Recent Servers… (⌥ Clear Recent
  Servers), Help, Browse (a new window on Network) and Connect. Favourites and recents come from
  Finder's lists until the demo keeps its own.
- [ ] Connect to Server: Clear Recent Servers' confirmation, the Help page and Finder's answer to a
  cancelled sign-in were not measured; the demo says nothing on a cancelled sign-in.
- [x] View Options: a floating panel that does not activate (`AXSystemFloatingWindow`, as Finder's),
  titled with the folder, which ⌘J opens and closes from either window. In list view it lists
  Finder's twelve Show Columns in Finder's order.
- [ ] Finder's list-view View Options panel is 236×646 and also holds Group By and text size among
  other rows; the demo's is 268×341, with Sort By and Show Columns only.
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
- Changing or removing an extension asks first, while Settings' warning is on, in a sheet with
  Finder's words (“Are you sure you want to change the extension from “.x” to “.y”?”) and buttons
  (Keep .x, then Use .y or Remove), taken from Finder's strings, not a capture. An extension given
  to a name that had none, or a leading dot, renames without a question. Finder's questions were
  not captured: typing into Finder's name field by synthetic keys did not register in three
  attempts.
- When a folder above the one shown is renamed, the demo follows to the new path; Finder's window
  title still read the old path 2.5 s later.
- Names follow LaunchServices' display name wherever an item can lose part of it (Finder's
  hide-extension flag, any `.app`, `.localized`), which on the mini matched Finder for all 40 names
  probed, quirks included ("Up.APP" shows as "Up.APP.app"). Search results come from the index,
  which keeps no hide-extension flag, so a flagged document there keeps its extension. A rename
  field shows the whole name where Finder's shows the displayed one.
