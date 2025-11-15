# Chocolatey Packaging Notes

These steps capture exactly what’s needed to publish `v0.1.0`. Update the versioned filenames/URLs when you cut a new release.

## 1. Build & zip the Windows binary

```powershell
cargo build --release --target x86_64-pc-windows-msvc

$zipPath = "anv-0.1.0-x86_64-pc-windows-msvc.zip"
Compress-Archive `
	-LiteralPath "target\x86_64-pc-windows-msvc\release\anv.exe" `
	-DestinationPath $zipPath -Force
```

Upload the zip to the GitHub release so it’s available at:
`https://github.com/Vedant-Asati03/anv/releases/download/v0.1.0/anv-0.1.0-x86_64-pc-windows-msvc.zip`

## 2. Record the checksum

```powershell
(Get-FileHash .\anv-0.1.0-x86_64-pc-windows-msvc.zip -Algorithm SHA256).Hash.ToLower()
```

Result used for `v0.1.0`:
`e1e22a53224e3a503602c95b6cb0012ce5a74951d57ce69e835d7a53e79ed1b5`

## 3. Update package metadata

- `anv.nuspec` → `<version>0.1.0</version>` and ensure URLs/tags look right.
- `tools/chocolateyInstall.ps1` → update `$version`, `$downloadUrl`, and `$checksum`.
- `legal/VERIFICATION.txt` → update Version, Download URL, and checksum line.

## 4. Pack and test locally

```powershell
choco pack .\packaging\chocolatey\anv.nuspec

# Elevated PowerShell required for install test
choco install anv --source . --force --yes
choco uninstall anv --yes
```

If you’re not running as Administrator, Chocolatey will fail when it tries to write into `C:\ProgramData`. Re-run the install/uninstall commands from an elevated shell to verify the package end-to-end.

## 5. Publish to Chocolatey.org

```powershell
choco push .\anv.0.1.0.nupkg --source https://push.chocolatey.org/ --api-key <your_api_key>
```

Moderation typically takes a few minutes. Watch your email for automated review feedback.
