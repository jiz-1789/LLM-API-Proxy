# LLM-API-Proxy 发布脚本
# 用法：.\publish.ps1 [版本号]
# 示例：.\publish.ps1 0.1.13

param(
    [Parameter(Mandatory=$true)]
    [string]$Version,
    [switch]$GitHub = $true,
    [switch]$Gitee = $false
)

# 读取 .env 文件
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

# 配置
$exePath = Join-Path $PSScriptRoot "target\release\bundle\LLM-API-Proxy_v${Version}_x64_portable.exe"

# 检查 exe 是否存在
if (-not (Test-Path $exePath)) {
    Write-Error "未找到 exe 文件: $exePath"
    Write-Host "请先运行: cargo tauri build"
    exit 1
}

# 读取 CHANGELOG 中对应版本的更新说明
$changelogPath = Join-Path $PSScriptRoot "CHANGELOG.md"
$releaseNotes = ""
if (Test-Path $changelogPath) {
    $content = Get-Content $changelogPath -Raw
    # 匹配版本号对应的更新说明
    $pattern = "## \[$Version\].*?(?=## \[|\z)"
    if ($content -match $pattern) {
        $section = $matches[0]
        # 移除版本号行，只保留变更内容
        $releaseNotes = ($section -split "`n" | Select-Object -Skip 1) -join "`n"
        $releaseNotes = $releaseNotes.Trim()
    }
}

if (-not $releaseNotes) {
    $releaseNotes = "发布 v$Version"
}

Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  LLM-API-Proxy 发布脚本" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "版本: $Version"
Write-Host "发布平台: $(if ($GitHub) { 'GitHub ' })$(if ($Gitee) { 'Gitee' })"
Write-Host ""

# ========================================
# GitHub 发布
# ========================================
if ($GitHub) {
    $token = $env:GITHUB_TOKEN
    if (-not $token) {
        Write-Error "未找到 GITHUB_TOKEN，请在 .env 文件中配置"
        exit 1
    }

    $repoOwner = $env:GITHUB_REPO_OWNER ?? "jiz-1789"
    $repoName = $env:GITHUB_REPO_NAME ?? "LLM-API-Proxy"

    Write-Host "[GitHub] 正在创建 Release..." -ForegroundColor Yellow
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
        Write-Host "[GitHub] Release 创建成功! ID: $releaseId" -ForegroundColor Green

        # 上传 exe
        Write-Host "[GitHub] 正在上传 exe..." -ForegroundColor Yellow
        $uploadUrl = "https://uploads.github.com/repos/$repoOwner/$repoName/releases/$releaseId/assets?name=LLM-API-Proxy_v${Version}_x64_portable.exe"
        $uploadResponse = Invoke-RestMethod -Uri $uploadUrl -Method Post -Headers $headers -ContentType "application/octet-stream" -InFile $exePath
        Write-Host "[GitHub] 上传成功!" -ForegroundColor Green
        Write-Host "[GitHub] 下载链接: $($uploadResponse.browser_download_url)" -ForegroundColor Cyan
    } catch {
        Write-Error "[GitHub] 发布失败: $_"
    }
    Write-Host ""
}

# ========================================
# Gitee 发布
# ========================================
if ($Gitee) {
    $token = $env:GITEE_TOKEN
    if (-not $token) {
        Write-Error "未找到 GITEE_TOKEN，请在 .env 文件中配置"
        exit 1
    }

    $repoOwner = $env:GITEE_REPO_OWNER ?? "yilichenaiosi"
    $repoName = $env:GITEE_REPO_NAME ?? "LLM-API-Proxy"
    $targetBranch = $env:GITEE_TARGET_BRANCH ?? "main"

    Write-Host "[Gitee] 正在创建 Release..." -ForegroundColor Yellow

    # Gitee API 使用表单数据
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
        Write-Host "[Gitee] Release 创建成功! ID: $releaseId" -ForegroundColor Green

        # 上传 exe
        Write-Host "[Gitee] 正在上传 exe..." -ForegroundColor Yellow
        
        # Gitee 上传附件 API
        $uploadUrl = "https://gitee.com/api/v5/repos/$repoOwner/$repoName/releases/$releaseId/attach_files"
        
        # 使用 multipart/form-data 上传文件
        $fileBytes = [System.IO.File]::ReadAllBytes($exePath)
        $fileEnc = [System.Text.Encoding]::GetEncoding('ISO-8859-1').GetString($fileBytes)
        $boundary = [System.Guid]::NewGuid().ToString()
        $LF = "`r`n"
        
        $bodyLines = (
            "--$boundary",
            "Content-Disposition: form-data; name=`"access_token`"$LF",
            $token,
            "--$boundary",
            "Content-Disposition: form-data; name=`"file`; filename=`"LLM-API-Proxy_v${Version}_x64_portable.exe`"",
            "Content-Type: application/octet-stream$LF",
            $fileEnc,
            "--$boundary--$LF"
        ) -join $LF

        $uploadResponse = Invoke-RestMethod -Uri $uploadUrl -Method Post -ContentType "multipart/form-data; boundary=$boundary" -Body $bodyLines
        Write-Host "[Gitee] 上传成功!" -ForegroundColor Green
        Write-Host "[Gitee] 下载链接: $($uploadResponse.browser_download_url)" -ForegroundColor Cyan
    } catch {
        Write-Error "[Gitee] 发布失败: $_"
    }
    Write-Host ""
}

Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  发布完成!" -ForegroundColor Green
Write-Host "========================================" -ForegroundColor Cyan
