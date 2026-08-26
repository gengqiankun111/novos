# 简易 pcap 解析：打印以太网帧摘要
$b = [IO.File]::ReadAllBytes($args[0])
"pcap size: $($b.Length)"
if ($b.Length -lt 60) { exit }
$n = [BitConverter]::ToUInt32($b, 8)
$o = 24
for ($f = 0; $f -lt [Math]::Min(5, $n); $f++) {
    $incl = [BitConverter]::ToUInt32($b, $o + 8)
    if ($o + 16 + $incl -gt $b.Length) { break }
    $dst = ""
    for ($i = 0; $i -lt 6; $i++) { $dst += ("{0:X2}:" -f $b[$o + 16 + $i]) }
    $src = ""
    for ($i = 6; $i -lt 12; $i++) { $src += ("{0:X2}:" -f $b[$o + 16 + $i]) }
    $et = ('{0:X4}' -f (($b[$o + 28] -shl 8) -bor $b[$o + 29]))
    $msg = "frame $f`: dst=$dst src=$src ethertype=$et len=$incl"
    if ($et -eq "0806") {
        $op = ('{0:X4}' -f (($b[$o + 34] -shl 8) -bor $b[$o + 35]))
        $msg += " ARP-op=$op"
    }
    Write-Host $msg
    $o += 16 + $incl
}
