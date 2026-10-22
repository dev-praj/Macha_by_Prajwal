$ErrorActionPreference = "Stop"

function Find-MachaUninstaller {
    $roots = @(
        "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall",
        "HKCU:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"
    )
    $uninstallEntries = @(
        foreach ($root in $roots) {
            if (-not (Test-Path -LiteralPath $root)) {
                continue
            }
            foreach ($key in Get-ChildItem -LiteralPath $root) {
                $entry = Get-ItemProperty -LiteralPath $key.PSPath
                if ($entry.DisplayName -match '^Macha(?:\s+[\d.]+)?$' -and $entry.UninstallString) {
                    [pscustomobject]@{
                        DisplayName     = $entry.DisplayName
                        DisplayVersion = $entry.DisplayVersion
                        UninstallString = [string]$entry.UninstallString
                    }
                }
            }
        }
    )

    if ($uninstallEntries.Count -gt 1) {
        throw "More than one Macha uninstall entry was found. No changes were made."
    }
    if ($uninstallEntries.Count -eq 0) {
        throw "The Macha uninstaller was not found in this Windows account. No changes were made."
    }
    return $uninstallEntries[0]
}

function Remove-MachaStartupTask {
    if (-not (Get-Command Get-ScheduledTask -ErrorAction SilentlyContinue)) {
        throw "Could not inspect the Macha startup task because the ScheduledTasks module is unavailable."
    }
    try {
        $task = Get-ScheduledTask -TaskName "Macha Auto Start" -TaskPath "\Macha\" -ErrorAction Stop
    } catch {
        Write-Warning "Could not inspect the Macha startup task: $($_.Exception.Message)"
        return
    }
    if ($null -ne $task) {
        Unregister-ScheduledTask -TaskName "Macha Auto Start" -TaskPath "\Macha\" -Confirm:$false
        Write-Host "Removed the Macha Auto Start task."
    }
}

function Assert-MachaDataDirectory([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) {
        return
    }

    $directory = Get-Item -LiteralPath $Path -Force
    if (-not $directory.PSIsContainer -or ($directory.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw "Refusing to remove a non-directory or linked path: $Path"
    }
    $linkedItems = @(
        Get-ChildItem -LiteralPath $Path -Force -Recurse |
            Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint }
    )
    if ($linkedItems.Count -gt 0) {
        throw "Refusing to remove $Path because it contains a junction or symbolic link."
    }
}

function Remove-MachaDataDirectory([string]$Path) {
    Assert-MachaDataDirectory $Path
    if (-not (Test-Path -LiteralPath $Path)) {
        Write-Host "Not present: $Path"
        return
    }
    Remove-Item -LiteralPath $Path -Recurse -Force
    Write-Host "Removed: $Path"
}

Write-Host ""
Write-Host "Macha uninstaller"
Write-Host "Choose what to remove:"
Write-Host "  1. Uninstall Macha and remove its startup task; preserve settings and remaining local data."
Write-Host "  2. Uninstall Macha, remove its startup task, and delete its Macha-specific settings, cache, logs, and inbox."
Write-Host "  Q. Cancel"
Write-Host ""
Write-Host "Both choices leave Windows Credential Manager entries and Claude Code settings untouched."
$choice = (Read-Host "Enter 1, 2, or Q").Trim().ToUpperInvariant()
if ($choice -notin @("1", "2")) {
    Write-Host "Cancelled. No changes were made."
    exit 0
}

$uninstaller = Find-MachaUninstaller
$appData = if ($env:APPDATA) { Join-Path $env:APPDATA "Macha" } else { $null }
$localAppData = if ($env:LOCALAPPDATA) { Join-Path $env:LOCALAPPDATA "Macha" } else { $null }
if ($choice -eq "2" -and (-not $appData -or -not $localAppData)) {
    throw "The current user's APPDATA paths are unavailable. No changes were made."
}
if ($choice -eq "2") {
    Assert-MachaDataDirectory $appData
    Assert-MachaDataDirectory $localAppData
}

Write-Host ""
Write-Host "The registered Macha uninstaller will run interactively:"
Write-Host "  $($uninstaller.UninstallString)"
Write-Host 'The task "\Macha\Macha Auto Start" will be removed if it exists.'
if ($choice -eq "2") {
    Write-Host "The following additional directories will be deleted:"
    Write-Host "  $appData"
    Write-Host "  $localAppData"
    Write-Host "This permanently deletes Macha preferences, stored local files, logs, and inbox contents."
}
Write-Host ""
$confirmation = Read-Host 'Type UNINSTALL to continue, or anything else to cancel'
if ($confirmation -cne "UNINSTALL") {
    Write-Host "Cancelled. No changes were made."
    exit 0
}
if ($choice -eq "2") {
    $dataConfirmation = Read-Host 'Type DELETE DATA to also remove the two Macha data directories'
    if ($dataConfirmation -cne "DELETE DATA") {
        Write-Host "Cancelled. No changes were made."
        exit 0
    }
}

$match = [regex]::Match(
    $uninstaller.UninstallString,
    '^\s*"(?<exe>[^"]+)"'
)
if (-not $match.Success) {
    $match = [regex]::Match(
        $uninstaller.UninstallString,
        '^\s*(?<exe>.*?\.exe)(?:\s|$)',
        [System.Text.RegularExpressions.RegexOptions]::IgnoreCase
    )
}
if (
    -not $match.Success -or
    [IO.Path]::GetFileName($match.Groups["exe"].Value) -notmatch '^(?:Uninstall Macha(?: [\d.]+)?|uninstall)\.exe$' -or
    -not (Test-Path -LiteralPath $match.Groups["exe"].Value)
) {
    throw "Could not safely resolve the registered Macha uninstaller. No changes were made."
}

$process = Start-Process -FilePath $match.Groups["exe"].Value -PassThru -Wait
if ($process.ExitCode -ne 0) {
    throw "The Macha uninstaller exited with code $($process.ExitCode). Data cleanup was not started."
}

Remove-MachaStartupTask
if ($choice -eq "2") {
    Remove-MachaDataDirectory $appData
    Remove-MachaDataDirectory $localAppData
}

Write-Host ""
Write-Host "Macha uninstall steps completed."
Write-Host "Review the retained Windows Credential Manager entries and Claude Code settings manually if you want to remove Macha-specific entries there."
