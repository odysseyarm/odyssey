Write-Host 'OdysseyHubClient' -ForegroundColor Blue
Write-Host 'Building native library (Rust)' -ForegroundColor Cyan
cargo build --manifest-path=../../Cargo.toml --release

Write-Host 'Clearing nuget locals'
nuget locals all -clear

Write-Host 'Building Radiosity.OdysseyHubClient' -ForegroundColor Cyan
dotnet build .\OdysseyHubClient\
dotnet pack .\OdysseyHubClient\

Write-Host 'Publishing Radiosity.OdysseyHubClient.Test' -ForegroundColor Cyan
dotnet build .\OdysseyHubClient.Test\
dotnet publish .\OdysseyHubClient.Test\ -o .\output

Write-Host 'Running published Radiosity.OdysseyHubClient.Test' -ForegroundColor Cyan
Write-Host
./output/Radiosity.OdysseyHubClient.Test.exe