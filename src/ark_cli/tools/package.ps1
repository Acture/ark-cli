[CmdletBinding()]
param(
	[switch] $SkipBuild,
	[ValidateSet('release', 'debug')]
	[string] $Profile = 'release',
	# Cargo features for the build, for example bundled-tools for the Windows release.
	[string[]] $Features = @()
)

$ErrorActionPreference = 'Stop'
$workspaceRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../..'))

if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
	throw 'Use package.sh to package a macOS or Linux executable.'
}

Push-Location -LiteralPath $workspaceRoot
try {
	if (-not $SkipBuild) {
		$buildArguments = @('build', '--locked', '-p', 'arknights-cli')
		if ($Profile -eq 'release') {
			$buildArguments += '--release'
		}
		if ($Features.Count -gt 0) {
			$buildArguments += @('--features', ($Features -join ','))
		}
		& cargo @buildArguments
		if ($LASTEXITCODE -ne 0) {
			throw "cargo build failed with exit code $LASTEXITCODE"
		}
	}

	$metadataText = & cargo metadata --locked --no-deps --format-version 1
	if ($LASTEXITCODE -ne 0) {
		throw "cargo metadata failed with exit code $LASTEXITCODE"
	}
	$metadata = $metadataText | ConvertFrom-Json
	$targetDirectory = [string] $metadata.target_directory
	$compilerDetails = & rustc -vV
	if ($LASTEXITCODE -ne 0) {
		throw "rustc failed with exit code $LASTEXITCODE"
	}
	$hostLine = $compilerDetails | Where-Object { $_ -match '^host: ' } | Select-Object -First 1
	if ($hostLine -notmatch '^host: ([A-Za-z0-9_]+)-.*windows') {
		throw "Cannot determine the Windows build architecture from: $hostLine"
	}
	$architecture = $Matches[1]
	$executable = Join-Path (Join-Path $targetDirectory $Profile) 'ark-cli.exe'
	if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
		throw "Executable not found at $executable; build that profile before using -SkipBuild."
	}
	$versionOutput = & $executable --version
	if ($LASTEXITCODE -ne 0 -or $versionOutput -notmatch '^ark-cli ([A-Za-z0-9.+-]+)$') {
		throw "Cannot determine the executable version: $versionOutput"
	}
	$version = $Matches[1]
	$packageName = "ark-cli-$version-windows-$architecture"
	$releaseRoot = [IO.Path]::GetFullPath((Join-Path $workspaceRoot 'assets/releases'))
	$destination = [IO.Path]::GetFullPath((Join-Path $releaseRoot $packageName))
	$archive = [IO.Path]::GetFullPath((Join-Path $releaseRoot "$packageName.zip"))
	$checksum = "$archive.sha256"
	$workspacePrefix = $workspaceRoot.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
	if (-not $releaseRoot.StartsWith($workspacePrefix, [StringComparison]::OrdinalIgnoreCase)) {
		throw "Release root is outside the workspace: $releaseRoot"
	}
	foreach ($outputPath in @($destination, $archive, $checksum)) {
		if ([IO.Path]::GetDirectoryName($outputPath) -ne $releaseRoot) {
			throw "Release destination is outside the intended directory: $outputPath"
		}
		if ($null -ne (Get-Item -LiteralPath $outputPath -Force -ErrorAction SilentlyContinue)) {
			throw "Refusing to overwrite existing release output: $outputPath"
		}
	}

	New-Item -ItemType Directory -Path $releaseRoot -Force | Out-Null
	New-Item -ItemType Directory -Path $destination | Out-Null
	Copy-Item -LiteralPath $executable -Destination (Join-Path $destination 'ark-cli.exe')
	Copy-Item -LiteralPath (Join-Path $PSScriptRoot '../RELEASE.md') `
		-Destination (Join-Path $destination 'README.md')
	foreach ($notice in @('LICENSE', 'NOTICE')) {
		Copy-Item -LiteralPath (Join-Path $workspaceRoot $notice) `
			-Destination (Join-Path $destination $notice)
	}
	# Third-party notices for the battle runtime's tools, embedded or downloaded at setup.
	Copy-Item -LiteralPath (Join-Path $workspaceRoot 'src/battle_runtime/licenses/THIRD_PARTY.md') `
		-Destination (Join-Path $destination 'THIRD_PARTY.md')
	$defaultConfig = "assets_dir = 'assets'`nclient_dir = 'client'`ngamedata_dir = 'client/_gamedata'`n"
	[IO.File]::WriteAllText((Join-Path $destination 'rogue.toml'), $defaultConfig)
	Compress-Archive -LiteralPath $destination -DestinationPath $archive -CompressionLevel Optimal
	$hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
	[IO.File]::WriteAllText($checksum, "$hash  $packageName.zip`n")
	Write-Output "Packaged $destination"
	Write-Output "Archive $archive"
	Write-Output "SHA256 $checksum"
}
finally {
	Pop-Location
}
