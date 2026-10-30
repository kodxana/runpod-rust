$root = $args[0]
$mode = $args[1]
$command = @($args | Select-Object -Skip 2)
Add-Content -LiteralPath (Join-Path $root 'commands.txt') -Value ($command -join '|') -Encoding UTF8
if ($command -contains '--version') {
    if ($mode -eq 'bootstrap' -or $mode -eq 'bootstrap-fail') { exit 1 }
    exit 0
}
if ($command -contains 'ensurepip') {
    if ($mode -eq 'bootstrap-fail') { [Console]::Error.WriteLine('bootstrap unavailable'); exit 1 }
    exit 0
}
if ($command -contains 'wheel') {
    if ($mode -eq 'wheel-fail') { [Console]::Error.WriteLine('wheel build failed'); exit 1 }
    $wheelDir = $command[[Array]::IndexOf($command, '-w') + 1]
    if ($mode -ne 'no-wheel') {
        [IO.File]::WriteAllText((Join-Path $wheelDir 'runpod-1-py3-none-any.whl'), '')
        [IO.File]::WriteAllText((Join-Path $wheelDir 'runpod-2-py3-none-any.whl'), '')
    }
    exit 0
}
if ($mode -eq 'large-error') { [Console]::Error.WriteLine(('x' * 262144)) }
[Console]::WriteLine('Collecting numpy>=1.0')
[Console]::WriteLine('  Downloading numpy-2.0-cp312.whl')
[Console]::WriteLine('Collecting requests[security]')
if ($mode -eq 'delay') { Start-Sleep -Milliseconds 250 }
if ($mode -eq 'large-error' -or $mode -eq 'vendor-fail') { [Console]::Error.WriteLine('resolver failed'); exit 1 }
if (Test-Path -LiteralPath (Join-Path $root 'seed')) {
    $target = $command[[Array]::IndexOf($command, '--target') + 1]
    [IO.Directory]::CreateDirectory($target) | Out-Null
    Get-ChildItem -LiteralPath (Join-Path $root 'seed') | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $target -Recurse -Force }
}
exit 0
