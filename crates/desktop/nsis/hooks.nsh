; 134 W5 (§5.5, §5.7): the Windows companion ships as the resource dir `$INSTDIR\companion\`
; (`scripts/package-windows.ps1`), so the stock template's running-app check (`${MAINBINARYNAME}.exe`
; only) and its registry cleanup (deep-link class only) need three additions. Included by Tauri's
; `bundle.windows.nsis.installerHooks` (`installer.nsi` `!include`, macros inserted around the
; Install / Uninstall sections).

; The shell's `companion::terminate()` covers a quit or an in-app update; a setup.exe run by hand
; while the Run-key-started companion is alive would otherwise hit locked DLLs under companion\.
; `taskkill` exits 128 when nothing matched; either way the result is popped and ignored.
!macro NSIS_HOOK_PREINSTALL
  nsExec::ExecToLog 'taskkill /F /IM Theuth.Companion.exe'
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::ExecToLog 'taskkill /F /IM Theuth.Companion.exe'
  Pop $0
!macroend

; D12: the companion writes `HKCU\...\Run\TheuthCompanion = "<exe>" --background` itself; nothing else
; removes it. Skipped in `/UPDATE` mode (the updater's in-place path never runs the uninstaller today, but
; the user's launch-at-login choice must survive any update that does).
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "TheuthCompanion"
  ${EndIf}
!macroend
