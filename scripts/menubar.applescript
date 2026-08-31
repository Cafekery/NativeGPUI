-- Dump one process's whole menu bar as TSV: menu, row title, key equivalent, modifier mask, enabled.
--
-- Reading another app's menus needs an Accessibility grant but NOT keyboard focus, which is the
-- whole reason this exists: it is the one instrument here that can check the bar without taking
-- the screen from anyone.
--
-- Every attribute is read WITHOUT coercing to text first. `value of attribute "…" as text` turns
-- an absent attribute into the literal string "missing value", so a row with no key equivalent
-- compares unequal to an empty expectation and six correct rows reported MISSING.
on run argv
	set thePid to (item 1 of argv) as integer
	set out to ""
	tell application "System Events"
		set proc to first application process whose unix id is thePid
		-- Reached before anything else, and deliberately not inside a `try`: it is the one read
		-- that needs the Accessibility grant, so a missing grant has to error here rather than be
		-- mistaken for an app with no menu bar.
		set probeName to name of proc
		set barCount to 0
		try
			tell proc
				set barCount to count of menu bar items of menu bar 1
			end tell
		end try
		-- An app that installed no bar has no `menu bar 1` at all: System Events raises
		-- "Invalid index" rather than returning an empty list. Reported as a row, so the caller
		-- distinguishes it from a read that failed.
		if barCount is 0 then return "NOBAR" & tab & probeName & tab & tab & tab & linefeed
		repeat with b from 1 to barCount
			tell proc
				set barName to name of menu bar item b of menu bar 1
			end tell
			if barName is missing value then set barName to "?"
			set rowCount to 0
			try
				tell proc
					set rowCount to count of menu items of menu 1 of menu bar item b of menu bar 1
				end tell
			end try
			repeat with r from 1 to rowCount
				tell proc
					set theRow to menu item r of menu 1 of menu bar item b of menu bar 1
					set rowName to name of theRow
					set ch to missing value
					try
						set ch to value of attribute "AXMenuItemCmdChar" of theRow
					end try
					set md to missing value
					try
						set md to value of attribute "AXMenuItemCmdModifiers" of theRow
					end try
					set en to (enabled of theRow) as text
				end tell
				if rowName is missing value then set rowName to "-"
				if rowName is "" then set rowName to "-"
				if ch is missing value then
					set ch to ""
				else
					set ch to ch as text
				end if
				if md is missing value then
					set md to ""
				else
					set md to md as text
				end if
				set out to out & barName & tab & rowName & tab & ch & tab & md & tab & en & linefeed
			end repeat
		end repeat
	end tell
	return out
end run
