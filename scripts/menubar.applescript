-- usage: osascript menubar.applescript <pid> — dump that process's menu bar as TSV:
-- menu, row title, key equivalent, modifier mask, enabled.
on run argv
	set thePid to (item 1 of argv) as integer
	set out to ""
	tell application "System Events"
		set proc to first application process whose unix id is thePid
		-- Outside any `try`: a missing Accessibility grant must error here, not read as "no bar".
		set probeName to name of proc
		set barCount to 0
		try
			tell proc
				set barCount to count of menu bar items of menu bar 1
			end tell
		end try
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
					-- Read raw: coercing an absent attribute `as text` yields "missing value".
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
