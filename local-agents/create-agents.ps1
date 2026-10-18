# Script to build and register agent models in Ollama on Windows
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$ModelfilesDir = Join-Path $ScriptDir "modelfiles"

$Agents = [ordered]@{
    "agent-router"     = "Modelfile.router"
    "agent-coder"      = "Modelfile.coder"
    "agent-summarizer" = "Modelfile.summarizer"
    "agent-general"    = "Modelfile.general"
}

Write-Host "Creating agent models from Modelfiles..." -ForegroundColor Cyan

foreach ($name in $Agents.Keys) {
    $file = Join-Path $ModelfilesDir $Agents[$name]
    if (Test-Path $file) {
        Write-Host "Creating $name from $($Agents[$name])..." -ForegroundColor Yellow
        ollama create $name -f $file
        if ($LASTEXITCODE -eq 0) {
            Write-Host "[OK] $name created successfully.`n" -ForegroundColor Green
        } else {
            Write-Host "[ERROR] Failed to create $name`n" -ForegroundColor Red
        }
    } else {
        Write-Host "[ERROR] Modelfile not found: $file`n" -ForegroundColor Red
    }
}

Write-Host "All agents processed. Current Ollama models:" -ForegroundColor Cyan
ollama list
