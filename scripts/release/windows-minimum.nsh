!include "LogicLib.nsh"
!include "WinVer.nsh"

!macro NSIS_HOOK_PREINSTALL
  !if "${MAINBINARYNAME}" != "mdreader-app"
    !error "MDReader NSIS main binary must be mdreader-app"
  !endif
  !if "${PRODUCTNAME}" != "MDReader"
    !error "MDReader NSIS product name does not match"
  !endif
  !if "${BUNDLEID}" != "dev.mdreader.app"
    !error "MDReader NSIS bundle identifier does not match"
  !endif
  !tempfile MDREADER_PACKAGE_CONTRACT
  !appendfile "${MDREADER_PACKAGE_CONTRACT}" "schemaVersion=1;product=${PRODUCTNAME};identifier=${BUNDLEID};version=${VERSION};installTarget=${MAINBINARYNAME}.exe;launchTarget=${MAINBINARYNAME}.exe"
  File /oname=mdreader-package-contract "${MDREADER_PACKAGE_CONTRACT}"
  !delfile "${MDREADER_PACKAGE_CONTRACT}"
  ${IfNot} ${AtLeastWin10}
    MessageBox MB_OK|MB_ICONSTOP "MDReader requires Windows 10 or later."
    Abort
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ${IfNot} ${FileExists} "$INSTDIR\mdreader-app.exe"
    MessageBox MB_OK|MB_ICONSTOP "MDReader installation did not produce the reviewed launch target."
    Abort
  ${EndIf}
!macroend
