@echo off
chcp 65001 >nul
setlocal

set "SCRIPT_DIR=%~dp0"

REM 检查 WebView2 运行环境，缺失时自动引导下载安装
if exist "%SCRIPT_DIR%deploy\windows\install-runtime.ps1" (
    powershell -NoProfile -ExecutionPolicy Bypass -File "%SCRIPT_DIR%deploy\windows\install-runtime.ps1"
    if errorlevel 1 (
        echo.
        echo [错误] 运行环境检查未通过，请先安装 Microsoft Edge WebView2 Runtime 后重试。
        pause
        exit /b 1
    )
)

echo 正在启动 MedicalStudio...
start "" "%SCRIPT_DIR%medical-studio.exe"
endlocal
