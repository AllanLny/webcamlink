# Vérifications locales : l'équivalent exact du job Windows de `ci.yml`, plus
# la validation des fichiers YAML. À lancer avant de pousser, ou quand GitHub
# Actions est indisponible.
#
#   powershell -ExecutionPolicy Bypass -File scripts/check.ps1
#
# Rien n'est partagé avec l'installation en cours : la compilation utilise un
# dossier cible temporaire et un dossier de build séparé, donc l'agent et la
# caméra virtuelle peuvent rester en service pendant la vérification.

$ErrorActionPreference = 'Stop'
Push-Location (Join-Path $PSScriptRoot '..')
try {
    '== tests de l''agent =='
    $env:CARGO_TARGET_DIR = Join-Path $env:TEMP 'webcamlink-check-target'
    cargo test --release --locked --manifest-path pc/agent/Cargo.toml
    if ($LASTEXITCODE -ne 0) { throw 'tests de l''agent en échec' }

    '== compilation de la caméra virtuelle =='
    $cmake = (Get-Command cmake -ErrorAction SilentlyContinue).Source
    if (-not $cmake) {
        $vswhere = 'C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe'
        if (Test-Path $vswhere) {
            $vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
            $candidate = Join-Path $vs 'Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe'
            if (Test-Path $candidate) { $cmake = $candidate }
        }
    }
    if (-not $cmake) { throw 'cmake introuvable (Visual Studio Build Tools ?)' }

    & $cmake -S pc/vcam -B pc/vcam/build-check -A x64
    if ($LASTEXITCODE -ne 0) { throw 'configuration CMake en échec' }
    & $cmake --build pc/vcam/build-check --config Release
    if ($LASTEXITCODE -ne 0) { throw 'compilation de la DLL en échec' }

    '== fichiers YAML des workflows =='
    python -c "import yaml,glob; [yaml.safe_load(open(f,encoding='utf-8')) for f in glob.glob('.github/workflows/*.yml')+glob.glob('.github/ISSUE_TEMPLATE/*.yml')+glob.glob('codemagic.yaml')]; print('yaml ok')"
    if ($LASTEXITCODE -ne 0) { throw 'YAML invalide' }

    '== terminé : agent testé, caméra virtuelle compilée, YAML valides =='
} finally {
    Pop-Location
}
