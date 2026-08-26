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
    "-device", "virtio-net-pci,disable-modern=on,netdev=net0",
    # M5-切片3/4：UDP 双规则回环 + TCP hostfwd（宿主连 guest echo 服务）
    "-netdev", "user,id=net0,hostfwd=udp:127.0.0.1:12345-10.0.2.15:19999,hostfwd=udp:127.0.0.1:12344-10.0.2.15:19999,hostfwd=tcp:127.0.0.1:20000-10.0.2.15:20000",
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
    $cmd = "help`nmkdir /mnt`nmount /mnt`nfstest /mnt/a.txt`nstat /mnt/a.txt`nmkdir /mnt/sub`nls /mnt`nudptest`ntcptest`nls`nversion`n"
    $bytes = [Text.Encoding]::ASCII.GetBytes($cmd)
    $s.Write($bytes, 0, $bytes.Length)
    $s.Flush()
    Write-Host "commands injected, waiting for tcptest listen..."
    # 等 guest 监听 20000 后，宿主连接并发数据、收回显（M5-切片4 演示）
    $tcpReady = $false
    $sb = New-Object System.Text.StringBuilder
    $deadline = (Get-Date).AddMilliseconds(15000)
    while ((Get-Date) -lt $deadline -and -not $tcpReady) {
        while ($s.DataAvailable) {
            [void]$sb.Append([char]$s.ReadByte())
            if ($sb.ToString().Contains("tcptest: listening on 20000")) { $tcpReady = $true }
        }
        Start-Sleep -Milliseconds 100
    }
    if ($tcpReady) {
        try {
            $tcp = New-Object Net.Sockets.TcpClient
            $tcp.Connect("127.0.0.1", 20000)
            Start-Sleep -Milliseconds 1500
            $ns = $tcp.GetStream()
            $payload = [Text.Encoding]::ASCII.GetBytes("hello tcp from host")
            $ns.Write($payload, 0, 19)
            $ns.Flush()
            $tcp.Client.ReceiveTimeout = 5000
            $rbuf = New-Object byte[] 128
            $n = $ns.Read($rbuf, 0, 128)
            if ($n -gt 0) { Write-Host ("host got TCP echo: " + [Text.Encoding]::ASCII.GetString($rbuf, 0, $n)) }
            $tcp.Close()
        } catch { Write-Host ("host TCP io: " + $_.Exception.Message) }
    }
    Write-Host "draining output..."
    Start-Sleep -Milliseconds 2000
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
