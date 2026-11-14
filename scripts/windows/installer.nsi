; Windows installer for SwitchHosts Lite, built by .github/workflows/release.yml.
;
; Installs for all users, so the installer and uninstaller ask for administrator
; rights. The in-app updater downloads this setup and runs it silently (/S) with
; elevation after the app exits.
; makensis is called with /DVERSION, /DAPP_SRC, /DAPP_ICON and /DOUT_FILE.

!include "MUI2.nsh"
!include "FileFunc.nsh"

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

; Icon and app paths come from the command line, relative to the repository root.
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
  ; %APPDATA%\SwitchHostsLite holds the user's hosts entries and settings.
SectionEnd
