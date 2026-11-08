# Runs target\release\studio-run.exe against real Roblox Studio and checks its contract.
# Requires Studio to be installed and logged in. Build first with `cargo build --release`.
$ErrorActionPreference = 'Stop'

$root = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $root 'target\release\studio-run.exe'
$fixtures = Join-Path $PSScriptRoot 'fixtures'
$script:failures = 0

function Invoke-Tool([string[]]$Arguments) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $exe
    $psi.Arguments = ($Arguments | ForEach-Object { '"' + $_ + '"' }) -join ' '
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.StandardOutputEncoding = [Text.Encoding]::UTF8
    $psi.StandardErrorEncoding = [Text.Encoding]::UTF8
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $process = [Diagnostics.Process]::Start($psi)
    $out = $process.StandardOutput.ReadToEndAsync()
    $err = $process.StandardError.ReadToEndAsync()
    $process.WaitForExit()
    [pscustomobject]@{
        Code = $process.ExitCode
        Out = $out.Result
        Err = $err.Result
        Seconds = [math]::Round($clock.Elapsed.TotalSeconds, 1)
    }
}

function Test-Case {
    param(
        [string]$Name,
        [string[]]$Arguments,
        [int]$Code,
        [string[]]$OutHas = @(),
        [string[]]$OutLacks = @(),
        [string[]]$ErrHas = @()
    )
    $result = Invoke-Tool $Arguments
    $problems = @()
    if ($result.Code -ne $Code) { $problems += "exit code $($result.Code), expected $Code" }
    foreach ($text in $OutHas) { if (-not $result.Out.Contains($text)) { $problems += "stdout lacks '$text'" } }
    foreach ($text in $OutLacks) { if ($result.Out.Contains($text)) { $problems += "stdout contains '$text'" } }
    foreach ($text in $ErrHas) { if (-not $result.Err.Contains($text)) { $problems += "stderr lacks '$text'" } }
    if ($problems.Count -eq 0) {
        "PASS  $Name ($($result.Seconds)s)"
    } else {
        $script:failures++
        "FAIL  $Name ($($result.Seconds)s)"
        $problems | ForEach-Object { "      $_" }
        "      stdout: $($result.Out)"
        "      stderr: $($result.Err)"
    }
}

function Fixture([string]$Name) { Join-Path $fixtures $Name }

Test-Case 'missing script is a usage error' @('--script', (Fixture 'does-not-exist.luau')) 2 -ErrHas @('not found')
Test-Case 'wrong script extension is a usage error' @('--script', (Join-Path $PSScriptRoot 'run-integration.ps1')) 2
Test-Case 'bad timeout is a usage error' @('--script', (Fixture 'pass.luau'), '--timeout', '0') 2
Test-Case 'unknown flag is a usage error' @('--nope') 2

Test-Case 'passing script' @('--script', (Fixture 'pass.luau')) 0 `
    -OutHas @('fixture stdout line') -OutLacks @('__STUDIO_RUN_', 'print(', 'Failed to apply')
Test-Case 'throwing script reports the correct line' @('--script', (Fixture 'throw.luau')) 1 `
    -OutHas @('before throw') -OutLacks @('deliberate failure') -ErrHas @('RunScript:2:', 'fixture deliberate failure')
Test-Case 'yielding script runs to the end' @('--script', (Fixture 'yield.luau')) 0 -OutHas @('resumed after yield')
Test-Case 'top-level return still passes' @('--script', (Fixture 'early-return.luau')) 0 -OutHas @('before return')
Test-Case 'syntax error fails' @('--script', (Fixture 'syntax-error.luau'), '--timeout', '60') 1 -OutLacks @('never runs') -ErrHas @('did not compile')
Test-Case 'unicode and trailing comment survive' @('--script', (Fixture 'unicode.luau')) 0 -OutHas @("$([char]0x2713) $([char]0x2014) h$([char]0xE9)llo")
Test-Case 'hang hits the timeout' @('--script', (Fixture 'hang.luau'), '--timeout', '20') 3 -OutHas @('about to hang') -ErrHas @('timed out')
Test-Case 'visible mode passes' @('--script', (Fixture 'pass.luau'), '--visible') 0 -OutHas @('fixture stdout line')

$log = Join-Path ([IO.Path]::GetTempPath()) "studio-run-test-$PID.log"
Test-Case '--log run passes' @('--script', (Fixture 'pass.luau'), '--log', $log) 0
if ((Test-Path $log) -and (Get-Content $log -Raw).Contains('fixture stdout line')) {
    'PASS  --log file contains script output'
} else {
    $script:failures++
    'FAIL  --log file missing or lacks script output'
}
Remove-Item $log -ErrorAction SilentlyContinue

$leftovers = Get-ChildItem ([IO.Path]::GetTempPath()) -Filter 'studio-run-*' -Directory -ErrorAction SilentlyContinue
if ($leftovers) {
    $script:failures++
    "FAIL  temp folders left behind: $($leftovers.Name -join ', ')"
} else {
    'PASS  no temp folders left behind'
}

if ($script:failures -gt 0) {
    "$($script:failures) check(s) failed"
    exit 1
}
'All checks passed'
