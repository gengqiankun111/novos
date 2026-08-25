# Novos-OS milestone screenshot helper: boot QEMU -> wait -> screendump -> quit.
# Usage:  powershell -ExecutionPolicy Bypass -File scripts/screenshot.ps1 -OutDir images/m1
param(
    [Parameter(Mandatory = $true)][string]$OutDir,
    [string]$Kernel = "target/novos-kernel.bin",
    [string]$LogFile = "target/boot.log",
    [int]$WaitSec = 4,
    [int]$Port = 4545
)

$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$ppm = Join-Path $OutDir "boot.ppm"
Remove-Item -Force $ppm, $LogFile -ErrorAction SilentlyContinue

$qemu = "C:/Program Files/qemu/qemu-system-x86_64.exe"
$mon  = "tcp:127.0.0.1:$Port,server,nowait"
$qemuArgs = @("-kernel", $Kernel, "-m", "64M",
              "-serial", "file:$LogFile",
              "-display", "none", "-no-reboot", "-monitor", $mon)

for ($try = 1; $try -le 3; $try++) {
    $p = Start-Process -FilePath $qemu -ArgumentList $qemuArgs -PassThru -WindowStyle Hidden
    Start-Sleep -Seconds $WaitSec

    if ($p.HasExited) {
        Write-Warning "QEMU exited early (code $($p.ExitCode)), retry $try/3"
        Start-Sleep -Seconds 2
        continue
    }

    # Connect monitor: screendump first, then quit (clean exit flushes serial buffer).
    try {
        $c = New-Object Net.Sockets.TcpClient
        $c.Connect("127.0.0.1", $Port)
        $s = $c.GetStream()
        $w = New-Object System.IO.StreamWriter($s)
        $w.NewLine = "`n"
        $w.WriteLine("screendump $ppm")
        $w.Flush()
        Start-Sleep -Milliseconds 1000
        $w.WriteLine("quit")
        $w.Flush()
        Start-Sleep -Milliseconds 500
        $c.Close()
    } catch {
        Write-Warning "monitor connect failed: $_"
        if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
        Start-Sleep -Seconds 2
        continue
    }

    $null = $p.WaitForExit(10000)
    if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }

    if (Test-Path $ppm) {
        Write-Host "screenshot saved: $ppm"
        Write-Host "serial log: $LogFile"
        exit 0
    }
    Write-Warning "screendump produced no file, retry $try/3"
    Start-Sleep -Seconds 2
}

Write-Error "screenshot failed after 3 attempts"
exit 1
