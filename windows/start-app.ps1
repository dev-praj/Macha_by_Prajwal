# Start Coucou's release build once and wait for its local speech service.
[CmdletBinding()]
param(
    [ValidateRange(0, 300)]
    [int]$StartupDelaySeconds = 0,
    [ValidateRange(1, 1800)]
    [int]$HealthTimeoutSeconds = 600,
    [switch]$MonitorProcess,
    [ValidateRange(0, 5)]
    [int]$MaxCrashRestarts = 3,
    [ValidateRange(1, 300)]
    [int]$RestartDelaySeconds = 10
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$appPath = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "target\release\coucou.exe"))
$logDirectory = Join-Path $env:LOCALAPPDATA "Coucou"
$logPath = Join-Path $logDirectory "startup.log"
$curlPath = Join-Path $env:SystemRoot "System32\curl.exe"
$healthUrl = "http://127.0.0.1:5005/health"

New-Item -ItemType Directory -Path $logDirectory -Force | Out-Null

function Write-StartupLog {
    param([Parameter(Mandatory = $true)][string]$Message)

    $timestamp = Get-Date -Format "yyyy-MM-dd HH:mm:ss.fff"
    Add-Content -LiteralPath $logPath -Value "$timestamp $Message" -Encoding UTF8
}

function Get-CoucouProcess {
    Get-Process -Name "coucou" -ErrorAction SilentlyContinue | Select-Object -First 1
}

try {
    if (-not (Test-Path -LiteralPath $appPath -PathType Leaf)) {
        throw "Release executable not found: $appPath. Build the release app before enabling startup."
    }
    if (-not (Test-Path -LiteralPath $curlPath -PathType Leaf)) {
        throw "Windows curl.exe not found at the expected path: $curlPath."
    }

    $restartCount = 0
    $delayBeforeLaunch = $StartupDelaySeconds
    while ($true) {
        $process = Get-CoucouProcess
        $startedHere = $false
        if ($null -eq $process) {
            if ($delayBeforeLaunch -gt 0) {
                Write-StartupLog "Waiting $delayBeforeLaunch seconds before startup."
                Start-Sleep -Seconds $delayBeforeLaunch
                $delayBeforeLaunch = 0
            }

            $process = Get-CoucouProcess
            if ($null -ne $process) {
                Write-StartupLog "Coucou started during the delay (PID $($process.Id)); waiting for its health endpoint."
            }
            else {
                $listeners = @(Get-NetTCPConnection -State Listen -LocalPort 5005 -ErrorAction SilentlyContinue)
                if ($listeners.Count -gt 0) {
                    $processIds = ($listeners | Select-Object -ExpandProperty OwningProcess -Unique) -join ", "
                    throw "Port 5005 is already in use by process ID(s) $processIds. Coucou was not started; inspect the listener and free the port."
                }

                $process = Start-Process -FilePath $appPath -WorkingDirectory $repoRoot -PassThru
                $startedHere = $true
                Write-StartupLog "Started Coucou from '$appPath' (PID $($process.Id))."
            }
        }
        else {
            Write-StartupLog "Coucou is already running (PID $($process.Id)); waiting for its health endpoint."
        }

        $deadline = [DateTime]::UtcNow.AddSeconds($HealthTimeoutSeconds)
        $healthy = $false
        do {
            if ($null -eq (Get-Process -Id $process.Id -ErrorAction SilentlyContinue)) {
                break
            }

            $body = & $curlPath --fail --silent --max-time 2 $healthUrl 2>$null
            if ($LASTEXITCODE -eq 0 -and (($body -join "") -match '"status"\s*:\s*"ok"')) {
                $healthy = $true
                break
            }

            Start-Sleep -Seconds 3
        } while ([DateTime]::UtcNow -lt $deadline)

        if (-not $healthy) {
            $reason = "Health endpoint $healthUrl did not become ready within $HealthTimeoutSeconds seconds."
            if ($startedHere -and $null -ne (Get-Process -Id $process.Id -ErrorAction SilentlyContinue)) {
                Stop-Process -Id $process.Id -Force
                Write-StartupLog "Stopped Coucou PID $($process.Id) after its startup health check failed."
            }
            if (-not $MonitorProcess -or -not $startedHere -or $restartCount -ge $MaxCrashRestarts) {
                throw "$reason See '$logPath' and '$logDirectory\coucou.log'."
            }

            $restartCount++
            Write-StartupLog "ERROR: $reason Restart $restartCount of $MaxCrashRestarts in $RestartDelaySeconds seconds."
            Start-Sleep -Seconds $RestartDelaySeconds
            continue
        }

        Write-StartupLog "Health check passed at $healthUrl."
        if (-not $MonitorProcess) {
            exit 0
        }

        Write-StartupLog "Monitoring Coucou process $($process.Id)."
        $process.WaitForExit()
        $process.Refresh()
        if ($process.ExitCode -eq 0) {
            Write-StartupLog "Coucou exited normally."
            exit 0
        }
        if ($restartCount -ge $MaxCrashRestarts) {
            throw "Coucou exited with code $($process.ExitCode) after $restartCount crash restart(s); no retries remain."
        }

        $restartCount++
        Write-StartupLog "ERROR: Coucou exited with code $($process.ExitCode). Restart $restartCount of $MaxCrashRestarts in $RestartDelaySeconds seconds."
        Start-Sleep -Seconds $RestartDelaySeconds
    }
}
catch {
    Write-StartupLog "ERROR: $($_.Exception.Message)"
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}
