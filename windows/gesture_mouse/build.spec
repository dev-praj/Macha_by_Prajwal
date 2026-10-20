from pathlib import Path

from PyInstaller.utils.hooks import collect_all, collect_submodules


spec_dir = Path(SPECPATH).resolve()
windows_dir = spec_dir.parent
model_path = windows_dir / "target" / "python-assets" / "hand_landmarker.task"
if not model_path.is_file():
    raise FileNotFoundError(f"Build asset not found: {model_path}")

datas = [(str(model_path), ".")]
binaries = []
hiddenimports = []
for package in ("mediapipe", "cv2", "numpy", "pyautogui", "aiohttp", "PIL"):
    package_datas, package_binaries, package_hiddenimports = collect_all(package)
    datas += package_datas
    binaries += package_binaries
    hiddenimports += package_hiddenimports
hiddenimports += collect_submodules("mediapipe.tasks.python.vision")
hiddenimports += collect_submodules("aiohttp")

a = Analysis(
    [str(spec_dir / "app.py")],
    pathex=[str(spec_dir)],
    binaries=binaries,
    datas=datas,
    hiddenimports=hiddenimports,
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    excludes=[],
    noarchive=False,
    optimize=0,
)
pyz = PYZ(a.pure)
exe = EXE(
    pyz,
    a.scripts,
    a.binaries,
    a.datas,
    [],
    name="MachaGestureMouse",
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=False,
    console=True,
    disable_windowed_traceback=False,
)
