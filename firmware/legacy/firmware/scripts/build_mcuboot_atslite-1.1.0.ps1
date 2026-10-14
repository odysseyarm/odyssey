# PowerShell script for ATS Lite 1.1

# Set features for ATS Lite 1.1
$FEATURES = "atslite-1-1"

# Run the sub build script for ATS Lite
& "$(Split-Path -Parent $MyInvocation.MyCommand.Definition)/sub/build_mcuboot_atslite.ps1"
