# M3 集成测试：双模式
#   -Mode boot  : -serial file 捕获完整启动日志，断言内核引导 + ELF 加载 + shell banner
#   -Mode shell : socket 串口注入 help/version，断言命令往返（sys_read/sys_write）
# 用法: powershell -ExecutionPolicy Bypass -File scripts/test-boot.ps1 -Mode boot
param(
    [string]$Kernel = "target/novos-kernel.bin",
    [ValidateSet("boot", "shell")][string]$Mode = "boot",
    [string]$LogFile = "target/test-boot.log",
    [int]$SerialPort = 4551,
    [int]$MonPort = 4552,
    [int]$WaitSec = 6
)

$ErrorActionPreference = "Stop"
Remove-Item -Force $LogFile -ErrorAction SilentlyContinue
$qemu = "C:/Program Files/qemu/qemu-system-x86_64.exe"

if ($Mode -eq "boot") {
    $qemuArgs = @("-kernel", $Kernel, "-m", "64M",
                  "-serial", "file:$LogFile", "-display", "none", "-no-reboot",
                  "-monitor", "tcp:127.0.0.1:$MonPort,server,nowait")
    $p = Start-Process -FilePath $qemu -ArgumentList $qemuArgs -PassThru -WindowStyle Hidden
    Start-Sleep -Seconds $WaitSec
    try {
        $m = New-Object Net.Sockets.TcpClient
        $m.Connect("127.0.0.1", $MonPort)
        $ms = $m.GetStream()
        $w = New-Object System.IO.StreamWriter($ms)
        $w.NewLine = "`n"
        $w.WriteLine("quit")
        $w.Flush()
        Start-Sleep -Milliseconds 300
        $m.Close()
    } catch { }
    $null = $p.WaitForExit(6000)
    if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
    $output = Get-Content -Raw -Path $LogFile
    $needles = @(
        "Novos-OS: boot ok",
        "m3: loading embedded userspace init",
        "m3/elf: PT_LOAD vaddr=0x8000000000",
        "Novos-OS M3 userspace shell (init)"
    )
} else {
    # shell 模式：socket chardev（无客户端时丢弃输出，须尽早连接）
    $qemuArgs = @(
        "-kernel", $Kernel, "-m", "64M",
        "-chardev", "socket,id=com1,host=127.0.0.1,port=$SerialPort,server=on,nowait",
        "-serial", "chardev:com1",
        "-display", "none", "-no-reboot",
        "-monitor", "tcp:127.0.0.1:$MonPort,server,nowait"
    )
    $p = Start-Process -FilePath $qemu -ArgumentList $qemuArgs -PassThru -WindowStyle Hidden
    $c = $null
    for ($i = 0; $i -lt 20 -and -not $p.HasExited; $i++) {
        try {
            $c = New-Object Net.Sockets.TcpClient
            $c.Connect("127.0.0.1", $SerialPort)
            break
        } catch {
            Start-Sleep -Milliseconds 200
        }
    }
    if ($null -eq $c -or -not $c.Connected) {
        Write-Error "serial connect failed"
        if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
        exit 1
    }
    Start-Sleep -Seconds $WaitSec
    if ($p.HasExited) {
        Write-Error "QEMU exited early (code $($p.ExitCode))"
        exit 1
    }
    $output = ""
    try {
        $s = $c.GetStream()
        $s.ReadTimeout = 300
        $sb = New-Object System.Text.StringBuilder
        while ($s.DataAvailable) { [void]$sb.Append([char]$s.ReadByte()) }
        $cmd = "help`nversion`nfdtest`n"
        $bytes = [Text.Encoding]::ASCII.GetBytes($cmd)
        $s.Write($bytes, 0, $bytes.Length)
        $s.Flush()
        Start-Sleep -Milliseconds 1500
        while ($true) {
            try { $b = $s.ReadByte() } catch { break }
            if ($b -lt 0) { break }
            [void]$sb.Append([char]$b)
        }
        $output = $sb.ToString()
        $c.Close()
    } catch {
        $output += "<serial io: $($_.Exception.Message)>"
    }
    try {
        $m = New-Object Net.Sockets.TcpClient
        $m.Connect("127.0.0.1", $MonPort)
        $ms = $m.GetStream()
        $w = New-Object System.IO.StreamWriter($ms)
        $w.NewLine = "`n"
        $w.WriteLine("quit")
        $w.Flush()
        Start-Sleep -Milliseconds 300
        $m.Close()
    } catch { }
    $null = $p.WaitForExit(6000)
    if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
    $output | Set-Content -NoNewline -Path $LogFile
    $needles = @(
        "commands: help | echo <text> | version | fdtest | exit",
        "Novos-OS userspace init v0.3.0 (M3)",
        "fdtest: opened /dev/uart fd=3",
        "fdtest: hello via open fd",
        "fdtest: close rc=0"
    )
}

# 断言
$ok = $true
foreach ($needle in $needles) {
    if ($output -notmatch [regex]::Escape($needle)) {
        Write-Host "FAIL: missing '$needle'"
        $ok = $false
    }
}
if ($ok) {
    Write-Host "M3 $Mode test: PASS"
    exit 0
}
Write-Host "--- output ($Mode) ---"
Write-Host $output
Write-Host "M3 $Mode test: FAIL"
exit 1
