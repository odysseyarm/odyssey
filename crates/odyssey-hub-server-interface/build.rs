fn main() -> std::io::Result<()> {
    tonic_build::compile_protos("proto/service_interface.proto")
}
