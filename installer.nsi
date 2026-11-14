Unicode True

!include "MUI2.nsh"

!ifndef VERSION
  !define VERSION "0.2.0"
!endif

!define APP_NAME "FFmpeg Queue Manager"
!define APP_EXE "ffmpeg-queue-manager.exe"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\FFmpeg Queue Manager"

Name "${APP_NAME}"
OutFile "FFmpeg-Queue-Manager_${VERSION}_x64-setup.exe"
InstallDir "$LOCALAPPDATA\FFmpeg Queue Manager"
InstallDirRegKey HKCU "${UNINST_KEY}" "InstallLocation"
RequestExecutionLevel user
ShowInstDetails show
ShowUninstDetails show

!define MUI_ICON "src-tauri\icons\icon.ico"
!define MUI_UNICON "src-tauri\icons\icon.ico"
!define MUI_ABORTWARNING

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES

!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "Launch FFmpeg Queue Manager"
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Section "Install"
  ; Terminate any running instance so binaries can be overwritten in place
  nsExec::ExecToLog 'taskkill /F /IM "${APP_EXE}"'
  Pop $0
  Sleep 500

  SetOutPath "$INSTDIR"
  SetOverwrite on

  File "dist-release\ffmpeg-queue-manager.exe"
  File "dist-release\ffmpeg.exe"
  File "dist-release\ffprobe.exe"

  ; Preserve any existing user profiles, queue state, and settings
  CreateDirectory "$INSTDIR\Profiles"

  WriteUninstaller "$INSTDIR\uninstall.exe"

  ; Shortcuts
  CreateDirectory "$SMPROGRAMS\FFmpeg Queue Manager"
  CreateShortCut "$SMPROGRAMS\FFmpeg Queue Manager\FFmpeg Queue Manager.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\${APP_EXE}" 0
  CreateShortCut "$SMPROGRAMS\FFmpeg Queue Manager\Uninstall.lnk" "$INSTDIR\uninstall.exe" "" "$INSTDIR\uninstall.exe" 0
  CreateShortCut "$SMPROGRAMS\FFmpeg Queue Manager.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\${APP_EXE}" 0
  CreateShortCut "$DESKTOP\FFmpeg Queue Manager.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\${APP_EXE}" 0

  ; Uninstall registry metadata (updating in place & cleaning legacy Tauri ID if present)
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "Anthony"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\${APP_EXE},0"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1

  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\com.ffmpegqueue.manager"
SectionEnd

Section "Uninstall"
  nsExec::ExecToLog 'taskkill /F /IM "${APP_EXE}"'
  Pop $0
  Sleep 500

  Delete "$INSTDIR\ffmpeg-queue-manager.exe"
  Delete "$INSTDIR\ffmpeg.exe"
  Delete "$INSTDIR\ffprobe.exe"
  Delete "$INSTDIR\uninstall.exe"

  Delete "$SMPROGRAMS\FFmpeg Queue Manager\FFmpeg Queue Manager.lnk"
  Delete "$SMPROGRAMS\FFmpeg Queue Manager\Uninstall.lnk"
  RMDir "$SMPROGRAMS\FFmpeg Queue Manager"
  Delete "$SMPROGRAMS\FFmpeg Queue Manager.lnk"
  Delete "$DESKTOP\FFmpeg Queue Manager.lnk"

  DeleteRegKey HKCU "${UNINST_KEY}"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\com.ffmpegqueue.manager"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${APP_NAME}"

  ; Only removes $INSTDIR if empty so user Profiles and settings are not lost unexpectedly
  RMDir "$INSTDIR"
SectionEnd
