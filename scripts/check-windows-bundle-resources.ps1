# SPDX-License-Identifier: Apache-2.0

[CmdletBinding()]
param(
    [string]$BundleDirectory = (
        Join-Path $PSScriptRoot "..\src-tauri\target\release\bundle"
    )
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$expectedResources = @(
    "LICENSE",
    "NOTICE",
    "THIRD_PARTY_LICENSES.txt",
    "THIRD_PARTY_LICENSES_RUST.txt"
)

if (-not (Test-Path -LiteralPath $BundleDirectory -PathType Container)) {
    throw "Windows bundle directory does not exist: $BundleDirectory"
}

$resolvedBundleDirectory = (Resolve-Path -LiteralPath $BundleDirectory).Path

function Get-OnlyBundleFile {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Filter,
        [Parameter(Mandatory = $true)]
        [string]$Label
    )

    $files = @(
        Get-ChildItem -LiteralPath $resolvedBundleDirectory -Recurse -File -Filter $Filter
    )
    if ($files.Count -ne 1) {
        throw "Expected exactly one $Label bundle, found $($files.Count)"
    }
    return $files[0]
}

function Assert-ExpectedResources {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Label,
        [Parameter(Mandatory = $true)]
        [Collections.Generic.HashSet[string]]$Names
    )

    $missing = @($expectedResources | Where-Object { -not $Names.Contains($_) })
    if ($missing.Count -gt 0) {
        throw "$Label is missing required legal resources: $($missing -join ', ')"
    }
}

$msi = Get-OnlyBundleFile -Filter "*.msi" -Label "MSI"
$msiNames = [Collections.Generic.HashSet[string]]::new(
    [StringComparer]::OrdinalIgnoreCase
)
$msiSizes = @{}
$installer = New-Object -ComObject WindowsInstaller.Installer
$database = $installer.GetType().InvokeMember(
    "OpenDatabase",
    [Reflection.BindingFlags]::InvokeMethod,
    $null,
    $installer,
    @($msi.FullName, 0)
)
$view = $database.GetType().InvokeMember(
    "OpenView",
    [Reflection.BindingFlags]::InvokeMethod,
    $null,
    $database,
    @("SELECT ``FileName``, ``FileSize`` FROM ``File``")
)
$view.GetType().InvokeMember(
    "Execute",
    [Reflection.BindingFlags]::InvokeMethod,
    $null,
    $view,
    $null
) | Out-Null

do {
    $record = $view.GetType().InvokeMember(
        "Fetch",
        [Reflection.BindingFlags]::InvokeMethod,
        $null,
        $view,
        $null
    )
    if ($null -ne $record) {
        $fileName = $record.GetType().InvokeMember(
            "StringData",
            [Reflection.BindingFlags]::GetProperty,
            $null,
            $record,
            1
        )
        $fileSize = $record.GetType().InvokeMember(
            "IntegerData",
            [Reflection.BindingFlags]::GetProperty,
            $null,
            $record,
            2
        )
        foreach ($name in ($fileName -split "\|")) {
            [void]$msiNames.Add($name)
            $msiSizes[$name] = $fileSize
        }
    }
} while ($null -ne $record)

Assert-ExpectedResources -Label "MSI" -Names $msiNames
foreach ($resource in $expectedResources) {
    if ($msiSizes[$resource] -le 0) {
        throw "MSI legal resource is empty: $resource"
    }
}

$nsis = Get-OnlyBundleFile -Filter "*.exe" -Label "NSIS"
$sevenZipCommand = Get-Command "7z.exe" -ErrorAction SilentlyContinue
$sevenZip = if ($sevenZipCommand) {
    $sevenZipCommand.Source
} else {
    Join-Path $env:ProgramFiles "7-Zip\7z.exe"
}
if (-not (Test-Path -LiteralPath $sevenZip -PathType Leaf)) {
    throw "7-Zip is required to inspect the NSIS bundle"
}

$sevenZipOutput = & $sevenZip l -slt $nsis.FullName 2>&1
if ($LASTEXITCODE -ne 0) {
    throw "7-Zip could not inspect the NSIS bundle"
}
$nsisNames = [Collections.Generic.HashSet[string]]::new(
    [StringComparer]::OrdinalIgnoreCase
)
$nsisSizes = @{}
$currentPath = $null
foreach ($line in $sevenZipOutput) {
    if ($line -match "^Path = (.+)$") {
        $currentPath = $Matches[1]
        [void]$nsisNames.Add($currentPath)
    } elseif ($null -ne $currentPath -and $line -match "^Size = (\d+)$") {
        $nsisSizes[$currentPath] = [long]$Matches[1]
    }
}

Assert-ExpectedResources -Label "NSIS" -Names $nsisNames
foreach ($resource in $expectedResources) {
    if ($nsisSizes[$resource] -le 0) {
        throw "NSIS legal resource is empty: $resource"
    }
}

Write-Output "Windows MSI and NSIS bundles contain all required legal resources."
