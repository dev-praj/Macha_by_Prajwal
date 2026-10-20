$ErrorActionPreference = "Stop"

$windowsDir = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$targetDir = Join-Path $windowsDir "target"
$assetsDir = Join-Path $targetDir "python-assets"
$distDir = Join-Path $targetDir "python-helpers"
$buildDir = Join-Path $targetDir "python-build"
$gestureModel = Join-Path $assetsDir "hand_landmarker.task"
$whisperModel = Join-Path $assetsDir "whisper-base.en"
$gestureEnv = Join-Path $buildDir "gesture-venv"
$sttEnv = Join-Path $buildDir "stt-venv"
$gesturePython = Join-Path $gestureEnv "Scripts\python.exe"
$sttPython = Join-Path $sttEnv "Scripts\python.exe"

$pythonVersion = & py -3.13 --version
if ($LASTEXITCODE -ne 0 -or $pythonVersion -notmatch "Python 3\.13") {
    throw "Python 3.13 is required to build the bundled helpers."
}

New-Item -ItemType Directory -Force $assetsDir, $distDir, $buildDir | Out-Null
if (-not (Test-Path $gesturePython)) {
    & py -3.13 -m venv $gestureEnv
    if ($LASTEXITCODE -ne 0) { throw "Could not create the gesture build environment." }
}
if (-not (Test-Path $sttPython)) {
    & py -3.13 -m venv $sttEnv
    if ($LASTEXITCODE -ne 0) { throw "Could not create the STT build environment." }
}

& $gesturePython -m pip install -r (Join-Path $windowsDir "gesture_mouse\requirements-build.txt")
if ($LASTEXITCODE -ne 0) { throw "Could not install gesture build dependencies." }
& $sttPython -m pip install -r (Join-Path $windowsDir "stt\requirements-build.txt")
if ($LASTEXITCODE -ne 0) { throw "Could not install STT build dependencies." }

if (-not (Test-Path $gestureModel)) {
    Invoke-WebRequest `
        -Uri "https://storage.googleapis.com/mediapipe-models/hand_landmarker/hand_landmarker/float16/1/hand_landmarker.task" `
        -OutFile $gestureModel
}
if (-not (Test-Path (Join-Path $whisperModel "model.bin"))) {
    & $sttPython -c "import sys; from huggingface_hub import snapshot_download; snapshot_download(repo_id='Systran/faster-whisper-base.en', revision='3d3d5dee26484f91867d81cb899cfcf72b96be6c', local_dir=sys.argv[1], allow_patterns=['config.json', 'model.bin', 'tokenizer.json', 'vocabulary.txt'])" $whisperModel
    if ($LASTEXITCODE -ne 0) { throw "Could not download the pinned faster-whisper base.en model." }
}

& $gesturePython -m PyInstaller --noconfirm --clean `
    --distpath $distDir `
    --workpath (Join-Path $buildDir "gesture-work") `
    (Join-Path $windowsDir "gesture_mouse\build.spec")
if ($LASTEXITCODE -ne 0) { throw "Could not build the Gesture Mouse executable." }

& $sttPython -m PyInstaller --noconfirm --clean `
    --distpath $distDir `
    --workpath (Join-Path $buildDir "stt-work") `
    (Join-Path $windowsDir "stt\build.spec")
if ($LASTEXITCODE -ne 0) { throw "Could not build the STT executable." }

Push-Location $windowsDir
try {
    $previousTargetDir = $env:CARGO_TARGET_DIR
    Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
    & cargo build --release -p macha-hook
    if ($LASTEXITCODE -ne 0) { throw "Could not build the Macha hook resource." }

    $packageTargetDir = Join-Path $targetDir "packaged"
    $env:CARGO_TARGET_DIR = $packageTargetDir
    try {
        & npm run tauri -- build
        if ($LASTEXITCODE -ne 0) { throw "Could not build the Macha Windows installer." }
    }
    finally {
        if ($null -eq $previousTargetDir) {
            Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
        }
        else {
            $env:CARGO_TARGET_DIR = $previousTargetDir
        }
    }

    $builtNsisDir = Join-Path $packageTargetDir "release\bundle\nsis"
    $standardNsisDir = Join-Path $targetDir "release\bundle\nsis"
    $installers = Get-ChildItem $builtNsisDir -Filter "*-setup.exe"
    if (-not $installers) { throw "The Tauri build produced no NSIS installer." }
    New-Item -ItemType Directory -Force $standardNsisDir | Out-Null
    Copy-Item -Path $installers.FullName -Destination $standardNsisDir -Force
    & node .\scripts\pack.mjs
    if ($LASTEXITCODE -ne 0) { throw "Could not publish the Macha Windows installer." }
}
finally {
    Pop-Location
}
