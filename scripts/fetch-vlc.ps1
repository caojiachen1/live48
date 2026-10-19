# fetch-vlc.ps1 — 下载 VLC 3.0.x redistributable DLL 到 <repo>/vlc/
#
# 用法: 在项目根目录运行  pwsh scripts/fetch-vlc.ps1
#
# 效果: 将 libvlc.dll + libvlccore.dll + plugins/ 放入 vlc/
#       src/vlc.rs 会优先在这个目录里找运行时

$VlcDir = Join-Path $PSScriptRoot "..\vlc"
$Version = "3.0.21"
$Arch = "win64"
$Url = "https://download.videolan.org/pub/videolan/vlc/$Version/$Arch/vlc-$Version-$Arch.7z"
$Archive = Join-Path $env:TEMP "vlc-$Version-$Arch.7z"

Write-Host "==> 下载 VLC $Version ($Arch) 便携版 ..." -ForegroundColor Cyan
Write-Host "    来源: $Url"

# 需要 7-Zip 解压
$7z = Get-Command 7z -ErrorAction SilentlyContinue
if (-not $7z) {
    # 尝试默认路径
    $7zPaths = @(
        "${env:ProgramFiles}\7-Zip\7z.exe",
        "${env:ProgramFiles(x86)}\7-Zip\7z.exe"
    )
    foreach ($p in $7zPaths) {
        if (Test-Path $p) { $7z = $p; break }
    }
}
if (-not $7z) {
    Write-Host "!! 需要 7-Zip 来解压。请安装 https://7-zip.org/" -ForegroundColor Red
    exit 1
}

# 下载
try {
    $wc = New-Object System.Net.WebClient
    Write-Host "    下载中，请稍候 ..."
    $wc.DownloadFile($Url, $Archive)
    Write-Host "    下载完成: $Archive" -ForegroundColor Green
} catch {
    Write-Host "!! 下载失败: $_" -ForegroundColor Red
    exit 1
}

# 创建目标目录
if (Test-Path $VlcDir) { Remove-Item -Recurse -Force $VlcDir }
New-Item -ItemType Directory -Path $VlcDir -Force | Out-Null

# 仅提取需要的文件
Write-Host "==> 提取 libvlc.dll + libvlccore.dll + plugins/ ..." -ForegroundColor Cyan
$7zExe = if ($7z -is [System.Management.Automation.CommandInfo]) { $7z.Source } else { $7z }

# 解压所需的核心 DLL
& $7zExe e -y -o"$VlcDir" "$Archive" "vlc-$Version\libvlc.dll" "vlc-$Version\libvlccore.dll" 2>&1 | Out-Null
if ($LASTEXITCODE -ne 0) {
    Write-Host "!! 解压 libvlc DLL 失败" -ForegroundColor Red
    exit 1
}

# 解压特定的 plugins 目录（修复了 stream_out 目录名，并加上了 access_output）
$PluginDirs = @(
    "vlc-$Version\plugins\access",
    "vlc-$Version\plugins\access_output",
    "vlc-$Version\plugins\audio_output",
    "vlc-$Version\plugins\codec",
    "vlc-$Version\plugins\demux",
    "vlc-$Version\plugins\mux",
    "vlc-$Version\plugins\packetizer",
    "vlc-$Version\plugins\stream_out",
    "vlc-$Version\plugins\video_chroma",
    "vlc-$Version\plugins\video_output"
)

foreach ($dir in $PluginDirs) {
    & $7zExe x -y -o"$VlcDir" "$Archive" "$dir\*" 2>&1 | Out-Null
}

# 移动 plugins 到正确位置
Move-Item -Path (Join-Path $VlcDir "vlc-$Version\plugins") -Destination $VlcDir -Force
Remove-Item -Recurse -Force (Join-Path $VlcDir "vlc-$Version") -ErrorAction SilentlyContinue

# 清理
Remove-Item $Archive -Force -ErrorAction SilentlyContinue

Write-Host ""
Write-Host "========================================" -ForegroundColor Green
Write-Host "  VLC $Version DLL 已放入:" -ForegroundColor Green
Write-Host "    $VlcDir" -ForegroundColor White
Write-Host "  总共 $(Get-ChildItem -Recurse $VlcDir | Measure-Object | %{$_.Count}) 个文件" -ForegroundColor Green
Write-Host "========================================" -ForegroundColor Green
