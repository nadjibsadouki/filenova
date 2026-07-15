<#
PowerShell test script for FileNova installers

Creates a TXT report with installer info, installs silently (NSIS preferred),
verifies installation files, launches the app briefly, checks Event Log for errors,
and produces a manual checklist. Prompts before uninstall.

Usage (run as Admin for system install):
    powershell -ExecutionPolicy Bypass -File .\test-filenova-installer.ps1

This script does not sign installers, does not modify FileNova source, and
does not automate UI clicks. It requires only built-in PowerShell cmdlets.
#>

param()

function Write-ReportLine {
    param($text)
    "$((Get-Date).ToString('s')) - $text" | Tee-Object -FilePath $global:reportPath -Append
}

function Get-FileVersionSafe {
    param($path)
    try {
        $item = Get-Item -LiteralPath $path -ErrorAction Stop
        if ($item.VersionInfo.ProductVersion) { return $item.VersionInfo.ProductVersion }
        if ($item.VersionInfo.FileVersion) { return $item.VersionInfo.FileVersion }
        return ''
    } catch { return '' }
}

function Get-MsiProductVersion {
    param($msiPath)
    try {
        $installer = New-Object -ComObject WindowsInstaller.Installer
        $db = $installer.OpenDatabase($msiPath, 0)
        $view = $db.OpenView("SELECT `Value` FROM `Property` WHERE `Property`='ProductVersion'")
        $view.Execute()
        $rec = $view.Fetch()
        if ($rec -ne $null) { return $rec.StringData(1) }
        return ''
    } catch { return '' }
}

function HashFileSha256 {
    param($path)
    try { return (Get-FileHash -Path $path -Algorithm SHA256 -ErrorAction Stop).Hash } catch { return '' }
}

# Report file
$global:reportPath = Join-Path -Path (Get-Location) -ChildPath 'test-filenova-report.txt'
if (Test-Path $global:reportPath) { Remove-Item $global:reportPath -Force }

Write-ReportLine "FileNova installer test started"

# Locate installers
$bundleDir = Join-Path -Path 'src-tauri\target\release\bundle' -ChildPath ''
$nsisPath = Get-ChildItem -Path $bundleDir -Recurse -ErrorAction SilentlyContinue | Where-Object { $_.Name -match 'setup.exe$' } | Select-Object -First 1 -ExpandProperty FullName -ErrorAction SilentlyContinue
$msiPath = Get-ChildItem -Path $bundleDir -Recurse -ErrorAction SilentlyContinue | Where-Object { $_.Extension -ieq '.msi' } | Select-Object -First 1 -ExpandProperty FullName -ErrorAction SilentlyContinue

Write-ReportLine "Bundle directory: $bundleDir"

if ($nsisPath) {
    $nsisSize = (Get-Item -LiteralPath $nsisPath).Length
    $nsisVer = Get-FileVersionSafe -path $nsisPath
    $nsisHash = HashFileSha256 -path $nsisPath
    Write-ReportLine "NSIS installer: $nsisPath"
    Write-ReportLine "NSIS size (bytes): $nsisSize"
    Write-ReportLine "NSIS version: $nsisVer"
    Write-ReportLine "NSIS SHA256: $nsisHash"
} else {
    Write-ReportLine "NSIS installer: NOT FOUND"
}

if ($msiPath) {
    $msiSize = (Get-Item -LiteralPath $msiPath).Length
    $msiVer = Get-MsiProductVersion -msiPath $msiPath
    if (-not $msiVer) { $msiVer = Get-FileVersionSafe -path $msiPath }
    $msiHash = HashFileSha256 -path $msiPath
    Write-ReportLine "MSI installer: $msiPath"
    Write-ReportLine "MSI size (bytes): $msiSize"
    Write-ReportLine "MSI version: $msiVer"
    Write-ReportLine "MSI SHA256: $msiHash"
} else {
    Write-ReportLine "MSI installer: NOT FOUND"
}

# Determine install dir
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if ($isAdmin) {
    $defaultInstallDir = Join-Path $env:ProgramFiles 'FileNova'
} else {
    $defaultInstallDir = Join-Path $env:LOCALAPPDATA 'Programs\FileNova'
}

Write-ReportLine "Running as Administrator: $isAdmin"
Write-ReportLine "Default install dir: $defaultInstallDir"

# Install step (prefer NSIS)
$installDir = $defaultInstallDir
$installResult = $false
$installLog = Join-Path (Get-Location) 'test-filenova-install.log'
if (Test-Path $installLog) { Remove-Item $installLog -Force }

if ($nsisPath) {
    Write-ReportLine "Installing with NSIS setup.exe (silent)"
    try {
        $args = @('/S', "/D=$installDir")
        $p = Start-Process -FilePath $nsisPath -ArgumentList $args -PassThru -Wait -WindowStyle Hidden
        Write-ReportLine "NSIS exit code: $($p.ExitCode)"
        if ($p.ExitCode -eq 0) { $installResult = $true }
    } catch {
        Write-ReportLine "NSIS install failed: $($_.Exception.Message)"
    }
} elseif ($msiPath) {
    Write-ReportLine "Installing with MSI (quiet)"
    try {
        $msiArgs = "/i `"$msiPath`" /qn /norestart" + " /L*v `"$installLog`""
        $p = Start-Process -FilePath msiexec.exe -ArgumentList $msiArgs -PassThru -Wait -NoNewWindow
        Write-ReportLine "msiexec exit code: $($p.ExitCode)"
        if ($p.ExitCode -eq 0) { $installResult = $true }
    } catch {
        Write-ReportLine "MSI install failed: $($_.Exception.Message)"
    }
} else {
    Write-ReportLine "No installer found to install. Aborting installation step."
}

Start-Sleep -Seconds 2

# Verification after install
Write-ReportLine "Post-install verification starting"
if (Test-Path $installDir) {
    Write-ReportLine "Install directory exists: $installDir"
    $installedFiles = Get-ChildItem -Path $installDir -Recurse -ErrorAction SilentlyContinue | Select-Object FullName, Length
    Write-ReportLine "Files installed (listing up to 200 entries):"
    $installedFiles | Select-Object -First 200 | ForEach-Object { Write-ReportLine " - $($_.FullName) ($($_.Length) bytes)" }
} else {
    Write-ReportLine "Install directory NOT FOUND: $installDir"
}

# Try to find files from the installed files list first (more reliable)
$foundExe = $null
$foundPdfium = $null
if ($installedFiles) {
    $foundExe = ($installedFiles | Where-Object { $_.FullName -match '(?i)\\filenova\.exe$' } | Select-Object -First 1).FullName
    $foundPdfium = ($installedFiles | Where-Object { $_.FullName -match '(?i)\\pdfium\\pdfium\.dll$' -or $_.FullName -match '(?i)\\pdfium\.dll$' } | Select-Object -First 1).FullName
    if ($foundExe) { Write-ReportLine "Found executable (from install listing): $foundExe" }
    if ($foundPdfium) { Write-ReportLine "Found pdfium.dll (from install listing): $foundPdfium"; Write-ReportLine "pdfium size: $((Get-Item $foundPdfium).Length)" }
}

# If not found yet, check common install locations
if (-not $foundExe -or -not $foundPdfium) {
    $possibleInstallDirs = @()
    if ($installDir) { $possibleInstallDirs += $installDir }
    if ($env:LOCALAPPDATA) { $possibleInstallDirs += (Join-Path $env:LOCALAPPDATA 'Programs\FileNova') }
    if ($env:ProgramFiles) { $possibleInstallDirs += (Join-Path $env:ProgramFiles 'FileNova') }
    try { $pf86 = ${env:ProgramFiles(x86)}; if ($pf86) { $possibleInstallDirs += (Join-Path $pf86 'FileNova') } } catch { }
    $possibleInstallDirs = $possibleInstallDirs | Where-Object { $_ -and -not [string]::IsNullOrWhiteSpace($_) } | Select-Object -Unique

    foreach ($dir in $possibleInstallDirs) {
        if ($dir -is [System.Array]) { $dirStr = $dir[0] } else { $dirStr = [string]$dir }
        if (-not $dirStr) { continue }
        $exeCandidates = @(
            Join-Path $dirStr 'filenova.exe',
            Join-Path $dirStr 'FileNova.exe',
            Join-Path $dirStr 'filenova\\filenova.exe'
        )
        foreach ($c in $exeCandidates) { if (Test-Path $c) { $foundExe = $c; break } }
        if ($foundExe) { break }
    }

    foreach ($dir in $possibleInstallDirs) {
        if ($dir -is [System.Array]) { $dirStr = $dir[0] } else { $dirStr = [string]$dir }
        if (-not $dirStr) { continue }
        $pdfCandidates = @(
            Join-Path $dirStr 'pdfium\\pdfium.dll',
            Join-Path $dirStr 'pdfium.dll',
            Join-Path $dirStr 'resources\\pdfium\\pdfium.dll'
        )
        foreach ($p in $pdfCandidates) { if (Test-Path $p) { $foundPdfium = $p; break } }
        if ($foundPdfium) { break }
    }

    if ($foundExe) { Write-ReportLine "Found executable: $foundExe" } else { Write-ReportLine "filenova.exe not found in checked locations: $($possibleInstallDirs -join '; ')" }
    if ($foundPdfium) { Write-ReportLine "Found pdfium.dll at: $foundPdfium"; Write-ReportLine "pdfium size: $((Get-Item $foundPdfium).Length)" } else { Write-ReportLine "pdfium.dll NOT found in checked locations: $($possibleInstallDirs -join '; ')" }
}

# Check Tauri-related files (approximate)
$tauriFiles = @('appsettings.json','tauri.conf.json','libcef.dll')
$tauriFound = @()
foreach ($dir in $possibleInstallDirs) {
    foreach ($f in $tauriFiles) {
        try {
            $hits = Get-ChildItem -Path $dir -Recurse -Filter $f -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName -ErrorAction SilentlyContinue
            if ($hits) { $tauriFound += $hits; Write-ReportLine "Found Tauri-related file: $hits" }
        } catch { }
    }
}
if ($tauriFound.Count -eq 0) { Write-ReportLine "No common Tauri support files were found by name (this is not necessarily fatal)." }

# Launch app briefly and check process + Event Log errors
$appLaunched = $false
if ($foundExe) {
    try {
        $startTime = Get-Date
        $proc = Start-Process -FilePath $foundExe -PassThru -WindowStyle Minimized
        Start-Sleep -Seconds 6
        $stillRunning = Get-Process -Id $proc.Id -ErrorAction SilentlyContinue
        if ($stillRunning) { Write-ReportLine "Process is running (Id=$($proc.Id))."; $appLaunched = $true } else { Write-ReportLine "Process did not remain running or exited quickly." }

        # Check Application event log for errors since start
        try {
            $errors = Get-WinEvent -FilterHashtable @{LogName='Application'; Level=2; StartTime=$startTime} -ErrorAction SilentlyContinue | Where-Object { $_.Message -match 'filenova' -or $_.ProviderName -match 'filenova' }
            if ($errors) {
                Write-ReportLine "Windows Application errors referencing 'filenova' since app start:"
                $errors | ForEach-Object { Write-ReportLine " - $($_.TimeCreated): $($_.ProviderName) - $($_.Id) - $($_.Message -replace "`r`n"," / ")" }
            } else {
                Write-ReportLine "No Application-level Windows errors referencing 'filenova' found since app start."
            }
        } catch {
            Write-ReportLine "Could not query Windows Event Log: $($_.Exception.Message)"
        }

        # Close the process we started
        try { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue; Write-ReportLine "Closed launched process." } catch { }
    } catch {
        Write-ReportLine "Failed to launch executable: $($_.Exception.Message)"
    }
} else {
    Write-ReportLine "Skipping launch: executable not found." 
}

# Manual test checklist (do not automate clicks)
Write-ReportLine "\n=== Manual Test Checklist ==="
@(
    'Compress Images: select sample images, compress, verify output quality and size reduction',
    'Convert Images: convert sample images between formats, verify outputs',
    'Resize Images: resize sample images, verify dimensions',
    'Merge PDF: merge sample PDFs and verify page order',
    'Split PDF: split a PDF and verify resulting files',
    'Organize Pages: reorder pages and save, verify result',
    'Images to PDF: create PDF from images and verify',
    'PDF to Images: convert PDF to images; verify conversion without separate PDFium install',
    'Preview: open previews for images/PDFs',
    'Open Image: open image via button; verify external viewer/open behavior',
    'Open Folder: open containing folder via button'
) | ForEach-Object { Write-ReportLine " - $_" }

# Ask before uninstall
$doUninstall = Read-Host "Do you want to uninstall FileNova now? (Y/N) [N]"
if ($doUninstall -and $doUninstall.ToUpper().StartsWith('Y')) {
    Write-ReportLine "User confirmed uninstall. Attempting uninstall..."
    # Try NSIS uninstaller first
    $uninstallExe = Join-Path $installDir 'Uninstall.exe'
    if (Test-Path $uninstallExe) {
        try {
            $p = Start-Process -FilePath $uninstallExe -ArgumentList '/S' -PassThru -Wait
            Write-ReportLine "Uninstall exit code: $($p.ExitCode)"
        } catch { Write-ReportLine "Uninstall failed: $($_.Exception.Message)" }
    } else {
        Write-ReportLine "Uninstall.exe not found. To uninstall manually, remove the install directory or use Control Panel / Settings -> Apps."
    }
    # Final check
    if (-not (Test-Path $installDir)) { Write-ReportLine "Install directory removed." } else { Write-ReportLine "Install directory still present: $installDir" }
} else {
    Write-ReportLine "Skipping uninstall as per user choice."
}

Write-ReportLine "Test completed. Report saved to: $global:reportPath"

Write-Output "Script finished. Report: $global:reportPath"
