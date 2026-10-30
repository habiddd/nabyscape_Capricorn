<#
.SYNOPSIS
  Samples the private working set of nabyscape.exe over time (the Rust counterpart of
  tools/Measure-Performance.ps1's memory sampling).

.DESCRIPTION
  Private working set is the RAM only this process uses (shared DLL pages excluded), the figure
  the memory design targets. Run it against a release build in the state you want to measure:
  dock hidden, closed, panel open, Settings open, voice active. Samples go to a CSV; a summary
  (min / median / p95 / max) is printed. Use -Start to launch the executable first; otherwise it
  attaches to a running nabyscape.exe.

.EXAMPLE
  ./measure-memory.ps1 -Executable ..\target\x86_64-pc-windows-msvc\release\nabyscape.exe -Start -Seconds 60 -Label closed-idle
#>
param(
    [string]$Executable,
    [switch]$Start,
    [ValidateRange(1, 3600)][int]$Seconds = 30,
    [ValidateRange(1, 60)][int]$IntervalSeconds = 1,
    [string]$Label = 'sample',
    [string]$Report
)
$ErrorActionPreference = 'Stop'

Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class NabyMemory {
    [StructLayout(LayoutKind.Sequential)]
    struct Counters {
        public uint Size, Faults;
        public UIntPtr Peak, Total, QuotaPeakPaged, QuotaPaged, QuotaPeakNonpaged, QuotaNonpaged, Pagefile, PeakPagefile, Commit, PrivateWorking;
        public ulong SharedCommit;
    }
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool K32GetProcessMemoryInfo(IntPtr process, ref Counters counters, uint size);
    // Returns { private working set, total working set, private commit } in MiB.
    public static double[] Read(IntPtr handle) {
        var c = new Counters { Size = (uint)Marshal.SizeOf<Counters>() };
        if (!K32GetProcessMemoryInfo(handle, ref c, c.Size)) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
        return new[] { c.PrivateWorking.ToUInt64() / 1048576d, c.Total.ToUInt64() / 1048576d, c.Commit.ToUInt64() / 1048576d };
    }
}
'@

$started = $null
if ($Start) {
    if (-not $Executable) { throw '-Start needs -Executable.' }
    $started = Start-Process -FilePath ([IO.Path]::GetFullPath($Executable)) -ArgumentList '--background' -PassThru
    Start-Sleep -Seconds 3
    $process = $started
} else {
    $process = Get-Process -Name nabyscape -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $process) { throw 'nabyscape.exe is not running. Start it or pass -Start -Executable.' }
}

$samples = [Collections.Generic.List[object]]::new()
try {
    $end = [DateTimeOffset]::UtcNow.AddSeconds($Seconds)
    while ([DateTimeOffset]::UtcNow -lt $end) {
        if ($process.HasExited) { throw 'nabyscape.exe exited while sampling.' }
        $m = [NabyMemory]::Read($process.Handle)
        $samples.Add([pscustomobject]@{
            label = $Label; at = [DateTimeOffset]::UtcNow.ToString('o')
            privateWorkingSetMiB = [math]::Round($m[0], 2); totalWorkingSetMiB = [math]::Round($m[1], 2); privateCommitMiB = [math]::Round($m[2], 2)
        })
        Start-Sleep -Seconds $IntervalSeconds
    }
} finally {
    if ($started -and -not $started.HasExited) { & $Executable --exit; $null = $started.WaitForExit(10000) }
}

if ($Report) { $samples | Export-Csv -LiteralPath $Report -NoTypeInformation -Encoding utf8 }
$sorted = @($samples | ForEach-Object privateWorkingSetMiB | Sort-Object)
$pick = { param($q) $sorted[[math]::Min($sorted.Count - 1, [math]::Floor($q * ($sorted.Count - 1)))] }
[pscustomobject]@{
    label = $Label; samples = $sorted.Count
    minMiB = $sorted[0]; medianMiB = (& $pick 0.5); p95MiB = (& $pick 0.95); maxMiB = $sorted[-1]
} | Format-List
