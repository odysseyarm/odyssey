# PowerShell script for ATS Lite 4.1

# Set features for ATS Lite 4.1
$FEATURES = "atslite-4-1"

# Run the sub build script for ATS Lite
& "$(Split-Path -Parent $MyInvocation.MyCommand.Definition)/sub/build_mcuboot_atslite.ps1"
