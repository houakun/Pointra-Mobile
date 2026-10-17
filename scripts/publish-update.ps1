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

# 更新说明：从 CHANGELOG.md 里取当前版本的章节（`## v0.3.3` 到下一个 `##` 之间）。
# 手机端「应用更新」会把它原样展开给用户看（保留换行）。
$notes = ""
$changeLog = Join-Path $root "CHANGELOG.md"
if (Test-Path $changeLog) {
  $capturing = $false
  $buffer = @()
  # 注意：PowerShell 5.1 的 Get-Content 默认按 ANSI(GBK) 读文件，
  # 而 CHANGELOG.md 是 UTF-8 —— 不显式指定就会把中文读成乱码再写进 JSON。
  foreach ($line in (Get-Content $changeLog -Encoding UTF8)) {
    if ($line -match '^##\s+v?([0-9]+\.[0-9]+\.[0-9]+)') {
      if ($capturing) { break }              # 读到下一个版本了，结束
      if ($Matches[1] -eq $version) { $capturing = $true; continue }
    }
    if ($capturing) { $buffer += $line }
  }
  $notes = ($buffer -join "`n").Trim()
}
if (-not $notes) {
  $notes = "（本次发布未写更新说明；可在 CHANGELOG.md 里补一个 $version 小节）"
}

$dest = Join-Path $DataDir "update"
New-Item -ItemType Directory -Force -Path $dest | Out-Null
Copy-Item $apkPath (Join-Path $dest "pointra.apk") -Force
$json = [ordered]@{
  version = $version; version_code = $code; size = $size; sha256 = $hash; notes = $notes
} | ConvertTo-Json
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
Write-Host ("  说明   : {0}" -f ($notes -split "`n")[0])
Write-Host ""
Write-Host "→ 电脑端通道（内网）：手机设置 → 应用更新 → 检查更新"
Write-Host ""
Write-Host "→ 不依赖电脑的通道（公网）：把下面两个文件传到 GitHub Release（tag 用 v$version），"
Write-Host "   以后手机就算电脑没开也能检查更新："
Write-Host ("     {0}" -f (Join-Path $dest "pointra.apk"))
Write-Host ("     {0}" -f (Join-Path $dest "pointra.json"))
Write-Host "   地址：https://github.com/houakun/Pointra-Mobile/releases"
