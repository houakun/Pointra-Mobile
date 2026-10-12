# 把新版本 APK 发布到 PC 端同步服务的 update 目录，供手机端「检查更新」拉取。
#
# 用法（在 Pointra-Mobile 目录下）：
#   powershell -ExecutionPolicy Bypass -File scripts\publish-update.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\publish-update.ps1 -Apk <自定义路径>
#
# 发布后，手机端（已与 PC 配对）在「设置 → 应用更新」点检查更新即可看到新版本。
param(
  [string]$Apk = "src-tauri\gen\android\app\build\outputs\apk\arm64\release\app-arm64-release.apk",
  [string]$DataDir = "$env:APPDATA\com.syjun.pointra"
)
$ErrorActionPreference = "Stop"

$root = Split-Path $PSScriptRoot -Parent
$apkPath = Join-Path $root $Apk
if (-not (Test-Path $apkPath)) {
  throw "找不到 APK：$apkPath`n请先运行：bash scripts/build-apk.sh aarch64 release"
}

# 版本号单一来源 = tauri.conf.json（与 App 内 getVersion() 显示的一致）
$conf = Get-Content (Join-Path $root "src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json
$version = $conf.version
$parts = $version.Split('.')
$code = [int]$parts[0] * 1000000 + [int]$parts[1] * 1000 + [int]$parts[2]

$hash = (Get-FileHash $apkPath -Algorithm SHA256).Hash.ToLower()
$size = (Get-Item $apkPath).Length

$dest = Join-Path $DataDir "update"
New-Item -ItemType Directory -Force -Path $dest | Out-Null
Copy-Item $apkPath (Join-Path $dest "pointra.apk") -Force
$json = [ordered]@{ version = $version; version_code = $code; size = $size; sha256 = $hash } |
  ConvertTo-Json
# 必须写成「不带 BOM」的 UTF-8：PC 端用 serde_json 解析，带 BOM 会报
# "expected value at line 1 column 1"。PowerShell 5.1 的 -Encoding UTF8 会加 BOM！
[System.IO.File]::WriteAllText(
  (Join-Path $dest "pointra.json"),
  $json,
  (New-Object System.Text.UTF8Encoding($false))
)

Write-Host ("已发布 v{0}（versionCode={1}，{2} MB）" -f $version, $code, [math]::Round($size / 1MB, 1))
Write-Host ("  APK    : {0}" -f (Join-Path $dest "pointra.apk"))
Write-Host ("  元数据 : {0}" -f (Join-Path $dest "pointra.json"))
Write-Host "  手机端：「设置 → 应用更新 → 检查更新」"
