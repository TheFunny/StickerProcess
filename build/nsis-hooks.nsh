; StickerProcess 安装器钩子（升级防护）。
;
; 由 `Dioxus.toml` 的 `[bundle.windows.nsis] installer_hooks` 引入：dx 把本文件
; `!include` 进生成的 installer.nsi。**本文件不经过 handlebars 渲染**，所以这里
; 不能写 {{version}} —— 新版本号从安装器自身的 VERSIONINFO 读（模板已把 {{version}}
; 同时写进 FileVersion/ProductVersion）。
;
; 解决 docs/RELEASE.md「现状缺失的三块」中的两块：
;   2. 覆盖安装时若程序正在运行，NSIS 会弹「文件被占用」(Retry/Abort) —— 先结束它；
;   3. 已装版本比本安装包新时会被静默降级覆盖 —— 拦下来（交互式确认 / 静默直接拒绝）。
;
; 版本比较不手写：用 NSIS 自带 WordFunc.nsh 的 ${VersionCompare}
; （0 = 相等，1 = 第一个更新，2 = 第二个更新；字段数不同按缺位补 0，
; 故已装 "0.1.0" 与安装包 "0.1.0.0" 判定为相等）。
;
; 唯一入口是 `.onInit`：它在任何 Section 之前运行，晚于它就没意义了（文件已被覆盖）。
; 若将来 dx 的模板自己定义 `.onInit`，这里会**编译期**报重复定义（大声失败，不是静默失效）。

!include "WordFunc.nsh"
!insertmacro VersionCompare
!include "FileFunc.nsh"
!insertmacro GetFileVersion

; 主程序 exe 名与卸载注册表键（与 Dioxus.toml 的 bundle.identifier / 生成模板一致）
!define STP_EXE "StickerProcess.exe"
!define STP_UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\com.stickerprocess.app"

Function .onInit
  Call STP_StopRunning
  Call STP_CheckDowngrade
FunctionEnd

; 覆盖 exe 前先把在跑的程序结束掉。
; 检测不靠窗口标题（FindWindow 会随标题改动失效），直接用 taskkill 的退出码：
; 0 = 确实结束了进程，128 = 没找到（无事可做）。
Function STP_StopRunning
  nsExec::ExecToStack 'taskkill /IM ${STP_EXE}'
  Pop $0
  Pop $1
  StrCmp $0 "0" 0 stp_stop_done      ; 没有在跑的实例 → 完事

  Sleep 800                          ; 不带 /F 是 WM_CLOSE，等它优雅退出
  nsExec::ExecToStack 'taskkill /IM ${STP_EXE}'
  Pop $0
  Pop $1
  StrCmp $0 "0" 0 stp_stop_done

  ; 还活着（多实例，或不响应 WM_CLOSE）
  IfSilent 0 stp_stop_prompt
    nsExec::Exec 'taskkill /F /IM ${STP_EXE}'
    Goto stp_stop_done
  stp_stop_prompt:
  MessageBox MB_OKCANCEL|MB_ICONEXCLAMATION \
    "StickerProcess is still running. Click OK to close it and continue, or Cancel to stop the installation." \
    IDOK stp_stop_force
  Abort
  stp_stop_force:
  nsExec::Exec 'taskkill /F /IM ${STP_EXE}'
  Sleep 300                          ; 等文件句柄真正释放，否则紧接着的覆盖仍会失败
  stp_stop_done:
FunctionEnd

; 纯字符串比较（不碰注册表/文件/进程，单测直接抽这一个函数）。
; 入/出：$0 = 已装版本串，$1 = 新版本串 → $2 = 1 表示"已装更新"（要拦），否则 0。
Function STP_IsDowngrade
  Push $3
  ${VersionCompare} "$0" "$1" $3
  StrCmp $3 "1" 0 stp_dg_no
    StrCpy $2 1
    Goto stp_dg_out
  stp_dg_no:
    StrCpy $2 0
  stp_dg_out:
  Pop $3
FunctionEnd

; 已装版本更新时不要静默降级覆盖。
; 读不到任一版本就放行——不能为了这事把正常安装卡死。
Function STP_CheckDowngrade
  Push $0
  Push $1
  Push $2

  ReadRegStr $0 SHCTX "${STP_UNINST_KEY}" "DisplayVersion"
  StrCmp $0 "" stp_cd_out
  ${GetFileVersion} "$EXEPATH" $1
  StrCmp $1 "" stp_cd_out

  Call STP_IsDowngrade
  StrCmp $2 "1" 0 stp_cd_out

  IfSilent 0 stp_cd_prompt
    SetErrorLevel 3                  ; 静默（CI / 无人值守）：拒绝，别把旧包盖上去
    Abort
  stp_cd_prompt:
  MessageBox MB_OKCANCEL|MB_ICONEXCLAMATION \
    "StickerProcess $0 is already installed, which is newer than this package ($1).$\r$\n$\r$\nClick OK to install this older version anyway, or Cancel to keep the newer one." \
    IDOK stp_cd_allow
  Abort
  stp_cd_allow:
  stp_cd_out:
  Pop $2
  Pop $1
  Pop $0
FunctionEnd
