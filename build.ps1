$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
$cargo = 'cargo'
$localCargo = Join-Path $PSScriptRoot '.tools\cargo\bin\cargo.exe'
if (Test-Path -LiteralPath $localCargo) {
    $env:RUSTUP_HOME = Join-Path $PSScriptRoot '.tools\rustup'
    $env:CARGO_HOME = Join-Path $PSScriptRoot '.tools\cargo'
    $cargo = $localCargo
    $llvm = Get-ChildItem (Join-Path $PSScriptRoot '.tools\llvm') -Directory -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($llvm) {
        $dlltool = Join-Path $llvm.FullName 'bin\llvm-dlltool.exe'
        $lld = Join-Path $env:RUSTUP_HOME 'toolchains\stable-x86_64-pc-windows-gnu\lib\rustlib\x86_64-pc-windows-gnu\bin\rust-lld.exe'
        $env:CARGO_ENCODED_RUSTFLAGS = @('-C', ('dlltool=' + $dlltool), '-C', ('linker=' + $lld), '-C', 'linker-flavor=ld.lld') -join [char]31
        $bundled = Join-Path $env:RUSTUP_HOME 'toolchains\stable-x86_64-pc-windows-gnu\lib\rustlib\x86_64-pc-windows-gnu\bin\self-contained'
        $env:PATH = $bundled + ';' + $env:PATH
    }
}
& $cargo test --locked
if ($LASTEXITCODE -ne 0) { throw 'Tests failed' }
& $cargo build --release --locked
if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item -LiteralPath target\release\sse-taskbar.exe -Destination dist\sse-taskbar.exe
Copy-Item -LiteralPath calendar.json,README.md -Destination dist
Compress-Archive -Path dist\sse-taskbar.exe,dist\calendar.json,dist\README.md -DestinationPath dist\sse-taskbar-windows-x64.zip -Force
Write-Output 'Ready: dist\sse-taskbar.exe'
