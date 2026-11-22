<#
.SYNOPSIS
    一键在本地 Windows 打包独立可运行的 MedicalStudio 安装包（NSIS / MSI）与便携套件 (.zip)。
.DESCRIPTION
    1. 自动构建 Vue 3 前端资源 (crates/medical-studio/frontend/dist)。
    2. 启用 +crt-static 静态链接 MSVC CRT，彻底消除新电脑缺少 VCRUNTIME140.dll 的问题。
    3. 使用 Tauri 2 CLI 生成带 WebView2 官方引导程序 (embedBootstrapper) 的 NSIS (.exe) 与 MSI 安装包：
       新电脑安装时若缺少 WebView2 Runtime，安装向导会自动下载并安装。
    4. 生成便携版 medical-suite-x86_64-pc-windows-msvc.zip（附带 start-medical-studio.bat 运行环境检测脚本）。
.PARAMETER OfflineWebView2
    可选开关：若目标电脑完全断网/内网环境，可传 -OfflineWebView2（需联网下载 ~127MB 离线 WebView2 嵌入安装包）。
#>
param(
    [switch]$OfflineWebView2,
    [switch]$SkipInstaller
)

$ErrorActionPreference = "Stop"
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Set-Location $RepoRoot

Write-Host "=== [1/4] 构建 MedicalStudio Vue 3 前端 ===" -ForegroundColor Cyan
Push-Location (Join-Path $RepoRoot "crates\medical-studio\frontend")
try {
    npm ci
    npm run build
} finally {
    Pop-Location
}

# 静态链接 MSVC C 运行库，避免新电脑报缺少 VCRUNTIME140.dll
$env:RUSTFLAGS = "-C target-feature=+crt-static"
Write-Host "=== [2/4] 编译静态 CRT 服务端 (medical-server.exe) ===" -ForegroundColor Cyan
cargo build --release --bin medical-server

if (-not $SkipInstaller) {
    Write-Host "=== [3/4] 使用 Tauri CLI 构建 Windows 安装包 (NSIS / MSI) ===" -ForegroundColor Cyan
    Push-Location (Join-Path $RepoRoot "crates\medical-studio")
    try {
        $tauriArgs = @("--yes", "@tauri-apps/cli@^2", "build")
        if ($OfflineWebView2) {
            Write-Host "  -> 启用 OfflineWebView2 离线全量打包模式（无需目标电脑联网）" -ForegroundColor Yellow
            $overrideJson = '{"bundle":{"windows":{"webviewInstallMode":{"type":"offlineInstaller","silent":true}}}}'
            $tauriArgs += @("--config", $overrideJson)
        }
        npx @tauriArgs
    } finally {
        Pop-Location
    }
} else {
    Write-Host "=== [3/4] 跳过安装器打包，仅编译静态 CRT medical-studio.exe ===" -ForegroundColor Yellow
    cargo build --release --bin medical-studio
}

Write-Host "=== [4/4] 打包便携套件 (medical-suite-x86_64-pc-windows-msvc.zip) ===" -ForegroundColor Cyan
$distDir = Join-Path $RepoRoot "dist_pkg"
if (Test-Path $distDir) { Remove-Item $distDir -Recurse -Force }
New-Item -ItemType Directory -Force -Path $distDir | Out-Null

Copy-Item (Join-Path $RepoRoot "target\release\medical-server.exe") $distDir
Copy-Item (Join-Path $RepoRoot "target\release\medical-studio.exe") $distDir
Copy-Item (Join-Path $RepoRoot "deploy\windows\start-medical-studio.bat") $distDir
Copy-Item -Recurse (Join-Path $RepoRoot "migrations") $distDir
Copy-Item -Recurse (Join-Path $RepoRoot "config") $distDir
Copy-Item -Recurse (Join-Path $RepoRoot "deploy") $distDir
Copy-Item -Recurse (Join-Path $RepoRoot "k6") $distDir

$zipPath = Join-Path $RepoRoot "medical-suite-x86_64-pc-windows-msvc.zip"
Compress-Archive -Path "$distDir\*" -DestinationPath $zipPath -Force
Remove-Item $distDir -Recurse -Force

Write-Host ""
Write-Host "打包完成！产物路径：" -ForegroundColor Green
Write-Host "  - 便携套件: $zipPath" -ForegroundColor White
$bundleDir = Join-Path $RepoRoot "target\release\bundle"
if (Test-Path $bundleDir) {
    Get-ChildItem -Path $bundleDir -Recurse -Include "*.exe", "*.msi" | ForEach-Object {
        Write-Host "  - 安装包  : $($_.FullName)" -ForegroundColor White
    }
}
