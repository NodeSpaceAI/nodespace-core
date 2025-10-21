@echo off
REM MCP Bridge - Translates stdio (Claude Code) to HTTP (NodeSpace)
REM
REM This script reads JSON-RPC 2.0 messages from stdin (from Claude Code),
REM POSTs them to the NodeSpace MCP HTTP server, and writes responses to stdout.
REM
REM Usage: Configure Claude Code to use this script as MCP server
REM Example %%APPDATA%%\Claude\claude.json:
REM   {
REM     "mcpServers": {
REM       "nodespace": {
REM         "type": "stdio",
REM         "command": "cmd.exe",
REM         "args": ["/c", "C:\\path\\to\\scripts\\mcp-bridge.bat"]
REM       }
REM     }
REM   }

setlocal enabledelayedexpansion

REM Configuration - Set defaults if not already defined
if not defined MCP_SERVER_URL set "MCP_SERVER_URL=http://localhost:3001/mcp"
if not defined CONNECT_TIMEOUT set "CONNECT_TIMEOUT=5"
if not defined REQUEST_TIMEOUT set "REQUEST_TIMEOUT=30"

REM Check if PowerShell is available
where powershell >nul 2>nul
if errorlevel 1 (
    echo Error: PowerShell is required but not found in PATH >&2
    exit /b 1
)

REM Read stdin line by line and forward to HTTP server
for /f "delims=" %%A in ('powershell -Command "Get-Content"') do (
    set "line=%%A"
    if not "!line!"=="" (
        REM Forward to HTTP server using PowerShell
        for /f "delims=" %%B in ('powershell -Command "
            $line = '!line!'
            try {
                $response = Invoke-RestMethod -Uri '!MCP_SERVER_URL!' -Method Post -Body $line -ContentType 'application/json' -TimeoutSec !REQUEST_TIMEOUT! -ErrorAction Stop
                $response | ConvertTo-Json -Compress
            } catch {
                $requestId = $line | ConvertFrom-Json | Select-Object -ExpandProperty id -ErrorAction SilentlyContinue
                if ($null -eq $requestId) { $requestId = 'null' }
                Write-Output \"{\\\"jsonrpc\\\":\\\"2.0\\\",\\\"id\\\":$requestId,\\\"error\\\":{\\\"code\\\":-32603,\\\"message\\\":\\\"Failed to connect to NodeSpace MCP server at '!MCP_SERVER_URL!'\\\"}}\"
            }
        "') do (
            echo %%B
        )
    )
)

endlocal
