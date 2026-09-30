# 独立诊断 UI Automation 的正文与选区能力，不操作窗口、不输出正文、不调用云端。
# 用 powershell -Mta -File 启动；观察期间把目标应用的测试输入框保持在前台。
[CmdletBinding()]
param(
    [string]$ProcessName = 'DingTalk',
    [ValidateRange(1, 120)]
    [int]$WatchSeconds = 30,
    [string]$ExpectedBefore = '今天',
    [string]$ExpectedAfter = '一起去公园',
    [string]$OutputPath
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

function Read-FocusedContext {
    $element = [System.Windows.Automation.AutomationElement]::FocusedElement
    if ($null -eq $element) { return @{ status = 'no_focus' } }
    $current = $element.Current
    $process = Get-Process -Id $current.ProcessId -ErrorAction SilentlyContinue
    if ($null -eq $process -or $process.ProcessName -ne $ProcessName) {
        return @{ status = 'waiting_for_target' }
    }
    $result = [ordered]@{
        status = 'unsupported'
        processId = $current.ProcessId
        class = $current.ClassName
        password = $current.IsPassword
    }
    if ($current.IsPassword) { $result.status = 'private'; return $result }
    $pattern = $null
    if (-not $element.TryGetCurrentPattern([System.Windows.Automation.TextPattern]::Pattern, [ref]$pattern)) {
        return $result
    }
    $selection = $pattern.GetSelection()
    $result.selectionCount = $selection.Length
    if ($selection.Length -ne 1) { $result.status = 'ambiguous_selection'; return $result }
    $range = $selection[0]
    $start = [System.Windows.Automation.Text.TextPatternRangeEndpoint]::Start
    $end = [System.Windows.Automation.Text.TextPatternRangeEndpoint]::End
    $unit = [System.Windows.Automation.Text.TextUnit]::Character
    $before = $range.Clone()
    $before.MoveEndpointByRange($end, $range, $start)
    $null = $before.MoveEndpointByUnit($start, $unit, -64)
    $after = $range.Clone()
    $after.MoveEndpointByRange($start, $range, $end)
    $null = $after.MoveEndpointByUnit($end, $unit, 32)
    $beforeText = $before.GetText(128)
    $afterText = $after.GetText(64)
    $selectedText = $range.GetText(128)
    $focus = [System.Windows.Automation.AutomationElement]::FocusedElement
    if ($null -eq $focus -or -not [System.Windows.Automation.Automation]::Compare($element, $focus)) {
        $result.status = 'focus_changed'
        return $result
    }
    $result.status = 'read'
    $result.beforeUnits = $beforeText.Length
    $result.afterUnits = $afterText.Length
    $result.selectedUnits = $selectedText.Length
    $result.beforeMatches = $beforeText -ceq $ExpectedBefore
    $result.afterMatches = $afterText -ceq $ExpectedAfter
    $result.beforeEndsWithWo = $beforeText.EndsWith('wo', [StringComparison]::Ordinal)
    $result.pass = $result.beforeMatches -and $result.afterMatches
    return $result
}

$deadline = [DateTime]::UtcNow.AddSeconds($WatchSeconds)
$previous = ''
while ([DateTime]::UtcNow -lt $deadline) {
    try {
        $sample = Read-FocusedContext
    }
    catch {
        # 异常消息可能携带提供方正文，只输出类型和 HRESULT。
        $sample = @{ status = 'error'; errorType = $_.Exception.GetType().FullName; hresult = $_.Exception.HResult }
    }
    $json = $sample | ConvertTo-Json -Compress
    if ($json -ne $previous) {
        Write-Output $json
        if ($OutputPath) { Add-Content -LiteralPath $OutputPath -Value $json -Encoding UTF8 }
        $previous = $json
    }
    Start-Sleep -Milliseconds 500
}
