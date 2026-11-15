# Ace Term PowerShell Integration
$global:__AceOrigPrompt = $function:prompt
function global:prompt {
    $ec = if ($LASTEXITCODE -ne $null) { $LASTEXITCODE } else { 0 }
    [Console]::Write("$([char]27)]133;D;$ec$([char]7)")
    $loc = (Get-Location).Path -replace '\\', '/'
    [Console]::Write("$([char]27)]7;file://localhost/$loc$([char]7)")
    [Console]::Write("$([char]27)]133;A$([char]7)")
    if ($global:__AceOrigPrompt) { & $global:__AceOrigPrompt } else { "PS $($executionContext.SessionState.Path.CurrentLocation)> " }
}
