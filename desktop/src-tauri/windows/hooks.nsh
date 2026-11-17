; Puts the bundled btmux on the user's PATH: as bin\btmux.exe, a hard link,
; since the install directory also holds the app and its uninstaller.

; Drops $env:BTMUX_BIN_DIR from the user's PATH, then appends it if
; $env:BTMUX_PATH_ADD is set. Reads and writes the raw value, keeping its
; %VARIABLES% (.NET's Environment API would expand them).
!define BTMUX_PATH_SCRIPT `$$d = $$env:BTMUX_BIN_DIR; $$k = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment'); $$p = @([string]$$k.GetValue('Path', '', 'DoNotExpandEnvironmentNames') -split ';' | Where-Object { $$_ -and $$_.TrimEnd('\') -ne $$d }); if ($$env:BTMUX_PATH_ADD) { $$p += $$d }; if ($$p) { $$k.SetValue('Path', $$p -join ';', 'ExpandString') } else { $$k.DeleteValue('Path', $$false) }`

; Passed through the environment, as a path may contain quotes.
!macro BTMUX_SET_PATH ADD
  System::Call 'kernel32::SetEnvironmentVariable(t "BTMUX_BIN_DIR", t "$INSTDIR\bin")'
  System::Call 'kernel32::SetEnvironmentVariable(t "BTMUX_PATH_ADD", t "${ADD}")'
  nsExec::Exec `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "${BTMUX_PATH_SCRIPT}"`
  Pop $0
  ; HWND_BROADCAST, WM_SETTINGCHANGE: new terminals see the change.
  SendMessage 0xFFFF 0x1A 0 "STR:Environment" /TIMEOUT=5000
!macroend

!macro NSIS_HOOK_POSTINSTALL
  CreateDirectory "$INSTDIR\bin"
  Delete "$INSTDIR\bin\btmux.exe"
  System::Call 'kernel32::CreateHardLinkW(w "$INSTDIR\bin\btmux.exe", w "$INSTDIR\btmux.exe", p 0) i .r0'
  ${If} $0 = 0
    CopyFiles /SILENT "$INSTDIR\btmux.exe" "$INSTDIR\bin\btmux.exe"
  ${EndIf}
  !insertmacro BTMUX_SET_PATH 1
!macroend

; After the uninstaller's checks, which can abort it.
!macro NSIS_HOOK_POSTUNINSTALL
  Delete /REBOOTOK "$INSTDIR\bin\btmux.exe"
  RMDir /REBOOTOK "$INSTDIR\bin"
  RMDir "$INSTDIR"
  ; An update reinstalls it.
  ${If} $UpdateMode <> 1
    !insertmacro BTMUX_SET_PATH ""
  ${EndIf}
!macroend
