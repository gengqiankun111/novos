# M3 切片4 demo: 启动内核 -> 进入用户态 shell -> 经 TCP 串口注入命令
# -> 读取 shell 输出 -> screendump -> quit。
# 用法: powershell -ExecutionPolicy Bypass -File scripts/m3-shell.ps1
param(
    [string]$Kernel = "target/novos-kernel.bin",
    [string]$OutDir = "images/m3",
    [string]$LogFile = "target/m3-shell.log",
    [int]$SerialPort = 4549,
    [int]$MonPort = 4550,
    [int]$WaitSec = 6
)

$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$ppm = Join-Path $OutDir "boot.ppm"
Remove-Item -Force $ppm, $LogFile -ErrorAction SilentlyContinue

$qemu = "C:/Program Files/qemu/qemu-system-x86_64.exe"
$qemuArgs = @(
    "-kernel", $Kernel, "-m", "64M",
    "-chardev", "socket,id=com1,host=127.0.0.1,port=$SerialPort,server=on,nowait",
    "-serial", "chardev:com1",
    "-display", "none", "-no-reboot",
    "-monitor", "tcp:127.0.0.1:$MonPort,server,nowait"
)

$p = Start-Process -FilePath $qemu -ArgumentList $qemuArgs -PassThru -WindowStyle Hidden
Write-Host "qemu started (pid $($p.Id)), waiting $WaitSec s for boot..."
Start-Sleep -Seconds $WaitSec

if ($p.HasExited) {
    Write-Error "QEMU exited early (code $($p.ExitCode))"
    exit 1
}

# 经串口注入命令，并读取 shell 回显/输出（验证 sys_read/sys_write 往返）。
$output = ""
try {
    $c = New-Object Net.Sockets.TcpClient
    $c.Connect("127.0.0.1", $SerialPort)
    $s = $c.GetStream()
    $s.ReadTimeout = 300
    # 先清空启动期间已到达的输出
    while ($s.DataAvailable) { [void]$s.ReadByte() }
    $cmd = "help`nversion`necho Hello from M3 shell`n"
    $bytes = [Text.Encoding]::ASCII.GetBytes($cmd)
    $s.Write($bytes, 0, $bytes.Length)
    $s.Flush()
    Write-Host "commands injected, draining output..."
    Start-Sleep -Milliseconds 2000
    $sb = New-Object System.Text.StringBuilder
    while ($true) {
        try { $b = $s.ReadByte() } catch { break }   # 读超时结束
        if ($b -lt 0) { break }
        [void]$sb.Append([char]$b)
    }
    $output = $sb.ToString()
    $c.Close()
} catch {
    $output += "<serial io: $($_.Exception.Message)>"
}

# screendump + quit
try {
    $m = New-Object Net.Sockets.TcpClient
    $m.Connect("127.0.0.1", $MonPort)
    $ms = $m.GetStream()
    $w = New-Object System.IO.StreamWriter($ms)
    $w.NewLine = "`n"
    $w.WriteLine("screendump $ppm")
    $w.Flush()
    Start-Sleep -Milliseconds 1200
    $w.WriteLine("quit")
    $w.Flush()
    Start-Sleep -Milliseconds 500
    $m.Close()
    Write-Host "monitor: screendump + quit sent"
} catch {
    Write-Warning "monitor connect failed: $_"
    if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
}
$null = $p.WaitForExit(8000)
if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }

$output | Set-Content -NoNewline -Path $LogFile
Write-Host "=== m3 shell output (after serial commands) ==="
Write-Host $output
if (Test-Path $ppm) { Write-Host "screenshot saved: $ppm" }
else { Write-Warning "screendump produced no file" }
