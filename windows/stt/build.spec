from pathlib import Path

from PyInstaller.utils.hooks import collect_all, collect_submodules


spec_dir = Path(SPECPATH).resolve()
windows_dir = spec_dir.parent
model_dir = windows_dir / "target" / "python-assets" / "whisper-base.en"
if not (model_dir / "model.bin").is_file():
    raise FileNotFoundError(f"Build asset not found: {model_dir / 'model.bin'}")

datas = []
binaries = []
hiddenimports = []
for package in ("faster_whisper", "ctranslate2", "av", "flask", "numpy"):
    package_datas, package_binaries, package_hiddenimports = collect_all(package)
    datas += package_datas
    binaries += package_binaries
    hiddenimports += package_hiddenimports
hiddenimports += collect_submodules("faster_whisper")
datas.append((str(model_dir), "whisper-base.en"))

a = Analysis(
    [str(spec_dir.parent.parent / "stt_server.py")],
    pathex=[str(spec_dir.parent.parent)],
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
    name="MachaSTT",
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=False,
    console=True,
    disable_windowed_traceback=False,
)
