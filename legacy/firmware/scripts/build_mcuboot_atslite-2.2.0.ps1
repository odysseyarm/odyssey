# PowerShell script for ATS Lite 2.2

# Set features for ATS Lite 2.2
$FEATURES = "atslite-2-2"

# Run the sub build script for ATS Lite
& "$(Split-Path -Parent $MyInvocation.MyCommand.Definition)/sub/build_mcuboot_atslite.ps1"
