; Ask for the language on every interactive install, even if one was remembered.
!ifndef MUI_LANGDLL_ALWAYSSHOW
  !define MUI_LANGDLL_ALWAYSSHOW
!endif

; Leaves the chosen language for Linger's next start. The app reads and removes this file.
!macro NSIS_HOOK_POSTINSTALL
  IfSilent linger_lang_end
  Push $R8
  Push $R9
  StrCpy $R9 "en"
  StrCmp $LANGUAGE 2052 0 +2
    StrCpy $R9 "zh-CN"
  CreateDirectory "$APPDATA\Linger"
  ClearErrors
  FileOpen $R8 "$APPDATA\Linger\install-language" w
  IfErrors linger_lang_restore
  FileWrite $R8 $R9
  FileClose $R8
  linger_lang_restore:
  Pop $R9
  Pop $R8
  linger_lang_end:
!macroend
