!include "LogicLib.nsh"
!include "WinVer.nsh"

!macro NSIS_HOOK_PREINSTALL
  ${IfNot} ${AtLeastWin10}
    MessageBox MB_OK|MB_ICONSTOP "MDReader requires Windows 10 or later."
    Abort
  ${EndIf}
!macroend
