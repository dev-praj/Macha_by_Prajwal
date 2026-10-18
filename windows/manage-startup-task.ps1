# Install or manage Coucou's per-user Windows logon task.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("Install", "Enable", "Disable", "Remove")]
    [string]$Action
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$appPath = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "target\release\coucou.exe"))
$runnerPath = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "start-app.ps1"))
$settingsPath = Join-Path $env:APPDATA "Coucou\settings.json"
$logDirectory = Join-Path $env:LOCALAPPDATA "Coucou"
$logPath = Join-Path $logDirectory "startup.log"
$statePath = Join-Path $logDirectory "startup-task-state.json"
$runKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
$runName = "Coucou"
$taskName = "Coucou Auto Start"
$taskFolder = "\Coucou\"
$powerShellPath = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
$currentUser = [System.Security.Principal.WindowsIdentity]::GetCurrent().Name

New-Item -ItemType Directory -Path $logDirectory -Force | Out-Null

function Write-StartupLog {
    param([Parameter(Mandatory = $true)][string]$Message)

    $timestamp = Get-Date -Format "yyyy-MM-dd HH:mm:ss.fff"
    Add-Content -LiteralPath $logPath -Value "$timestamp $Message" -Encoding UTF8
}

function Get-RunValue {
    $values = Get-ItemProperty -Path $runKey -ErrorAction SilentlyContinue
    if ($null -eq $values) {
        return $null
    }

    $property = $values.PSObject.Properties[$runName]
    if ($null -eq $property) {
        return $null
    }
    return [string]$property.Value
}

function Set-AutostartPreference {
    param([Parameter(Mandatory = $true)][bool]$Enabled)

    if (-not (Test-Path -LiteralPath $settingsPath -PathType Leaf)) {
        throw "Coucou settings were not found at '$settingsPath'. Launch Coucou once to create its settings before installing the task."
    }

    $settings = [System.IO.File]::ReadAllText($settingsPath)
    $regex = [regex]::new('(?im)(^\s*"autostart"\s*:\s*)(true|false)')
    $match = $regex.Match($settings)
    if (-not $match.Success) {
        throw "Could not find the app-managed autostart preference in '$settingsPath'; no setting was changed."
    }

    $desired = $Enabled.ToString().ToLowerInvariant()
    if ($match.Groups[2].Value -ine $desired) {
        $updated = $regex.Replace($settings, ('$1' + $desired), 1)
        [System.IO.File]::WriteAllText(
            $settingsPath,
            $updated,
            [System.Text.UTF8Encoding]::new($false)
        )
    }
}

function Get-AppRunExecutable {
    param([AllowNull()][string]$CommandLine)

    if ([string]::IsNullOrWhiteSpace($CommandLine)) {
        return $null
    }

    $trimmed = $CommandLine.Trim()
    if ($trimmed -match '^"([^"]+)"') {
        return $Matches[1]
    }
    return ($trimmed -split '\s+', 2)[0]
}

function Restore-PreviousStartup {
    param($State)

    if ($State.settingsAutostart -ne $null) {
        Set-AutostartPreference -Enabled ([bool]$State.settingsAutostart)
    }

    $currentRunValue = Get-RunValue
    if ([bool]$State.registryValueExisted) {
        if ($null -eq $currentRunValue) {
            New-Item -Path $runKey -Force | Out-Null
            Set-ItemProperty -Path $runKey -Name $runName -Value ([string]$State.registryCommand)
            Write-StartupLog "Restored the previous Coucou logon registration."
        }
        elseif ($currentRunValue -cne [string]$State.registryCommand) {
            Write-StartupLog "WARNING: A different '$runName' logon value now exists; it was left unchanged."
        }
    }
    elseif ($null -ne $currentRunValue) {
        $currentExe = Get-AppRunExecutable -CommandLine $currentRunValue
        if ($currentExe -and [System.IO.Path]::GetFullPath($currentExe) -match '(?i)\\windows\\target\\(debug|release)\\coucou\.exe$') {
            Remove-ItemProperty -Path $runKey -Name $runName
            Write-StartupLog "Removed the app-managed logon registration created after installation."
        }
    }
}

function Install-StartupTask {
    if (-not (Test-Path -LiteralPath $appPath -PathType Leaf)) {
        throw "Release executable not found: $appPath. Build the release app before installing startup."
    }
    if (-not (Test-Path -LiteralPath $runnerPath -PathType Leaf)) {
        throw "Startup script not found: $runnerPath."
    }
    if (-not (Test-Path -LiteralPath $powerShellPath -PathType Leaf)) {
        throw "Windows PowerShell was not found at the expected path: $powerShellPath."
    }
    if ((Get-CoucouProcesses).Count -gt 0) {
        throw "Close Coucou before installing or replacing its logon registration, then run this command again."
    }

    $existingTask = Get-ScheduledTask -TaskName $taskName -TaskPath $taskFolder -ErrorAction SilentlyContinue
    $stateExists = Test-Path -LiteralPath $statePath -PathType Leaf
    if ($null -ne $existingTask -and -not $stateExists) {
        throw "A task named '$taskFolder$taskName' exists without the installer's state record. It was left unchanged."
    }

    if ($stateExists) {
        $state = Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json
    }
    else {
        if (-not (Test-Path -LiteralPath $settingsPath -PathType Leaf)) {
            throw "Coucou settings were not found at '$settingsPath'. Launch Coucou once to create its settings before installing the task."
        }
        $settings = [System.IO.File]::ReadAllText($settingsPath) | ConvertFrom-Json
        $autostartProperty = $settings.PSObject.Properties["autostart"]
        if ($null -eq $autostartProperty) {
            throw "Could not find the app-managed autostart preference in '$settingsPath'; no setting was changed."
        }

        $previousRunValue = Get-RunValue
        if ($null -ne $previousRunValue) {
            $previousExe = Get-AppRunExecutable -CommandLine $previousRunValue
            $expectedExecutables = @(
                [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "target\debug\coucou.exe")),
                $appPath
            )
            if (-not $previousExe -or $expectedExecutables -notcontains [System.IO.Path]::GetFullPath($previousExe)) {
                throw "The existing '$runName' logon registration does not point to a known Coucou build. It was left unchanged."
            }
        }

        $state = [pscustomobject]@{
            settingsAutostart = [bool]$autostartProperty.Value
            registryValueExisted = ($null -ne $previousRunValue)
            registryCommand = $previousRunValue
        }
        $state | ConvertTo-Json | Set-Content -LiteralPath $statePath -Encoding UTF8
    }

    $trigger = New-ScheduledTaskTrigger `
        -AtLogOn `
        -User $currentUser
    $arguments = "-NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$runnerPath`" -StartupDelaySeconds 20"
    $taskAction = New-ScheduledTaskAction `
        -Execute $powerShellPath `
        -Argument $arguments `
        -WorkingDirectory $repoRoot
    $principal = New-ScheduledTaskPrincipal `
        -UserId $currentUser `
        -LogonType Interactive `
        -RunLevel Limited
    $taskSettings = New-ScheduledTaskSettingsSet `
        -StartWhenAvailable `
        -AllowStartIfOnBatteries `
        -DontStopIfGoingOnBatteries `
        -ExecutionTimeLimit (New-TimeSpan -Minutes 15) `
        -RestartCount 3 `
        -RestartInterval (New-TimeSpan -Seconds 10) `
        -MultipleInstances IgnoreNew

    try {
        Register-ScheduledTask `
            -TaskName $taskName `
            -TaskPath $taskFolder `
            -Action $taskAction `
            -Trigger $trigger `
            -Settings $taskSettings `
            -Principal $principal `
            -Description "Starts Coucou after the current user logs on and checks its local speech service." `
            -Force | Out-Null

        Set-AutostartPreference -Enabled $false
        $currentRunValue = Get-RunValue
        if ($null -ne $currentRunValue) {
            Remove-ItemProperty -Path $runKey -Name $runName
        }
        Write-StartupLog "Installed and enabled '$taskFolder$taskName' for $currentUser; the in-app logon trigger is disabled."
    }
    catch {
        if ($null -eq $existingTask) {
            Unregister-ScheduledTask -TaskName $taskName -TaskPath $taskFolder -Confirm:$false -ErrorAction SilentlyContinue
        }
        Restore-PreviousStartup -State $state
        if (-not $existingTask) {
            Remove-Item -LiteralPath $statePath -Force -ErrorAction SilentlyContinue
        }
        throw
    }
}

function Get-CoucouProcesses {
    @(Get-Process -Name "coucou" -ErrorAction SilentlyContinue)
}

try {
    switch ($Action) {
        "Install" {
            Install-StartupTask
        }
        "Enable" {
            $task = Get-ScheduledTask -TaskName $taskName -TaskPath $taskFolder -ErrorAction SilentlyContinue
            if ($null -eq $task) {
                throw "Task '$taskFolder$taskName' is not installed. Run with -Action Install first."
            }
            Enable-ScheduledTask -TaskName $taskName -TaskPath $taskFolder | Out-Null
            Write-StartupLog "Enabled '$taskFolder$taskName'."
        }
        "Disable" {
            $task = Get-ScheduledTask -TaskName $taskName -TaskPath $taskFolder -ErrorAction SilentlyContinue
            if ($null -eq $task) {
                throw "Task '$taskFolder$taskName' is not installed."
            }
            Disable-ScheduledTask -TaskName $taskName -TaskPath $taskFolder | Out-Null
            Write-StartupLog "Disabled '$taskFolder$taskName'."
        }
        "Remove" {
            $task = Get-ScheduledTask -TaskName $taskName -TaskPath $taskFolder -ErrorAction SilentlyContinue
            if ($null -ne $task) {
                Unregister-ScheduledTask -TaskName $taskName -TaskPath $taskFolder -Confirm:$false
            }
            if (Test-Path -LiteralPath $statePath -PathType Leaf) {
                $state = Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json
                Restore-PreviousStartup -State $state
                Remove-Item -LiteralPath $statePath -Force
            }
            Write-StartupLog "Removed '$taskFolder$taskName'."
        }
    }
    Write-Host "$Action completed. Startup log: $logPath"
}
catch {
    Write-StartupLog "ERROR ($Action): $($_.Exception.Message)"
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}
