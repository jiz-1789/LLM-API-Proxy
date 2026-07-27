# LLM-API-Proxy Publish Script
# Usage: .\publish.ps1 [version]
# Example: .\publish.ps1 0.1.13

param(
    [Parameter(Mandatory=$true)]
    [string]$Version,
    [switch]$GitHub = $true,
    [switch]$Gitee = $false
)

# Read .env file
$envFile = Join-Path $PSScriptRoot ".env"
if (Test-Path $envFile) {
    Get-Content $envFile | ForEach-Object {
        if ($_ -match '^\s*([^#][^=]+)\s*=\s*(.+)\s*$') {
            $name = $matches[1].Trim()
            $value = $matches[2].Trim()
            [Environment]::SetEnvironmentVariable($name, $value, "Process")
        }
    }
}

# Config
$exePath = Join-Path $PSScriptRoot "target\release\bundle\LLM-API-Proxy_v${Version}_x64_portable.exe"

# Check exe exists
if (-not (Test-Path $exePath)) {
    Write-Error "EXE not found: $exePath"
    Write-Host "Please run: cargo tauri build"
    exit 1
}

# Read release notes from CHANGELOG
$changelogPath = Join-Path $PSScriptRoot "CHANGELOG.md"
$releaseNotes = ""
if (Test-Path $changelogPath) {
    $content = Get-Content $changelogPath -Raw
    $pattern = "## \[$Version\].*?(?=## \[|\z)"
    if ($content -match $pattern) {
        $section = $matches[0]
        $releaseNotes = ($section -split "`n" | Select-Object -Skip 1) -join "`n"
        $releaseNotes = $releaseNotes.Trim()
    }
}

if (-not $releaseNotes) {
    $releaseNotes = "Release v$Version"
}

Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  LLM-API-Proxy Publish Script" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "Version: $Version"
Write-Host "Platforms: $(if ($GitHub) { 'GitHub ' })$(if ($Gitee) { 'Gitee' })"
Write-Host ""

# ========================================
# GitHub Release
# ========================================
if ($GitHub) {
    $token = $env:GITHUB_TOKEN
    if (-not $token) {
        Write-Error "GITHUB_TOKEN not found in .env"
        exit 1
    }

    $repoOwner = if ($env:GITHUB_REPO_OWNER) { $env:GITHUB_REPO_OWNER } else { "jiz-1789" }
    $repoName = if ($env:GITHUB_REPO_NAME) { $env:GITHUB_REPO_NAME } else { "LLM-API-Proxy" }

    Write-Host "[GitHub] Creating release..." -ForegroundColor Yellow
    $headers = @{
        Authorization = "token $token"
        Accept = "application/vnd.github.v3+json"
    }

    $body = @{
        tag_name = "v$Version"
        name = "v$Version"
        body = $releaseNotes
        draft = $false
        prerelease = $false
    } | ConvertTo-Json

    try {
        $response = Invoke-RestMethod -Uri "https://api.github.com/repos/$repoOwner/$repoName/releases" -Method Post -Headers $headers -Body ([System.Text.Encoding]::UTF8.GetBytes($body))
        $releaseId = $response.id
        Write-Host "[GitHub] Release created! ID: $releaseId" -ForegroundColor Green

        Write-Host "[GitHub] Uploading exe..." -ForegroundColor Yellow
        $uploadUrl = "https://uploads.github.com/repos/$repoOwner/$repoName/releases/$releaseId/assets?name=LLM-API-Proxy_v${Version}_x64_portable.exe"
        $uploadResponse = Invoke-RestMethod -Uri $uploadUrl -Method Post -Headers $headers -ContentType "application/octet-stream" -InFile $exePath
        Write-Host "[GitHub] Upload success!" -ForegroundColor Green
        Write-Host "[GitHub] Download: $($uploadResponse.browser_download_url)" -ForegroundColor Cyan
    } catch {
        Write-Error "[GitHub] Failed: $($_.Exception.Message)"
    }
    Write-Host ""
}

# ========================================
# Gitee Release
# ========================================
if ($Gitee) {
    $token = $env:GITEE_TOKEN
    if (-not $token) {
        Write-Error "GITEE_TOKEN not found in .env"
        exit 1
    }

    $repoOwner = if ($env:GITEE_REPO_OWNER) { $env:GITEE_REPO_OWNER } else { "yilichenaiosi" }
    $repoName = if ($env:GITEE_REPO_NAME) { $env:GITEE_REPO_NAME } else { "LLM-API-Proxy" }
    $targetBranch = if ($env:GITEE_TARGET_BRANCH) { $env:GITEE_TARGET_BRANCH } else { "main" }

    Write-Host "[Gitee] Creating release..." -ForegroundColor Yellow

    $giteeBody = @{
        access_token = $token
        tag_name = "v$Version"
        target_commitish = $targetBranch
        name = "v$Version"
        body = $releaseNotes
        prerelease = "false"
    }

    try {
        $response = Invoke-RestMethod -Uri "https://gitee.com/api/v5/repos/$repoOwner/$repoName/releases" -Method Post -Body $giteeBody
        $releaseId = $response.id
        Write-Host "[Gitee] Release created! ID: $releaseId" -ForegroundColor Green

        Write-Host "[Gitee] Uploading exe..." -ForegroundColor Yellow
        
        # Use curl for Gitee upload (more reliable for multipart/form-data)
        $uploadUrl = "https://gitee.com/api/v5/repos/$repoOwner/$repoName/releases/$releaseId/attach_files"
        $curlArgs = @(
            "-X", "POST",
            $uploadUrl,
            "-F", "access_token=$token",
            "-F", "file=@$exePath"
        )
        
        $curlOutput = & curl.exe @curlArgs 2>&1
        $curlOutput | Write-Host
        
        if ($LASTEXITCODE -eq 0) {
            Write-Host "[Gitee] Upload success!" -ForegroundColor Green
            # Parse download URL from response
            try {
                $jsonResponse = $curlOutput | ConvertFrom-Json
                Write-Host "[Gitee] Download: $($jsonResponse.browser_download_url)" -ForegroundColor Cyan
            } catch {
                Write-Host "[Gitee] Download URL: https://gitee.com/$repoOwner/$repoName/releases/download/v$Version/LLM-API-Proxy_v${Version}_x64_portable.exe" -ForegroundColor Cyan
            }
        } else {
            Write-Error "[Gitee] Upload failed with exit code: $LASTEXITCODE"
        }
    } catch {
        Write-Error "[Gitee] Failed: $($_.Exception.Message)"
    }
    Write-Host ""
}

Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  Publish Complete!" -ForegroundColor Green
Write-Host "========================================" -ForegroundColor Cyan
