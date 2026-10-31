; Tauri NSIS installer hooks: register/unregister the Explorer context menu.
; nsExec::Exec runs the exe without a console window.
!macro NSIS_HOOK_PREINSTALL
!macroend

!macro NSIS_HOOK_POSTINSTALL
  nsExec::Exec '"$INSTDIR\hasami.exe" register'
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::Exec '"$INSTDIR\hasami.exe" unregister'
  Pop $0
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
!macroend
