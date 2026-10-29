!include "WinVer.nsh"
!include "x64.nsh"

!macro NSIS_HOOK_PREINSTALL
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "WinMeter requires 64-bit Windows."
    Abort
  ${EndIf}
  ${IfNot} ${AtLeastWin10}
    MessageBox MB_ICONSTOP "WinMeter requires Windows 10 or Windows 11."
    Abort
  ${EndIf}
  ; Sensor helper targets .NET Framework 4.7.2.
  SetRegView 64
  Push $0
  ReadRegDWORD $0 HKLM "SOFTWARE\Microsoft\NET Framework Setup\NDP\v4\Full" "Release"
  ${If} $0 < 461808
    Pop $0
    MessageBox MB_ICONSTOP "WinMeter sensors require .NET Framework 4.7.2 or later. Enable or repair .NET Framework, then run this installer again."
    Abort
  ${EndIf}
  Pop $0
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "WinMeter"
!macroend
