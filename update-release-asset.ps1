# Update v0.4.2 release: delete old asset, upload new exe, update description
$Version = "0.4.2"
$exePath = Join-Path $PSScriptRoot "target\release\bundle\LLM-API-Proxy_v${Version}_x64_portable.exe"

# Read .env
$envFile = Join-Path $PSScriptRoot ".env"
if (Test-Path $envFile) {
    Get-Content $envFile | ForEach-Object {
        if ($_ -match '^\s*([^#][^=]+)\s*=\s*(.+)\s*$') {
            [Environment]::SetEnvironmentVariable($matches[1].Trim(), $matches[2].Trim(), "Process")
        }
    }
}

# Read release notes from CHANGELOG
$changelogPath = Join-Path $PSScriptRoot "CHANGELOG.md"
$releaseNotes = ""
if (Test-Path $changelogPath) {
    $content = Get-Content $changelogPath -Raw -Encoding UTF8
    $escapedVersion = [regex]::Escape($Version)
    $pattern = "(?s)## \[$escapedVersion\].*?(?=## \[|\z)"
    if ($content -match $pattern) {
        $section = $matches[0]
        $releaseNotes = ($section -split "`n" | Select-Object -Skip 1) -join "`n"
        $releaseNotes = $releaseNotes.Trim()
    }
}
if (-not $releaseNotes) { $releaseNotes = "Release v$Version" }

Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  Updating v$Version Release Assets" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan

# ========================================
# GitHub: delete old asset, upload new, update body
# ========================================
$githubToken = $env:GITHUB_TOKEN
if ($githubToken) {
    $repoOwner = "jiz-1789"
    $repoName = "LLM-API-Proxy"
    $releaseId = 367418951
    $headers = @{ Authorization = "token $githubToken"; Accept = "application/vnd.github.v3+json" }

    # 1. Get existing assets and delete old exe
    Write-Host "[GitHub] Fetching existing assets..." -ForegroundColor Yellow
    $assets = Invoke-RestMethod -Uri "https://api.github.com/repos/$repoOwner/$repoName/releases/$releaseId/assets" -Method Get -Headers $headers
    foreach ($asset in $assets) {
        if ($asset.name -like "*portable.exe") {
            Write-Host "[GitHub] Deleting old asset: $($asset.name) (ID: $($asset.id))" -ForegroundColor Yellow
            try {
                Invoke-RestMethod -Uri "https://api.github.com/repos/$repoOwner/$repoName/releases/assets/$($asset.id)" -Method Delete -Headers $headers | Out-Null
                Write-Host "[GitHub] Old asset deleted." -ForegroundColor Green
            } catch {
                Write-Host "[GitHub] Failed to delete: $($_.Exception.Message)" -ForegroundColor Red
            }
        }
    }

    # 2. Upload new exe
    Write-Host "[GitHub] Uploading new exe..." -ForegroundColor Yellow
    $uploadUrl = "https://uploads.github.com/repos/$repoOwner/$repoName/releases/$releaseId/assets?name=LLM-API-Proxy_v${Version}_x64_portable.exe"
    try {
        $uploadResponse = Invoke-RestMethod -Uri $uploadUrl -Method Post -Headers $headers -ContentType "application/octet-stream" -InFile $exePath
        Write-Host "[GitHub] Upload success! Download: $($uploadResponse.browser_download_url)" -ForegroundColor Green
    } catch {
        Write-Host "[GitHub] Upload failed: $($_.Exception.Message)" -ForegroundColor Red
    }

    # 3. Update release body
    Write-Host "[GitHub] Updating release description..." -ForegroundColor Yellow
    $body = @{ tag_name = "v$Version"; name = "v$Version"; body = $releaseNotes } | ConvertTo-Json
    try {
        Invoke-RestMethod -Uri "https://api.github.com/repos/$repoOwner/$repoName/releases/$releaseId" -Method Patch -Headers $headers -Body ([System.Text.Encoding]::UTF8.GetBytes($body)) | Out-Null
        Write-Host "[GitHub] Description updated." -ForegroundColor Green
    } catch {
        Write-Host "[GitHub] Failed to update description: $($_.Exception.Message)" -ForegroundColor Red
    }
}

# ========================================
# Gitee: delete old asset, upload new, update body
# ========================================
$giteeToken = $env:GITEE_TOKEN
if ($giteeToken) {
    $giteeOwner = "yilichenaiosi"
    $giteeRepo = "LLM-API-Proxy"
    $giteeReleaseId = 781445

    # 1. Get existing assets and delete old exe
    Write-Host "[Gitee] Fetching existing assets..." -ForegroundColor Yellow
    try {
        $releaseInfo = Invoke-RestMethod -Uri "https://gitee.com/api/v5/repos/$giteeOwner/$giteeRepo/releases/$giteeReleaseId?access_token=$giteeToken" -Method Get
        foreach ($asset in $releaseInfo.assets) {
            if ($asset.name -like "*portable.exe") {
                $assetId = $asset.id
                Write-Host "[Gitee] Deleting old asset: $($asset.name) (ID: $assetId)" -ForegroundColor Yellow
                try {
                    & curl.exe -s -X DELETE "https://gitee.com/api/v5/repos/$giteeOwner/$giteeRepo/releases/$giteeReleaseId/attach_files/$assetId?access_token=$giteeToken" 2>&1 | Out-Null
                    Write-Host "[Gitee] Old asset deleted." -ForegroundColor Green
                } catch {
                    Write-Host "[Gitee] Failed to delete: $($_.Exception.Message)" -ForegroundColor Red
                }
            }
        }
    } catch {
        Write-Host "[Gitee] Failed to fetch release: $($_.Exception.Message)" -ForegroundColor Red
    }

    # 2. Upload new exe
    Write-Host "[Gitee] Uploading new exe..." -ForegroundColor Yellow
    try {
        $uploadResult = & curl.exe -s -X POST "https://gitee.com/api/v5/repos/$giteeOwner/$giteeRepo/releases/$giteeReleaseId/attach_files" -F "access_token=$giteeToken" -F "file=@$exePath" 2>&1
        Write-Host "[Gitee] Upload complete." -ForegroundColor Green
    } catch {
        Write-Host "[Gitee] Upload failed: $($_.Exception.Message)" -ForegroundColor Red
    }

    # 3. Update release body
    Write-Host "[Gitee] Updating release description..." -ForegroundColor Yellow
    $giteeNotes = $releaseNotes | Out-File -FilePath "$env:TEMP\gitee_notes.txt" -Encoding UTF8 -NoNewline
    try {
        & curl.exe -s -X PATCH "https://gitee.com/api/v5/repos/$giteeOwner/$giteeRepo/releases/$giteeReleaseId" -F "access_token=$giteeToken" -F "tag_name=v$Version" -F "name=v$Version" -F "body=<$env:TEMP\gitee_notes.txt" -F "prerelease=false" 2>&1 | Out-Null
        Write-Host "[Gitee] Description updated." -ForegroundColor Green
    } catch {
        Write-Host "[Gitee] Failed to update description: $($_.Exception.Message)" -ForegroundColor Red
    }
}

Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  Update Complete!" -ForegroundColor Green
Write-Host "========================================" -ForegroundColor Cyan
