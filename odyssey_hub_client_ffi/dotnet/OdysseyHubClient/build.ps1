Write-Host 'OdysseyHubClient' -ForegroundColor Blue
Write-Host 'Building native library (Rust)' -ForegroundColor Cyan
cargo build --manifest-path=../../Cargo.toml --release

Write-Host 'Building OdysseyHubClient' -ForegroundColor Cyan
dotnet pack .\OdysseyHubClient\

Write-Host 'Publishing OdysseyHubClient.Test' -ForegroundColor Cyan
dotnet publish .\OdysseyHubClient.Test\ -o .\output

Write-Host 'Running published OdysseyHubClient.Test' -ForegroundColor Cyan
Write-Host
./output/OdysseyHubClient.Test.exe