$scratch = $args[0]
$mode = $args[1]
$commandArgs = @($args | Select-Object -Skip 2)
$utf8 = New-Object System.Text.UTF8Encoding($false)
[IO.File]::AppendAllText((Join-Path $scratch 'commands.txt'), ($env:DEBIAN_FRONTEND + '|' + ($commandArgs -join '|') + "`n"), $utf8)
if ($mode -eq 'hang') {
    [IO.File]::WriteAllText((Join-Path $scratch 'pid'), [string]$PID, $utf8)
    Start-Sleep -Seconds 60
}
if (($mode -eq 'update-fail' -and $commandArgs[0] -eq 'update') -or ($mode -eq 'install-fail' -and $commandArgs[0] -eq 'install')) {
    [Console]::Error.Write(('x' * 2100) + 'TAIL')
    exit 9
}
exit 0
