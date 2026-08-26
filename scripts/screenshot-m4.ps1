# M4 runtime functional test + QEMU screendump screenshots
# Boots the guest, injects M4 commands (ramfs / dcache / tmpfs) over serial,
# captures VGA screenshots via QEMU monitor "screendump" at key milestones.
# NOTE: ASCII-only comments (UTF-8 Chinese comments break PS 5.1 ANSI parsing).
# Usage: powershell -ExecutionPolicy Bypass -File scripts/screenshot-m4.ps1
param(
    [string]$Kernel = "target/novos-kernel.bin",
    [int]$SerialPort = 4551,
    [int]$MonPort = 4552,
    [string]$OutDir = "target/screens"
)
$ErrorActionPreference = "Stop"
Get-Process qemu-system-x86_64 -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
$OutDir = [IO.Path]::GetFullPath($OutDir)
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

# scratch disk (M10 ext4-lite formats it; harmless for M4)
$BlkImg = "target/blk.img"
if (-not (Test-Path $BlkImg)) {
    $blk = New-Object byte[] (1024 * 1024)
    [IO.File]::WriteAllBytes($BlkImg, $blk)
}

$qemu = "C:/Program Files/qemu/qemu-system-x86_64.exe"
$qemuArgs = @(
    "-kernel", $Kernel, "-m", "64M",
    "-chardev", "socket,id=com1,host=127.0.0.1,port=$SerialPort,server=on",
    "-serial", "chardev:com1",
    "-device", "virtio-net-pci,disable-modern=on,netdev=net0",
    "-netdev", "user,id=net0",
    "-drive", "if=none,id=blk0,file=$BlkImg,format=raw,cache=unsafe",
    "-device", "virtio-blk-pci,drive=blk0",
    "-display", "none",
    "-vga", "std",
    "-no-reboot",
    "-monitor", "tcp:127.0.0.1:$MonPort,server,nowait"
)
$p = Start-Process -FilePath $qemu -ArgumentList $qemuArgs -PassThru -WindowStyle Hidden

# wait for serial chardev (QEMU blocks on it, so no output is lost)
$c = $null
for ($i = 0; $i -lt 30 -and -not $p.HasExited; $i++) {
    try {
        $c = New-Object Net.Sockets.TcpClient
        $c.Connect("127.0.0.1", $SerialPort)
        break
    } catch {
        Start-Sleep -Milliseconds 200
    }
}
if ($null -eq $c -or -not $c.Connected) {
    Write-Host "serial connect failed"
    if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
    exit 1
}
$s = $c.GetStream()
$s.ReadTimeout = 300
$sb = New-Object System.Text.StringBuilder

function Read-Out {
    while ($s.DataAvailable) { [void]$sb.Append([char]$s.ReadByte()) }
}
function Send-Line([string]$line) {
    $b = [Text.Encoding]::ASCII.GetBytes($line + "`n")
    $s.Write($b, 0, $b.Length)
    $s.Flush()
}
function Shot([string]$name) {
    $out = "$OutDir\$name.png"
    try {
        $m = New-Object Net.Sockets.TcpClient
        $m.Connect("127.0.0.1", $MonPort)
        $ms = $m.GetStream()
        $w = New-Object System.IO.StreamWriter($ms)
        $w.NewLine = "`n"
        $w.WriteLine("screendump $out")
        $w.Flush()
        Start-Sleep -Milliseconds 1500
        $m.Close()
        # QEMU screendump writes PPM here; convert to real PNG
        Ppm-ToPng $out
        Write-Host "screenshot saved: $name.png"
    } catch {
        Write-Host "screenshot $name failed: $($_.Exception.Message)"
    }
}

function Ppm-ToPng([string]$path) {
    if (-not (Test-Path $path)) { return }
    $b = [IO.File]::ReadAllBytes($path)
    if ($b.Length -lt 15) { return }
    $hdr = [Text.Encoding]::ASCII.GetString($b[0..1])
    if ($hdr -ne "P6") { return }  # already PNG or not PPM, skip
    # Pure .NET PNG encoder (no System.Drawing): P6 PPM(720x400) -> PNG
    $w = 720; $h = 400
    $raw = New-Object byte[] ($h * (1 + $w * 3))
    $idx = 15
    for ($y = 0; $y -lt $h; $y++) {
        $row = $y * (1 + $w * 3)
        $raw[$row] = 0  # filter: None
        for ($x = 0; $x -lt $w; $x++) {
            $p = $row + 1 + $x * 3
            $raw[$p] = $b[$idx]; $raw[$p + 1] = $b[$idx + 1]; $raw[$p + 2] = $b[$idx + 2]
            $idx += 3
        }
    }
    # zlib: 0x78 0x9C + deflate + adler32
    $ms = New-Object System.IO.MemoryStream
    $ms.WriteByte(0x78); $ms.WriteByte(0x9C)
    $ds = New-Object System.IO.Compression.DeflateStream($ms, [System.IO.Compression.CompressionMode]::Compress, $true)
    $ds.Write($raw, 0, $raw.Length)
    $ds.Close()
    $idat = $ms.ToArray()
    $ms.Close()
    $ad = Get-Adler32 $raw  # Int32 bit pattern = uint32 adler
    $adB = [BitConverter]::GetBytes($ad)
    [Array]::Reverse($adB)  # big-endian
    $idatFinal = New-Object byte[] ($idat.Length + 4)
    [Array]::Copy($idat, 0, $idatFinal, 0, $idat.Length)
    [Array]::Copy($adB, 0, $idatFinal, $idat.Length, 4)
    # Assemble PNG file
    $out = New-Object System.IO.MemoryStream
    $out.Write([byte[]](0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A), 0, 8)
    $ihdr = New-Object byte[] 13
    $wB = [BitConverter]::GetBytes([int32]$w); [Array]::Reverse($wB)
    $hB = [BitConverter]::GetBytes([int32]$h); [Array]::Reverse($hB)
    [Array]::Copy($wB, 0, $ihdr, 0, 4)
    [Array]::Copy($hB, 0, $ihdr, 4, 4)
    $ihdr[8] = 8; $ihdr[9] = 2; $ihdr[10] = 0; $ihdr[11] = 0; $ihdr[12] = 0
    Write-PngChunk $out "IHDR" $ihdr
    Write-PngChunk $out "IDAT" $idatFinal
    Write-PngChunk $out "IEND" (New-Object byte[] 0)
    $fs = [IO.File]::Create($path)
    $out.WriteTo($fs)
    $fs.Close(); $out.Close()
}

function Write-PngChunk($stream, [string]$type, [byte[]]$data) {
    $len = [BitConverter]::GetBytes([int32]$data.Length); [Array]::Reverse($len)
    $stream.Write($len, 0, 4)
    $t = [Text.Encoding]::ASCII.GetBytes($type)
    $stream.Write($t, 0, 4)
    $stream.Write($data, 0, $data.Length)
    $crcInput = New-Object byte[] (4 + $data.Length)
    [Array]::Copy($t, 0, $crcInput, 0, 4)
    [Array]::Copy($data, 0, $crcInput, 4, $data.Length)
    $crc = Get-Crc32 $crcInput
    $cb = [BitConverter]::GetBytes($crc)
    [Array]::Reverse($cb)  # big-endian CRC
    $stream.Write($cb, 0, 4)
}

function Get-Crc32([byte[]]$data) {
    # Pure Int32 bit ops (arithmetic shift == the logical shift CRC needs)
    $crc = [int]-1
    foreach ($byte in $data) {
        $crc = $crc -bxor [int]$byte
        for ($i = 0; $i -lt 8; $i++) {
            if (($crc -band 1) -ne 0) {
                $crc = ($crc -shr 1) -bxor [int]-306674912  # 0xEDB88320
            } else {
                $crc = $crc -shr 1
            }
        }
    }
    $crc -bxor [int]-1
}

function Get-Adler32([byte[]]$data) {
    $a = [int]1; $b = [int]0
    foreach ($byte in $data) {
        $a = ($a + [int]$byte) % 65521
        $b = ($b + $a) % 65521
    }
    ($a -bor ($b -shl 16))  # Int32 bit pattern = uint32 adler
}

# wait for userspace shell banner
$booted = $false
$deadline = (Get-Date).AddSeconds(15)
while ((Get-Date) -lt $deadline -and -not $booted) {
    Read-Out
    if ($sb.ToString().Contains("type 'help' for commands")) { $booted = $true }
    Start-Sleep -Milliseconds 200
}
if (-not $booted) {
    Write-Host "guest boot timeout; last serial output:"
    Write-Host $sb.ToString()
    if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
    exit 1
}
Write-Host "guest booted"

# Shot 1: boot + shell banner
Start-Sleep -Milliseconds 500
Shot "m4-1-boot"

# Batch A: ramfs root fs (M4-1/2)
Send-Line "mkdir /data"
Send-Line "ls"
Send-Line "fstest"
Send-Line "cat /etc/motd"
Start-Sleep -Milliseconds 1500
Read-Out
Shot "m4-2-ramfs"

# Batch B: dcache stress (M4-3)
Send-Line "dtest"
Start-Sleep -Milliseconds 1500
Read-Out
Shot "m4-3-dcache-created"
Send-Line "ls /dtest"
Start-Sleep -Milliseconds 2500
Read-Out
Shot "m4-4-dcache-listing"

# Batch C: tmpfs mount + stat + subdir (M4-4)
Send-Line "mkdir /mnt"
Send-Line "mount /mnt"
Send-Line "fstest /mnt/a.txt"
Send-Line "stat /mnt/a.txt"
Send-Line "mkdir /mnt/sub"
Send-Line "ls /mnt"
Start-Sleep -Milliseconds 1500
Read-Out
Shot "m4-5-tmpfs"

# drain rest of output, close
Start-Sleep -Milliseconds 800
Read-Out
$c.Close()

# needle checks (M4 functional assertions)
$ok = $true
$needles = @(
    "Novos-OS: boot ok",
    "m3: loading embedded userspace init",
    "Novos-OS M3 userspace shell (init)",
    "fstest: read 17B: hello from ramfs",
    "etc/",
    "data/",
    "hello from ramfs",
    "dcache: shrink",
    "dtest: created 1000 files",
    "f999",
    "stat: mode=33188 size=17",
    "a.txt",
    "sub/"
)
$output = $sb.ToString()
foreach ($needle in $needles) {
    if ($output -notmatch [regex]::Escape($needle)) {
        Write-Host "FAIL: missing '$needle'"
        $ok = $false
    }
}
$output | Set-Content -NoNewline -Path "target/m4-screenshot.log"

if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
if ($ok) {
    Write-Host "M4 screenshot test: PASS (see $OutDir)"
    Write-Host "  m4-1-boot.png  m4-2-ramfs.png  m4-3-dcache-created.png  m4-4-dcache-listing.png  m4-5-tmpfs.png"
    exit 0
} else {
    Write-Host "M4 screenshot test: FAIL"
    exit 1
}
