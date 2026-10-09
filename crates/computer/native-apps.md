---
name: native-apps
description: Drive the user's apps by script instead of by clicking — `applescript` and `shortcuts` on a Mac, `powershell` on Windows — with recipes for Reminders, Notes, Calendar, Messages, Mail, Music, Finder, Shortcuts, Outlook, Explorer and Windows Settings, the verify step for each, and the safety rules.
---

# native-apps: script the apps, then read back

`applescript(script, timeout_secs?, purpose?)`, `shortcuts(name, input?, purpose?)` (Mac) and `powershell(script, timeout_secs?, purpose?)`
(Windows) exec the system interpreter directly with your script as one argument — no shell in between. Each returns a dict
`{ok, stdout, stderr, exit_code, ms}`; the text form is the script's result, then a trailer `exit N · T s`. Default timeout 30 s, max 120.

## How to work
1. **One line back.** End every AppleScript with `return …` and every PowerShell script with one `Write-Output`; quote that line in your answer.
2. **Verify with a second call** that reads the thing back (the reminder exists, the event is in the calendar, the note has the body).
3. **Quote user text.** In AppleScript, text goes in `"…"` with inner `"` written as `\"`; in PowerShell use single quotes `'…'` and double `''` inside.
   Build the script in Python with the quoting done (`json.dumps(s)` yields a valid AppleScript string literal).
4. **Never** splice user text into `do shell script` or `Invoke-Expression`; use the app's dictionary or `shell` with an argument list instead.
5. **Confirm before deleting or sending** (Messages, Mail send, `delete`, `Remove-Item`) unless the user's words asked for exactly that; name the target in `purpose`.
6. First use of an app asks the user for Automation permission once (System Settings › Privacy & Security › Automation); a `-1743` error means it was denied — say so.
7. Dates: build them in the script (`current date` arithmetic or `date "…"`) in the user's local time; never hard-code a time zone.

## macOS — AppleScript recipes
**Reminders — new reminder in a list, due at a time**
```applescript
tell application "Reminders"
  set theList to list "Reminders" -- or the user's list name
  set dueDate to (current date)
  set hours of dueDate to 18
  set minutes of dueDate to 0
  set seconds of dueDate to 0
  set r to make new reminder at end of theList with properties {name:"Review shots", due date:dueDate}
  return name of r & " · " & (due date of r as string)
end tell
```
Verify: `tell application "Reminders" to return name of every reminder of list "Reminders" whose completed is false`. Pitfall: `list "X"` must exist —
list them first with `return name of every list`; `remind me date` is the alert, `due date` the date shown.

**Notes — new note in a folder; append to a note by name**
```applescript
tell application "Notes"
  tell account "iCloud" to set n to make new note at folder "Notes" with properties {name:"Shopping", body:"<h1>Shopping</h1><div>milk</div>"}
  return name of n
end tell
```
Append: `tell application "Notes" to set body of note "Shopping" to (body of note "Shopping") & "<div>eggs</div>"`. Verify: `return body of note "Shopping"`.
Pitfall: `body` is HTML; the first line becomes the title. Search: `return name of every note whose name contains "Shop"`.

**Calendar — new event with start/end and an alarm**
```applescript
tell application "Calendar"
  set startDate to (current date) + 1 * days
  set hours of startDate to 14
  set minutes of startDate to 0
  set seconds of startDate to 0
  set endDate to startDate + 30 * minutes
  tell calendar "Home"
    set e to make new event with properties {summary:"Dentist", start date:startDate, end date:endDate}
    tell e to make new display alarm at end with properties {trigger interval:-15}
  end tell
  return summary of e & " · " & (start date of e as string)
end tell
```
Verify: `tell application "Calendar" to return summary of every event of calendar "Home" whose start date > (current date)`. Pitfall: list calendars first
(`return name of every calendar`); Calendar may take a second to show the event — re-read, do not re-create.

**Messages — send to a contact** (destructive: it sends; confirm first)
```applescript
tell application "Messages"
  set svc to 1st account whose service type = iMessage
  send "On my way" to participant "+15551234567" of svc
  return "sent"
end tell
```
Pitfall: use a phone number or the exact Apple ID; a name is not resolved. There is no read-back — tell the user it was sent.

**Mail — draft, or send**
```applescript
tell application "Mail"
  set m to make new outgoing message with properties {subject:"Notes from today", content:"Hi —" & return & return & "…", visible:true}
  tell m to make new to recipient at end of to recipients with properties {address:"ada@example.com"}
  return "draft open: " & subject of m
end tell
```
Send = add `send m` before the return — only when the user asked to send. Search: `return subject of (messages of inbox whose subject contains "invoice")`.
Pitfall: `visible:true` shows the draft so the user can edit; without it the draft is invisible until sent.

**Music** — `tell application "Music" to playpause`; `tell application "Music" to play playlist "Focus"`; now playing:
`tell application "Music" to return name of current track & " — " & artist of current track`. Pitfall: `play playlist "X"` needs the exact playlist name.

**Finder / System Events** — reveal a file: `tell application "Finder" to reveal POSIX file "/Users/me/Desktop/shot.png"` then `activate`;
open a folder: `tell application "Finder" to open POSIX file "/Users/me/Desktop/Shots"`; clipboard: `set the clipboard to "text"`;
front app: `tell application "System Events" to return name of first process whose frontmost is true`. Launch an app with `computer_open(app=…)`.

## macOS — Shortcuts
`shortcuts(name="__list__")` lists the user's shortcuts (read-only). Run one: `shortcuts(name="Add to Reading List", input="https://…")`
— `input` arrives as the shortcut's input text, its output comes back as `stdout`. A shortcut that shows UI waits for the user; keep `timeout_secs` ≤ 60.

## Windows — PowerShell recipes
**Outlook mail draft / send** (COM; Outlook must be installed)
```powershell
$o = New-Object -ComObject Outlook.Application
$m = $o.CreateItem(0); $m.To = 'ada@example.com'; $m.Subject = 'Notes from today'; $m.Body = 'Hi'
$m.Display()   # or $m.Send() — only when asked
Write-Output "draft open: $($m.Subject)"
```
**Outlook appointment**: `$a = $o.CreateItem(1); $a.Subject='Dentist'; $a.Start=(Get-Date).Date.AddDays(1).AddHours(14); $a.Duration=30; $a.ReminderMinutesBeforeStart=15; $a.Save(); Write-Output $a.Start`.
**Outlook task**: `$t = $o.CreateItem(3); $t.Subject='Review shots'; $t.DueDate=(Get-Date).Date.AddHours(18); $t.Save()`.
Verify: `$o.GetNamespace('MAPI').GetDefaultFolder(9).Items | Where-Object Start -gt (Get-Date) | Select-Object -First 5 Subject,Start`.
**Windows Settings**: `Start-Process ms-settings:bluetooth`. **Explorer**: `Start-Process explorer.exe -ArgumentList '/select,"C:\Users\me\Desktop\shot.png"'`.
**Notepad**: `Start-Process notepad.exe 'C:\path\notes.txt'`. **Toast** (if BurntToast is installed): `if (Get-Module -ListAvailable BurntToast) { New-BurntToastNotification -Text 'Done' }`.
Pitfalls: COM objects need Outlook signed in; `-NonInteractive` means a prompt fails instead of waiting — pass everything as arguments.

## Safety rules
- No `do shell script` / `Invoke-Expression` with user text; no `keystroke` of user text into unknown windows.
- Confirm before `delete`, `Remove-Item`, `send`, `Stop-Process`; never `Format-*`, `shutdown`, `Remove-Item -Recurse` from a spoken command.
- Create first, read back second, then report the one ✓/✗ line with what changed and where.
