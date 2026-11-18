; Puts the bundled btmux on the user's PATH: as bin\btmux.exe, a hard link,
; since the install directory also holds the app and its uninstaller.
; PowerShell gets paths through the environment, as they may contain quotes.

; Exits 1 if a btmux.exe of $env:BTMUX_DIR runs (by either name). Through
; WMI: the installer's 32-bit PowerShell can't read native processes' paths.
!define BTMUX_RUNNING_SCRIPT `\
  $$ErrorActionPreference = 'Stop'; \
  $$paths = @((Join-Path $$env:BTMUX_DIR 'btmux.exe'), (Join-Path $$env:BTMUX_DIR 'bin\btmux.exe')); \
  if (Get-CimInstance Win32_Process -Filter 'Name = ''btmux.exe''' | Where-Object { $$paths -contains $$_.ExecutablePath }) { exit 1 }`

; Appends $env:BTMUX_BIN_DIR to the user's PATH if $env:BTMUX_PATH_ADD is
; set and it isn't there (exiting 2 without its btmux.exe), else removes it.
; Keeps the value's type, unexpanded %VARIABLES%, and other entries as they
; are (.NET's Environment API would expand them).
!define BTMUX_PATH_SCRIPT `\
  $$ErrorActionPreference = 'Stop'; \
  $$d = $$env:BTMUX_BIN_DIR; \
  $$add = [bool]$$env:BTMUX_PATH_ADD; \
  if ($$add -and -not (Test-Path -LiteralPath (Join-Path $$d 'btmux.exe') -PathType Leaf)) { exit 2 }; \
  $$k = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment'); \
  $$raw = $$k.GetValue('Path', $$null, 'DoNotExpandEnvironmentNames'); \
  $$all = @(if ($$null -ne $$raw) { ([string]$$raw) -split ';' }); \
  $$rest = @($$all | Where-Object { $$_.TrimEnd('\') -ne $$d }); \
  $$has = $$rest.Count -lt $$all.Count; \
  if ($$add -eq $$has) { exit 0 }; \
  $$new = if ($$add) { (@(([string]$$raw).TrimEnd(';')) + $$d | Where-Object { $$_ }) -join ';' } else { $$rest -join ';' }; \
  $$kind = if ($$null -ne $$raw) { $$k.GetValueKind('Path') } else { 'ExpandString' }; \
  if ($$new) { $$k.SetValue('Path', $$new, $$kind) } else { $$k.DeleteValue('Path') }`

!macro BTMUX_POWERSHELL SCRIPT
  nsExec::ExecToLog `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "${SCRIPT}"`
  Pop $0
!macroend

; Waits for running servers of this installation, which would keep their
; files, to stop: the app's checks only find the app itself.
!macro BTMUX_CHECK_RUNNING
  System::Call 'kernel32::SetEnvironmentVariable(t "BTMUX_DIR", t "$INSTDIR")'
  ${Do}
    !insertmacro BTMUX_POWERSHELL "${BTMUX_RUNNING_SCRIPT}"
    ${If} $0 != 1
      ${Break}
    ${EndIf}
    ${If} ${Cmd} `MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "btmux is running from $INSTDIR. Quit the btmux Desktop app, stop any btmux server it kept running or that you started with the btmux command, then choose Retry." /SD IDCANCEL IDCANCEL`
      Abort "btmux is running from $INSTDIR."
    ${EndIf}
  ${Loop}
!macroend

!macro BTMUX_SET_PATH ADD
  System::Call 'kernel32::SetEnvironmentVariable(t "BTMUX_BIN_DIR", t "$INSTDIR\bin")'
  System::Call 'kernel32::SetEnvironmentVariable(t "BTMUX_PATH_ADD", t "${ADD}")'
  !insertmacro BTMUX_POWERSHELL "${BTMUX_PATH_SCRIPT}"
  ; HWND_BROADCAST, WM_SETTINGCHANGE: new terminals see the change.
  SendMessage 0xFFFF 0x1A 0 "STR:Environment" /TIMEOUT=5000
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro BTMUX_CHECK_RUNNING
!macroend

!macro NSIS_HOOK_POSTINSTALL
  CreateDirectory "$INSTDIR\bin"
  Delete "$INSTDIR\bin\btmux.exe"
  System::Call 'kernel32::CreateHardLinkW(w "$INSTDIR\bin\btmux.exe", w "$INSTDIR\btmux.exe", p 0) i .r0'
  ${If} $0 = 0
    CopyFiles /SILENT "$INSTDIR\btmux.exe" "$INSTDIR\bin\btmux.exe"
  ${EndIf}
  !insertmacro BTMUX_SET_PATH 1
  ${If} $0 != 0
    DetailPrint "Could not add $INSTDIR\bin\btmux.exe to PATH ($0)."
    MessageBox MB_OK|MB_ICONEXCLAMATION "btmux Desktop is installed, but the btmux command couldn't be added to your PATH. Run $INSTDIR\btmux.exe directly, or reinstall." /SD IDOK
    SetErrorLevel 2
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro BTMUX_CHECK_RUNNING
!macroend

; After the uninstaller's checks, which can abort it.
!macro NSIS_HOOK_POSTUNINSTALL
  Delete "$INSTDIR\bin\btmux.exe"
  RMDir "$INSTDIR\bin"
  RMDir "$INSTDIR"
  ; An update reinstalls it.
  ${If} $UpdateMode <> 1
    !insertmacro BTMUX_SET_PATH ""
    ${If} $0 != 0
      DetailPrint "Could not remove $INSTDIR\bin from PATH ($0)."
      MessageBox MB_OK|MB_ICONEXCLAMATION "Could not remove $INSTDIR\bin from your PATH. Remove it in Settings > System > About > Advanced system settings > Environment Variables." /SD IDOK
    ${EndIf}
  ${EndIf}
!macroend
