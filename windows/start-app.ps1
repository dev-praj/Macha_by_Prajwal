# Start Coucou's release build once and wait for its local speech service.
[CmdletBinding()]
param(
    [ValidateRange(0, 300)]
    [int]$StartupDelaySeconds = 0,
    [ValidateRange(1, 1800)]
    [int]$HealthTimeoutSeconds = 600
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

function Get-CoucouProcesses {
    @(Get-Process -Name "coucou" -ErrorAction SilentlyContinue)
}

try {
    if (-not (Test-Path -LiteralPath $appPath -PathType Leaf)) {
        throw "Release executable not found: $appPath. Build the release app before enabling startup."
    }
    if (-not (Test-Path -LiteralPath $curlPath -PathType Leaf)) {
        throw "Windows curl.exe not found at the expected path: $curlPath."
    }

    $processes = Get-CoucouProcesses
    if ($processes.Count -gt 0) {
        $process = $processes[0]
        Write-StartupLog "Coucou is already running (PID $($process.Id)); waiting for its health endpoint."
    }
    else {
        if ($StartupDelaySeconds -gt 0) {
            Write-StartupLog "Waiting $StartupDelaySeconds seconds before startup."
            Start-Sleep -Seconds $StartupDelaySeconds
        }

        $processes = Get-CoucouProcesses
        if ($processes.Count -gt 0) {
            $process = $processes[0]
            Write-StartupLog "Coucou started during the delay (PID $($process.Id)); waiting for its health endpoint."
        }
        else {
            $listeners = @(Get-NetTCPConnection -State Listen -LocalPort 5005 -ErrorAction SilentlyContinue)
            if ($listeners.Count -gt 0) {
                $processIds = ($listeners | Select-Object -ExpandProperty OwningProcess -Unique) -join ", "
                throw "Port 5005 is already in use by process ID(s) $processIds. Coucou was not started; inspect the listener and free the port."
            }

            $process = Start-Process -FilePath $appPath -WorkingDirectory $repoRoot -PassThru
            Write-StartupLog "Started Coucou from '$appPath' (PID $($process.Id))."
        }
    }

    $deadline = [DateTime]::UtcNow.AddSeconds($HealthTimeoutSeconds)
    do {
        if ((Get-CoucouProcesses).Count -eq 0) {
            throw "Coucou exited before its speech service became healthy."
        }

        $body = & $curlPath --fail --silent --show-error --max-time 2 $healthUrl 2>$null
        if ($LASTEXITCODE -eq 0 -and (($body -join "") -match '"status"\s*:\s*"ok"')) {
            Write-StartupLog "Health check passed at $healthUrl."
            exit 0
        }

        Start-Sleep -Seconds 3
    } while ([DateTime]::UtcNow -lt $deadline)

    throw "Health endpoint $healthUrl did not become ready within $HealthTimeoutSeconds seconds. See '$logPath' and '$logDirectory\coucou.log'."
}
catch {
    Write-StartupLog "ERROR: $($_.Exception.Message)"
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}
