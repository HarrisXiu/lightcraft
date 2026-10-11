<# Read-only regression checks on the compiled MSI. Never installs or removes a product.
   Run after package.ps1: pwsh packaging/windows/test-msi.ps1 -Msi dist/release/<file>.msi #>
param([Parameter(Mandatory = $true)] [string] $Msi)
$ErrorActionPreference = 'Stop'
$installer = New-Object -ComObject WindowsInstaller.Installer
$database = $installer.OpenDatabase((Resolve-Path -LiteralPath $Msi).Path, 0)
function Read-Rows([string] $Query) {
  $view = $database.OpenView($Query)
  $null = $view.Execute()
  try {
    while ($record = $view.Fetch()) {
      $count = $record.GetType().InvokeMember('FieldCount', 'GetProperty', $null, $record, $null)
      $fields = for ($i = 1; $i -le $count; $i++) {
        $record.GetType().InvokeMember('StringData', 'GetProperty', $null, $record, @($i))
      }
      ,$fields
    }
  } finally { $null = $view.Close() }
}
function Assert-True([bool] $Value, [string] $Message) {
  if (-not $Value) { throw $Message }
}
$controls = @(Read-Rows 'SELECT `Control`, `Text` FROM `Control` WHERE `Dialog_` = ''ExitDialog''')
Assert-True (@($controls | Where-Object { $_[0] -eq 'Title' -and $_[1] -like '*[[]LIGHTCRAFT_COMPLETIONTITLE[]]*' }).Count -eq 1) 'Success title must use the completion property'
Assert-True (@($controls | Where-Object { $_[0] -eq 'Description' -and $_[1] -eq '[LIGHTCRAFT_COMPLETIONTEXT]' }).Count -eq 1) 'Success description must use the completion property'
$actions = @(Read-Rows 'SELECT `Source`, `Target` FROM `CustomAction` WHERE `Source` = ''LIGHTCRAFT_COMPLETIONTITLE''')
Assert-True (@($actions | Where-Object { $_[1] -eq 'LightCraft has been uninstalled' }).Count -eq 1) 'Remove must have an explicit success title'
$sequence = @(Read-Rows 'SELECT `Action`, `Condition`, `Sequence` FROM `InstallUISequence`')
function Get-Step([string] $Action) {
  $row = @($sequence | Where-Object { $_[0] -eq $Action })
  Assert-True ($row.Count -eq 1) "InstallUISequence must schedule $Action once"
  return @{ Condition = $row[0][1]; Sequence = [int]$row[0][2] }
}
$execute = Get-Step 'ExecuteAction'
$welcome = Get-Step 'MaintenanceWelcomeDlg'
$options = Get-Step 'CleanUninstallDlg'
# The wizard pages are where Remove is chosen: text set ahead of them would always see "not removing".
$removing = 'Installed AND NOT UPGRADINGPRODUCTCODE AND (WixUI_InstallMode="Remove" OR REMOVE~="ALL")'
foreach ($name in 'SetLIGHTCRAFT_COMPLETIONTITLE', 'SetLIGHTCRAFT_COMPLETIONTEXT') {
  $step = Get-Step $name
  Assert-True ($step.Condition -eq $removing) "$name must apply to Remove only, excluding upgrades"
  Assert-True ($step.Sequence -gt $welcome.Sequence -and $step.Sequence -gt $options.Sequence -and $step.Sequence -lt $execute.Sequence) "$name must run after the wizard pages and before execution"
}

# Uninstall from Apps must reach the same pages: Windows shows no dialogs for an MSI's own entry.
Assert-True ($welcome.Condition -like '* AND NOT LIGHTCRAFTUNINSTALL') 'Uninstall from Apps must skip the Welcome page'
Assert-True ($options.Condition -eq 'Installed AND NOT RESUME AND NOT Preselected AND NOT PATCH AND LIGHTCRAFTUNINSTALL') 'Uninstall from Apps must open on the options page'
$mode = Get-Step 'SetWixUI_InstallMode'
Assert-True ($mode.Condition -eq 'Installed AND LIGHTCRAFTUNINSTALL AND NOT PATCH' -and $mode.Sequence -lt $options.Sequence) 'Uninstall from Apps must select Remove before its first page'
$arp = @{}
foreach ($row in @(Read-Rows 'SELECT `Root`, `Key`, `Name`, `Value` FROM `Registry`')) {
  if ($row[0] -eq '2' -and $row[1] -eq 'Software\Microsoft\Windows\CurrentVersion\Uninstall\LightCraft') { $arp[$row[2]] = $row[3] }
}
Assert-True ($arp['DisplayName'] -eq 'LightCraft' -and $arp['DisplayVersion'] -eq '[ProductVersion]') 'Apps entry must name the product and its version'
Assert-True ($arp['UninstallString'] -eq 'MsiExec.exe /I[ProductCode] LIGHTCRAFTUNINSTALL=1') 'Uninstall in Apps must open the Remove pages with full UI'
$secureUi = @(Read-Rows 'SELECT `Value` FROM `Property` WHERE `Property` = ''SecureCustomProperties''')[0] -split ';'
Assert-True ($secureUi -contains 'LIGHTCRAFTUNINSTALL') 'LIGHTCRAFTUNINSTALL must be secure: Windows Installer ignores it otherwise for a non-elevated user, and Uninstall opens on the Welcome page'
Assert-True ($arp['QuietUninstallString'] -eq 'MsiExec.exe /X[ProductCode] /qn') 'Silent uninstall must stay a plain removal that keeps user data'
Assert-True (@(Read-Rows 'SELECT `Value` FROM `Property` WHERE `Property` = ''ARPSYSTEMCOMPONENT''' | Where-Object { $_[0] -eq '1' }).Count -eq 1) 'The product''s own dialog-less Apps entry must be hidden, or LightCraft is listed twice'
Assert-True (@($sequence | Where-Object { $_[0] -eq 'ExitDialog' -and $_[2] -eq '-1' }).Count -eq 1) 'Success page must remain restricted to successful completion'
Assert-True (@($sequence | Where-Object { $_[0] -eq 'FatalError' -and $_[2] -eq '-3' }).Count -eq 1) 'Failures must retain the failure page'
Assert-True (@($sequence | Where-Object { $_[0] -eq 'UserExit' -and $_[2] -eq '-2' }).Count -eq 1) 'Cancellation must retain the cancellation page'
Write-Output 'MSI completion regression checks passed (read-only).'

# Cleanup must be opt-in at the actual MSI property/sequence boundary, not just in helper tests.
$properties = @(Read-Rows 'SELECT `Property`, `Value` FROM `Property`')
foreach ($name in 'CLEANUSERDATA', 'DELETEORIGINALS') {
  Assert-True (@($properties | Where-Object { $_[0] -eq $name -and $_[1] -eq '1' }).Count -eq 0) "$name must default to unchecked"
}
$secure = @($properties | Where-Object { $_[0] -eq 'SecureCustomProperties' })[0][1] -split ';'
foreach ($name in 'CLEANUSERDATA', 'DELETEORIGINALS', 'CLEANCONFIGDIR', 'CLEANLIBRARYDIR') {
  Assert-True ($secure -contains $name) "$name must survive elevation"
}
$cleanup = @(Read-Rows 'SELECT `Type`, `Source`, `Target` FROM `CustomAction` WHERE `Action` = ''CleanLightcraftData''')[0]
$type = [int]$cleanup[0]
Assert-True (($type -band 0x600) -eq 0x600 -and ($type -band 0x800) -eq 0 -and ($type -band 0x40) -eq 0) 'Cleanup must be a checked, impersonated commit action'
Assert-True ($cleanup[1] -eq 'LightcraftCleanup' -and $cleanup[2] -like '*--delete-originals*') 'Cleanup must run the embedded helper with separate original-photo consent'
$executeSequence = @(Read-Rows 'SELECT `Action`, `Condition`, `Sequence` FROM `InstallExecuteSequence`')
$cleanStep = @($executeSequence | Where-Object { $_[0] -eq 'CleanLightcraftData' })[0]
Assert-True ($cleanStep[1] -eq 'Installed AND REMOVE~="ALL" AND NOT UPGRADINGPRODUCTCODE AND CLEANUSERDATA="1"') 'Cleanup must exclude install, repair, upgrades and ordinary uninstall'
$finalize = @($executeSequence | Where-Object { $_[0] -eq 'InstallFinalize' })[0]
Assert-True ([int]$cleanStep[2] -lt [int]$finalize[2]) 'Commit action must be scheduled before InstallFinalize'
# A refused cleanup rolls the uninstall back; the message must say so and what to do about it.
$failed = @(Read-Rows 'SELECT `Error`, `Message` FROM `Error`' | Where-Object { $_[0] -eq '1722' })
Assert-True ($failed.Count -eq 1 -and $failed[0][1] -like '*not uninstalled*Close LightCraft*lightcraft-uninstall.log*') 'A failed cleanup must explain that nothing was uninstalled and how to retry'
$exeActions = @(Read-Rows 'SELECT `Action`, `Type` FROM `CustomAction`' | Where-Object { ([int]$_[1] -band 0x3F) -in 2, 18, 34, 50 -and ([int]$_[1] -band 0x40) -eq 0 })
Assert-True ($exeActions.Count -eq 1 -and $exeActions[0][0] -eq 'CleanLightcraftData') 'Error 1722 is worded for the cleanup helper: no other checked EXE action may share it'
$events = @(Read-Rows 'SELECT `Dialog_`, `Control_`, `Event`, `Argument`, `Condition` FROM `ControlEvent`')
Assert-True (@($events | Where-Object { $_[0] -eq 'MaintenanceTypeDlg' -and $_[1] -eq 'RemoveButton' -and $_[2] -eq 'NewDialog' -and $_[3] -eq 'CleanUninstallDlg' }).Count -eq 1) 'Remove must offer the cleanup choices'
Assert-True (@($events | Where-Object { $_[0] -eq 'MaintenanceTypeDlg' -and $_[1] -eq 'RepairButton' -and $_[2] -eq 'NewDialog' -and $_[3] -eq 'VerifyReadyDlg' }).Count -eq 1) 'Repair must skip cleanup choices'
Assert-True (@($events | Where-Object { $_[0] -eq 'CleanUninstallDlg' -and $_[1] -eq 'Next' -and $_[2] -eq '[DELETEORIGINALS]' -and $_[3] -eq '{}' -and $_[4] -eq 'CLEANUSERDATA<>"1"' }).Count -eq 1) 'Turning cleanup off must clear stale original-photo consent'
Write-Output 'MSI cleanup regression checks passed (read-only).'
