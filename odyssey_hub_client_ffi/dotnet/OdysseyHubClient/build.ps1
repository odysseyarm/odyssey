Write-Host 'OdysseyHubClient' -ForegroundColor Blue
Write-Host 'Building native library (Rust)' -ForegroundColor Cyan
cargo build --manifest-path=../../Cargo.toml --release
cargo build --manifest-path=../../Cargo.toml --release --target=i686-pc-windows-msvc

Write-Host 'Building Radiosity.OdysseyHubClient' -ForegroundColor Cyan
dotnet build .\OdysseyHubClient\
dotnet pack .\OdysseyHubClient\ -o nugets

Write-Host 'Publishing Radiosity.OdysseyHubClient.Test' -ForegroundColor Cyan
dotnet build .\OdysseyHubClient.Test\
dotnet publish .\OdysseyHubClient.Test\ -o .\output

Write-Host 'Running published Radiosity.OdysseyHubClient.Test' -ForegroundColor Cyan
Write-Host
./output/Radiosity.OdysseyHubClient.Test.exe
