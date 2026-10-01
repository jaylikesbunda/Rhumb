; Per-user installer for Rhumb. Build with:
;   makensis /DVERSION=0.1.0 installer/rhumb.nsi
; Run after `cargo build --release`. Pass /DEXE=<path> to use another binary.

Unicode true
!include "MUI2.nsh"

!define ROOT ".."

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef EXE
  !define EXE "${ROOT}\target\release\rhumb.exe"
!endif

Name "Rhumb"
OutFile "${ROOT}\dist\rhumb-${VERSION}-setup.exe"
InstallDir "$LOCALAPPDATA\Programs\Rhumb"
InstallDirRegKey HKCU "Software\Rhumb" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma

!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Rhumb"

!define MUI_ICON "${ROOT}\assets\icon.ico"
!define MUI_UNICON "${ROOT}\assets\icon.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\rhumb.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Start Rhumb"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${ROOT}\LICENSE"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "Rhumb"
VIAddVersionKey "FileDescription" "Rhumb installer"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"

Section "Install"
  SetOutPath "$INSTDIR"
  File "${EXE}"
  File "${ROOT}\LICENSE"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  CreateDirectory "$SMPROGRAMS\Rhumb"
  CreateShortcut "$SMPROGRAMS\Rhumb\Rhumb.lnk" "$INSTDIR\rhumb.exe"

  WriteRegStr HKCU "Software\Rhumb" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "Rhumb"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\rhumb.exe"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\rhumb.exe"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\Rhumb\Rhumb.lnk"
  RMDir "$SMPROGRAMS\Rhumb"
  DeleteRegKey HKCU "${UNINSTALL_KEY}"
  DeleteRegKey HKCU "Software\Rhumb"
SectionEnd
