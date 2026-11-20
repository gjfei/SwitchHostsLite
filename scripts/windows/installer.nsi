; Windows installer for SwitchHosts Lite, built by .github/workflows/release.yml.
;
; Installs for all users, so the installer and uninstaller ask for administrator
; rights. The in-app updater downloads this setup and runs it silently (/S) with
; elevation after the app exits.
; makensis is called with /DVERSION, /DAPP_SRC, /DAPP_ICON and /DOUT_FILE.

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"

!ifndef VERSION
  !error "VERSION is not defined"
!endif
!ifndef APP_SRC
  !error "APP_SRC is not defined"
!endif
!ifndef OUT_FILE
  !error "OUT_FILE is not defined"
!endif
!ifndef APP_ICON
  !error "APP_ICON is not defined"
!endif

!define APP_NAME "SwitchHosts Lite"
!define APP_EXE "switch-hosts-lite.exe"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\SwitchHostsLite"

Name "${APP_NAME}"
OutFile "${OUT_FILE}"
InstallDir "$PROGRAMFILES64\SwitchHostsLite"
InstallDirRegKey HKLM "Software\SwitchHostsLite" "InstallDir"
RequestExecutionLevel admin
SetCompressor /SOLID lzma
Unicode true

; Icon and app paths come from the command line as absolute paths.
!define MUI_ICON "${APP_ICON}"
!define MUI_UNICON "${APP_ICON}"
!define MUI_ABORTWARNING

; No finish-page "run" box: the installer is elevated, and the app must not
; start with an administrator token.

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "SimpChinese"

LangString APP_RUNNING ${LANG_ENGLISH} "SwitchHosts Lite is running and must be closed before continuing.$\r$\n$\r$\nYes: close it for me (settings are saved on exit).$\r$\nNo: I will close it myself, then retry.$\r$\nCancel: abort."
LangString APP_RUNNING ${LANG_SIMPCHINESE} "SwitchHosts Lite 正在运行，继续前需要关闭。$\r$\n$\r$\n是：代为关闭（退出时自动保存设置）。$\r$\n否：我自己关闭，然后重试。$\r$\n取消：放弃。"
LangString APP_RUNNING_MANUAL ${LANG_ENGLISH} "SwitchHosts Lite is running. This version cannot be closed automatically; quit it from the tray icon (or Task Manager), then retry."
LangString APP_RUNNING_MANUAL ${LANG_SIMPCHINESE} "SwitchHosts Lite 正在运行。该版本无法自动关闭，请从托盘图标退出（或任务管理器结束）后重试。"

; The running app owns the exe image, so overwriting it fails while it runs.
; Newer versions own a message-only tray window that accepts a registered
; quit message (src/tray.rs); older ones must be closed by hand.
!macro EnsureAppClosed UN
  Function ${UN}AppRunning
    nsExec::ExecToStack '"$SYSDIR\tasklist.exe" /FI "IMAGENAME eq ${APP_EXE}" /NH'
    Pop $R0
    Pop $R1
    System::Call 'msvcrt::wcsstr(w $R1, w "${APP_EXE}") p .rR0'
  FunctionEnd

  Function ${UN}PostQuit
    System::Call 'user32::RegisterWindowMessageW(w "SwitchHostsLite_Quit") i .rR2'
    StrCpy $R3 0
    ${Do}
      System::Call 'user32::FindWindowExW(p -3, p $R3, w "SwitchHostsLiteTray", w "SwitchHosts Lite Tray") p .rR3'
      ${If} $R3 == 0
        ${Break}
      ${EndIf}
      System::Call 'user32::PostMessageW(p $R3, i $R2, p 0, p 0)'
    ${Loop}
  FunctionEnd

  Function ${UN}WaitExit
    StrCpy $R4 0
    ${Do}
      Sleep 500
      Call ${UN}AppRunning
      ${If} $R0 == 0
        ${Break}
      ${EndIf}
      IntOp $R4 $R4 + 1
    ${LoopUntil} $R4 >= 20
  FunctionEnd

  Function ${UN}EnsureAppClosed
    check:
    Call ${UN}AppRunning
    StrCmp $R0 0 done
    System::Call 'user32::FindWindowExW(p -3, p 0, w "SwitchHostsLiteTray", w "SwitchHosts Lite Tray") p .rR1'
    ${If} ${Silent}
      StrCmp $R1 0 wait
      Call ${UN}PostQuit
      wait:
      Call ${UN}WaitExit
      Call ${UN}AppRunning
      StrCmp $R0 0 done
      SetErrorLevel 2
      Abort
    ${EndIf}
    StrCmp $R1 0 manual
    MessageBox MB_YESNOCANCEL|MB_ICONQUESTION "$(APP_RUNNING)" IDYES quick IDNO manual IDCANCEL cancel
    quick:
    Call ${UN}PostQuit
    Call ${UN}WaitExit
    Goto check
    manual:
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "$(APP_RUNNING_MANUAL)" IDRETRY check IDCANCEL cancel
    cancel:
    Abort
    done:
  FunctionEnd
!macroend
!insertmacro EnsureAppClosed ""
!insertmacro EnsureAppClosed "un."

Function .onInit
  Call EnsureAppClosed
FunctionEnd

Function un.onInit
  Call un.EnsureAppClosed
FunctionEnd

Section "SwitchHosts Lite" SecMain
  SetShellVarContext all
  SetOutPath "$INSTDIR"
  File "${APP_SRC}"
  WriteUninstaller "$INSTDIR\Uninstall.exe"

  CreateShortcut "$SMPROGRAMS\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}"
  CreateShortcut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}"

  WriteRegStr HKLM "Software\SwitchHostsLite" "InstallDir" "$INSTDIR"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\${APP_EXE}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "Publisher" "gjfei"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKLM "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegDWORD HKLM "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINSTALL_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKLM "${UNINSTALL_KEY}" "EstimatedSize" "$0"
SectionEnd

Section "Uninstall"
  SetShellVarContext all
  Delete "$DESKTOP\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}.lnk"
  Delete "$INSTDIR\${APP_EXE}"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"

  DeleteRegKey HKLM "${UNINSTALL_KEY}"
  DeleteRegKey HKLM "Software\SwitchHostsLite"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "SwitchHostsLite"
  ; %APPDATA%\SwitchHostsLite holds the user's hosts entries and settings.
SectionEnd
