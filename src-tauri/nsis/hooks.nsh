; Tauri NSIS installer hooks: register/unregister the Explorer context menu.
; nsExec::Exec runs the exe without a console window.
!macro NSIS_HOOK_PREINSTALL
!macroend

!macro NSIS_HOOK_POSTINSTALL
  nsExec::Exec '"$INSTDIR\hasami.exe" register'
  ; $0 is the exit code, or "error" if it couldn't start.
  Pop $0
  DetailPrint "Hasami register exit code: $0"
  StrCmp $0 "0" +2
    MessageBox MB_OK|MB_ICONEXCLAMATION "Hasami couldn't add its right-click menu. Run the installer again." /SD IDOK
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::Exec '"$INSTDIR\hasami.exe" unregister'
  Pop $0
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
!macroend
