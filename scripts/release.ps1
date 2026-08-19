[CmdletBinding()]
param(
    [switch]$SkipBuild,
    [switch]$SkipInstaller
)

$ErrorActionPreference = 'Stop'
$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Set-Location $repositoryRoot

function Invoke-Checked {
    param(
        [Parameter(Mandatory, Position = 0)]
        [string]$Command,
        [Parameter(Position = 1)]
        [string[]]$Arguments
    )

    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "命令执行失败（$LASTEXITCODE）：$Command $($Arguments -join ' ')"
    }
}

$metadata = (Invoke-Checked cargo @('metadata', '--no-deps', '--format-version', '1')) | ConvertFrom-Json
$appPackage = $metadata.packages | Where-Object name -eq 'wisp-app' | Select-Object -First 1
if (-not $appPackage) {
    throw 'Cargo metadata 中未找到 wisp-app'
}

$version = $appPackage.version
$releaseExe = Join-Path $repositoryRoot 'target\release\wisp.exe'
$distRoot = Join-Path $repositoryRoot 'dist'
$portableStage = Join-Path $distRoot 'staging\Wisp'
$portableArchive = Join-Path $distRoot "Wisp-v$version-portable-win-x64.zip"
$repositoryPrefix = $repositoryRoot.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
$resolvedDistRoot = [IO.Path]::GetFullPath($distRoot)
if (-not $resolvedDistRoot.StartsWith($repositoryPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "发布目录越出仓库边界：$resolvedDistRoot"
}

if (-not $SkipBuild) {
    Invoke-Checked cargo @('build', '--release', '--locked', '-p', 'wisp-app')
}
if (-not (Test-Path -LiteralPath $releaseExe -PathType Leaf)) {
    throw "Release 可执行文件不存在：$releaseExe"
}

if (Test-Path -LiteralPath $distRoot) {
    Remove-Item -LiteralPath $distRoot -Recurse -Force
}
New-Item -ItemType Directory -Path $portableStage -Force | Out-Null

Copy-Item -LiteralPath $releaseExe -Destination (Join-Path $portableStage 'wisp.exe')
Copy-Item -LiteralPath (Join-Path $repositoryRoot 'LICENSE') -Destination $portableStage
Copy-Item -LiteralPath (Join-Path $repositoryRoot 'packaging\portable.flag') -Destination $portableStage
Copy-Item -LiteralPath (Join-Path $repositoryRoot 'packaging\README-portable.txt') `
    -Destination (Join-Path $portableStage 'README.txt')

Compress-Archive -LiteralPath $portableStage -DestinationPath $portableArchive -CompressionLevel Optimal

$artifacts = [System.Collections.Generic.List[string]]::new()
$artifacts.Add($portableArchive)

if (-not $SkipInstaller) {
    $isccCandidates = @(
        (Get-Command ISCC.exe -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source -First 1),
        (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 7\ISCC.exe'),
        (Join-Path $env:ProgramFiles 'Inno Setup 7\ISCC.exe'),
        (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 7\ISCC.exe'),
        (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'),
        (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
        (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe')
    ) | Where-Object { $_ -and (Test-Path -LiteralPath $_ -PathType Leaf) }
    $iscc = $isccCandidates | Select-Object -First 1
    if (-not $iscc) {
        throw '未找到 Inno Setup 7/6 编译器 ISCC.exe。安装后重试，或使用 -SkipInstaller 仅生成便携版。'
    }

    $installerScript = Join-Path $repositoryRoot 'packaging\wisp.iss'
    Invoke-Checked $iscc @(
        "/DMyAppVersion=$version",
        "/DMyAppExe=$releaseExe",
        "/DMyOutputDir=$distRoot",
        $installerScript
    )

    $installer = Join-Path $distRoot "Wisp-v$version-setup-win-x64.exe"
    if (-not (Test-Path -LiteralPath $installer -PathType Leaf)) {
        throw "安装包未生成：$installer"
    }
    $artifacts.Add($installer)
}

$checksums = $artifacts | ForEach-Object {
    $hash = Get-FileHash -LiteralPath $_ -Algorithm SHA256
    "{0}  {1}" -f $hash.Hash.ToLowerInvariant(), (Split-Path $_ -Leaf)
}
$checksums | Set-Content -LiteralPath (Join-Path $distRoot 'SHA256SUMS.txt') -Encoding ascii

Remove-Item -LiteralPath (Join-Path $distRoot 'staging') -Recurse -Force

Write-Host "Wisp v$version 发布产物已生成："
Get-ChildItem -LiteralPath $distRoot -File | Select-Object Name, Length, LastWriteTime
