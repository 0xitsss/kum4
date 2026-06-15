param(
    [int]$NodeCount = 3,
    [int]$BasePort = 8080,
    [switch]$TorEnabled
)

$BaseDir = Join-Path $PSScriptRoot ".." | Resolve-Path
$Binary = Join-Path $BaseDir "target\release\kum4.exe"
if (!(Test-Path $Binary)) {
    $Binary = Join-Path $BaseDir "target\debug\kum4.exe"
    if (!(Test-Path $Binary)) {
        Write-Host "Building kum4 first..." -ForegroundColor Yellow
        Push-Location $BaseDir
        cargo build 2>&1 | Out-Null
        Pop-Location
    }
}

$Seed = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
$DataDir = Join-Path $BaseDir "target\testnet"
if (Test-Path $DataDir) { Remove-Item -Recurse -Force $DataDir }
New-Item -ItemType Directory -Path $DataDir -Force | Out-Null

$Procs = @()
try {
    for ($i = 0; $i -lt $NodeCount; $i++) {
        $Port = $BasePort + $i
        $NodeDir = Join-Path $DataDir "node$i"
        New-Item -ItemType Directory -Path $NodeDir -Force | Out-Null

        $env:SEED_PHRASE = $Seed
        $env:NODE_PORT = "$Port"
        $env:NODE_ID = "kum4-test-$i"
        $env:NODE_VERSION = "0.0.4-test"
        $env:DB_PATH = $NodeDir
        $env:KEY_PATH = Join-Path $NodeDir "key.kum4"
        $env:WEB_HOST = "127.0.0.1"
        $env:TOR_ENABLED = if ($TorEnabled) { "true" } else { "false" }
        if ($TorEnabled) { $env:TOR_PROXY = "socks5://127.0.0.1:19050" }
        $env:ADMIN_TOKEN = "test-token-$i"
        $env:TRON_RPC_URL = "https://test-tron.example"
        $env:BSC_RPC_URL = "https://test-bsc.example"
        $env:MEMPOOL_URL = "https://test-mempool.example"
        $env:BTC_NETWORK = "regtest"
        $env:TRON_USDT_CONTRACT = "TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf"
        $env:BSC_USDT_CONTRACT = "0x337610d27c682E347C9cD60BD4b3b107C9d34dDd"
        $env:MIN_USDT_AMOUNT = "0.0"
        $env:PROFIT_FEE_USD = "0.0"
        $env:REBALANCE_THRESHOLD = "9999999"
        $env:BTC_RESERVE_INDEX = "0"
        $env:MAX_PENDING_PER_CHAIN = "1"
        $env:TRON_CONFIRMATIONS = "1"
        $env:BSC_CONFIRMATIONS = "1"
        $env:BOT_TOKEN = ""
        $env:ADMIN_USER_ID = "0"
        $env:RUST_LOG = "info"

        $logFile = Join-Path $NodeDir "output.log"
        Write-Host "Starting node $i on port $Port..." -ForegroundColor Green
        $proc = Start-Process -FilePath $Binary -NoNewWindow -PassThru `
            -RedirectStandardOutput $logFile -RedirectStandardError $logFile
        $Procs += $proc
        Start-Sleep -Seconds 1
    }

    Write-Host "`n=== Testnet running ===" -ForegroundColor Cyan
    Write-Host "Node 0: http://127.0.0.1:$($BasePort)" -ForegroundColor Cyan
    Write-Host "Node 1: http://127.0.0.1:$($BasePort+1)" -ForegroundColor Cyan
    Write-Host "Node 2: http://127.0.0.1:$($BasePort+2)" -ForegroundColor Cyan
    Write-Host "`nPress Ctrl+C to stop all nodes.`n" -ForegroundColor Yellow

    while ($true) { Start-Sleep -Seconds 10 }
}
finally {
    Write-Host "`nStopping all nodes..." -ForegroundColor Yellow
    foreach ($proc in $Procs) {
        if (!$proc.HasExited) { $proc.Kill() }
    }
    Remove-Item -Recurse -Force $DataDir -ErrorAction SilentlyContinue
    Write-Host "Done." -ForegroundColor Green
}
