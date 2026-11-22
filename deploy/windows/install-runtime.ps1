<#
.SYNOPSIS
    检查并自动下载安装 MedicalStudio 所需的 Windows 运行环境（Microsoft Edge WebView2 Runtime）。
.DESCRIPTION
    本脚本供便携版 (.zip) 用户在全新 Windows 电脑上首次运行前执行。
    编译产物已静态链接 MSVC CRT (+crt-static)，无需单独安装 VC++ Redistributable；
    若系统缺少 WebView2 Runtime，本脚本会自动下载微软官方引导程序并完成安装。
#>
param(
    [switch]$Silent,
    [switch]$InstallVcRedist
)

$ErrorActionPreference = "Stop"

function Test-WebView2Installed {
    $clientGuid = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"
    $regPaths = @(
        "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\$clientGuid",
        "HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\$clientGuid",
        "HKCU:\Software\Microsoft\EdgeUpdate\Clients\$clientGuid"
    )
    foreach ($path in $regPaths) {
        if (Test-Path $path) {
            $pv = (Get-ItemProperty -Path $path -Name "pv" -ErrorAction SilentlyContinue).pv
            if ($pv -and $pv -ne "0.0.0.0") {
                return $pv
            }
        }
    }
    return $null
}

$wv2Version = Test-WebView2Installed
if ($wv2Version) {
    Write-Host "[OK] 已检测到 Microsoft Edge WebView2 Runtime (版本: $wv2Version)" -ForegroundColor Green
} else {
    Write-Host "[!] 未检测到 Microsoft Edge WebView2 Runtime，MedicalStudio 桌面界面依赖该组件。" -ForegroundColor Yellow

    if (-not $Silent) {
        $choice = Read-Host "是否立即自动下载并安装官方 WebView2 Runtime？(Y/n)"
        if ($choice -match "^[Nn]$") {
            Write-Host "已取消自动安装。您可手动下载离线安装包：" -ForegroundColor Yellow
            Write-Host "https://developer.microsoft.com/zh-cn/microsoft-edge/webview2/" -ForegroundColor Cyan
            exit 1
        }
    }

    $installerPath = Join-Path $env:TEMP "MicrosoftEdgeWebview2Setup.exe"
    $downloadUrl = "https://go.microsoft.com/fwlink/p/?LinkId=2124703"

    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        Write-Host "[1/2] 正在下载 Microsoft Edge WebView2 官方引导程序..." -ForegroundColor Cyan
        Invoke-WebRequest -Uri $downloadUrl -OutFile $installerPath -UseBasicParsing

        Write-Host "[2/2] 正在安装 WebView2 Runtime（请稍候）..." -ForegroundColor Cyan
        $installArgs = if ($Silent) { "/silent /install" } else { "/install" }
        $proc = Start-Process -FilePath $installerPath -ArgumentList $installArgs -Wait -PassThru
        if ($proc.ExitCode -ne 0) {
            throw "WebView2 安装程序返回退出码: $($proc.ExitCode)"
        }
        Write-Host "[OK] Microsoft Edge WebView2 Runtime 安装完成！" -ForegroundColor Green
    } catch {
        Write-Host "[ERROR] 自动下载或安装 WebView2 失败：$($_.Exception.Message)" -ForegroundColor Red
        Write-Host "如果当前电脑处于内网/离线环境，请在有网设备下载离线安装包后拷贝安装：" -ForegroundColor Yellow
        Write-Host "https://developer.microsoft.com/zh-cn/microsoft-edge/webview2/" -ForegroundColor Cyan
        exit 1
    } finally {
        if (Test-Path $installerPath) {
            Remove-Item $installerPath -Force -ErrorAction SilentlyContinue
        }
    }
}

if ($InstallVcRedist) {
    $vcInstaller = Join-Path $env:TEMP "vc_redist.x64.exe"
    $vcUrl = "https://aka.ms/vs/17/release/vc_redist.x64.exe"
    Write-Host "[*] 正在下载并安装 Visual C++ 2015-2022 Redistributable (x64)..." -ForegroundColor Cyan
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest -Uri $vcUrl -OutFile $vcInstaller -UseBasicParsing
    Start-Process -FilePath $vcInstaller -ArgumentList "/install /passive /norestart" -Wait
    Remove-Item $vcInstaller -Force -ErrorAction SilentlyContinue
    Write-Host "[OK] Visual C++ Redistributable 安装完成！" -ForegroundColor Green
}
