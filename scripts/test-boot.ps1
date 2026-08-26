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
# 清理残留 QEMU（失败退出时会遗留，导致下次 hostfwd 端口占用）
Get-Process qemu-system-x86_64 -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Remove-Item -Force $LogFile -ErrorAction SilentlyContinue
$qemu = "C:/Program Files/qemu/qemu-system-x86_64.exe"

if ($Mode -eq "boot") {
    $qemuArgs = @("-kernel", $Kernel, "-m", "64M",
                  "-serial", "file:$LogFile", "-display", "none", "-no-reboot",
                  "-device", "virtio-net-pci,disable-modern=on,netdev=net0",
                  "-netdev", "user,id=net0",
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
        "Novos-OS M3 userspace shell (init)",
        "virtio-net: io=",                    # M5-切片1：virtio-net 驱动初始化
        "net: arp who-has 10.0.2.2",          # tx：ARP 请求发出
        "arp: gateway 10.0.2.2",              # rx：学得网关 MAC
        "icmp: echo reply from 10.0.2.2"      # M5-切片2：IP+ICMP 回路（ping 通）
    )
} else {
    # shell 模式：socket chardev（无客户端时丢弃输出，须尽早连接）
    $qemuArgs = @(
        "-kernel", $Kernel, "-m", "64M",
        "-chardev", "socket,id=com1,host=127.0.0.1,port=$SerialPort,server=on,nowait",
        "-serial", "chardev:com1",
        "-device", "virtio-net-pci,disable-modern=on,netdev=net0",
        # M5-切片3/4/5：hostfwd 规则
        #   udp 12345/12344→19999：guest 发往 10.0.2.2:port 经 slirp 宿主侧回环
        #   tcp 20000：宿主连 guest echo 服务；tcp 80：宿主 HTTP GET guest 服务
        "-netdev", "user,id=net0,hostfwd=udp:127.0.0.1:12345-10.0.2.15:19999,hostfwd=udp:127.0.0.1:12344-10.0.2.15:19999,hostfwd=tcp:127.0.0.1:20000-10.0.2.15:20000,hostfwd=tcp:127.0.0.1:80-10.0.2.15:80",
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
    $s = $c.GetStream()
    $s.ReadTimeout = 300
    $sb = New-Object System.Text.StringBuilder
    try {
        while ($s.DataAvailable) { [void]$sb.Append([char]$s.ReadByte()) }
        $cmd = "help`nversion`nfdtest`nmkdir /data`nls`nfstest`ncat /etc/motd`nrm /etc/motd`ndtest`nls /dtest`nmkdir /mnt`nmount /mnt`nfstest /mnt/a.txt`nstat /mnt/a.txt`nmkdir /mnt/sub`nls /mnt`nudptest`ntcptest`nhttptest`nforktest`nutstest`n"
        $bytes = [Text.Encoding]::ASCII.GetBytes($cmd)
        $s.Write($bytes, 0, $bytes.Length)
        $s.Flush()
        # M5-切片4：等 guest 进入 tcptest 监听后，宿主再经 hostfwd(tcp:20000) 连接。
        # 过早连接会被 slirp 乐观接受、SYN 转发时 guest 尚未监听而丢弃。
        $script:hostTcp = ""
        $tcpReady = $false
        $deadline = (Get-Date).AddMilliseconds(20000)
        while ((Get-Date) -lt $deadline -and -not $tcpReady) {
            while ($s.DataAvailable) {
                [void]$sb.Append([char]$s.ReadByte())
                if ($sb.ToString().Contains("tcptest: listening on 20000")) { $tcpReady = $true }
            }
            Start-Sleep -Milliseconds 100
        }
        if ($tcpReady) {
            $tcpClient = $null
            $cdeadline = (Get-Date).AddMilliseconds(15000)
            while ((Get-Date) -lt $cdeadline -and $null -eq $tcpClient) {
                try {
                    $tcpClient = New-Object Net.Sockets.TcpClient
                    $tcpClient.Connect("127.0.0.1", 20000)
                } catch {
                    $tcpClient = $null
                    Start-Sleep -Milliseconds 200
                }
            }
            if ($null -ne $tcpClient) {
                try {
                    # 握手落定后发送，再读回显
                    Start-Sleep -Milliseconds 1500
                    $ns = $tcpClient.GetStream()
                    $payload = [Text.Encoding]::ASCII.GetBytes("hello tcp from host")
                    $ns.Write($payload, 0, 19)
                    $ns.Flush()
                    $tcpClient.Client.ReceiveTimeout = 5000
                    $rbuf = New-Object byte[] 128
                    $n = $ns.Read($rbuf, 0, 128)
                    if ($n -gt 0) { $script:hostTcp = [Text.Encoding]::ASCII.GetString($rbuf, 0, $n) }
                } catch {
                    $script:hostTcp = "<io>"
                }
                $tcpClient.Close()
            } else {
                $script:hostTcp = "<connect timeout>"
            }
        }
        # M5-切片5：等 guest 进入 httptest 监听后，宿主发 HTTP GET 并校验响应
        $script:hostHttp = ""
        $httpReady = $false
        $hdeadline = (Get-Date).AddMilliseconds(15000)
        while ((Get-Date) -lt $hdeadline -and -not $httpReady) {
            while ($s.DataAvailable) {
                [void]$sb.Append([char]$s.ReadByte())
                if ($sb.ToString().Contains("httptest: listening on 80")) { $httpReady = $true }
            }
            Start-Sleep -Milliseconds 100
        }
        if ($httpReady) {
            $httpClient = $null
            $h2 = (Get-Date).AddMilliseconds(10000)
            while ((Get-Date) -lt $h2 -and $null -eq $httpClient) {
                try {
                    $httpClient = New-Object Net.Sockets.TcpClient
                    $httpClient.Connect("127.0.0.1", 80)
                } catch {
                    $httpClient = $null
                    Start-Sleep -Milliseconds 200
                }
            }
            if ($null -ne $httpClient) {
                try {
                    Start-Sleep -Milliseconds 1500
                    $ns = $httpClient.GetStream()
                    $req = [Text.Encoding]::ASCII.GetBytes("GET / HTTP/1.0`r`n`r`n")
                    $ns.Write($req, 0, $req.Length)
                    $ns.Flush()
                    $httpClient.Client.ReceiveTimeout = 5000
                    $hb = New-Object System.Text.StringBuilder
                    $rrb = New-Object byte[] 256
                    $hdone = (Get-Date).AddMilliseconds(4000)
                    while ((Get-Date) -lt $hdone -and $hb.Length -lt 400) {
                        try {
                            $rn = $ns.Read($rrb, 0, 256)
                            if ($rn -le 0) { break }
                            [void]$hb.Append([Text.Encoding]::ASCII.GetString($rrb, 0, $rn))
                            if ($hb.ToString().Contains("</h1>")) { break }
                        } catch { break }
                    }
                    $script:hostHttp = $hb.ToString()
                } catch {
                    $script:hostHttp = "<io>"
                }
                $httpClient.Close()
            } else {
                $script:hostHttp = "<connect timeout>"
            }
        }
        # 排空串口输出（guest 依次执行命令，udptest 回环 + tcptest echo + httptest 服务后自行结束）
        Start-Sleep -Milliseconds 4000
        $s.ReadTimeout = 1500
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
        "commands: help | ls [dir] | cat <f> | echo <text> | mkdir <d> | rm <f> | rmdir <d> | mount <d> | stat <f> | version | fdtest | fstest [path] | dtest | udptest | tcptest | httptest | forktest | utstest | exit",
        "Novos-OS userspace init v0.3.0 (M3)",
        "fdtest: opened /dev/uart fd=3",
        "fdtest: hello via open fd",
        "fdtest: close rc=0",
        "fstest: read 17B: hello from ramfs",
        "etc/",      # ls 输出：预置目录
        "data/",     # ls 输出：mkdir 新建
        "hello from ramfs",          # cat /etc/motd 输出
        "dcache: shrink",            # M4-切片3：1000 文件触发回收
        "dtest: created 1000 files", # dtest 完成
        "f999",                      # ls /dtest 输出（完整枚举到末项）
        "stat: mode=33188 size=17",  # M4-切片4：tmpfs 文件 stat（0o100644 + 17B）
        "a.txt",                     # ls /mnt：tmpfs 挂载后写入的文件
        "sub/",                      # ls /mnt：tmpfs 内 mkdir
        "udptest: sent rc=20",       # M5-切片3：guest UDP 出站 1（20B "hello udp from novos"）
        "udptest: sent2 rc=15",      # M5-切片3：guest UDP 出站 2（15B "pong from novos"）
        "udptest: recv 20B: hello udp from novos", # M5-切片3：hostfwd 12345 回环入站
        "udptest: recv 15B: pong from novos", # M5-切片3：hostfwd 12344 回环入站
        "tcptest: listening on 20000", # M5-切片4：TCP 监听
        "tcptest: accepted fd=",       # M5-切片4：accept 取到连接
        "tcptest: recv 19B: hello tcp from host", # M5-切片4：收到宿主数据
        "tcptest: echoed 19",          # M5-切片4：echo 回发成功
        "httptest: listening on 80",   # M5-切片5：HTTP 服务监听
        "httptest: accepted fd=",      # M5-切片5：accept 取到连接
        "httptest: epoll wake",        # M5-切片5：epoll_wait 就绪
        "httptest: got request",       # M5-切片5：收到 HTTP 请求
        "httptest: served",            # M5-切片5：回发响应成功
        "forktest: parent getpid=1",   # M6-切片1：根 ns init pid=1
        "forktest: child A getpid=2",  # M6-切片1：fork 子进程根 ns pid=2
        "forktest: waitpid A reaped=", # M6-切片1：waitpid 回收子 A
        "forktest: child B getpid=1 (new pid ns)", # M6-切片1：CLONE_NEWPID 子进程 pid=1
        "forktest: waitpid B reaped=", # M6-切片1：waitpid 回收子 B
        "utstest: parent hostname=novos",    # M6-切片2：根 uts ns hostname
        "utstest: child hostname=childns",   # M6-切片2：CLONE_NEWUTS 子进程改 hostname
        "utstest: parent hostname after=novos" # M6-切片2：父 hostname 不受子影响
        # 注：网络（arp/icmp）断言仅放 boot 模式——shell 模式 nowait socket
        # 会在客户端连接前丢弃启动早期日志。
    )
}

# 断言
$ok = $true
if ($Mode -eq "shell" -and $script:hostTcp -ne "hello tcp from host") {
    Write-Host "FAIL: host TCP echo mismatch (got '$($script:hostTcp)')"
    $ok = $false
}
if ($Mode -eq "shell" -and ($script:hostHttp -notmatch "HTTP/1.0 200 OK" -or $script:hostHttp -notmatch "Novos-OS HTTP OK")) {
    Write-Host "FAIL: host HTTP response mismatch (got '$($script:hostHttp)')"
    $ok = $false
}
foreach ($needle in $needles) {
    if ($output -notmatch [regex]::Escape($needle)) {
        Write-Host "FAIL: missing '$needle'"
        $ok = $false
    }
}
if ($ok) {
    if ($null -ne $p -and -not $p.HasExited) { Stop-Process -Id $p.Id -Force }
    Write-Host "M3 $Mode test: PASS"
    exit 0
}
Write-Host "--- output ($Mode) ---"
Write-Host $output
Write-Host "M3 $Mode test: FAIL"
if ($null -ne $p -and -not $p.HasExited) { Stop-Process -Id $p.Id -Force }
exit 1
