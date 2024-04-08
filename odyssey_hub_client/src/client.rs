use std::time::Duration;

use interprocess::local_socket::{tokio::prelude::LocalSocketStream, traits::tokio::Stream, NameTypeSupport, ToFsName, ToNsName};
use odyssey_hub_service_interface::{greeter_client::GreeterClient, HelloRequest};
use tokio_util::sync::CancellationToken;
use tonic::transport::{Endpoint, Uri};
use tower::service_fn;

#[derive(Default)]
pub struct Client {
    pub end_token: CancellationToken,
}

impl Client {
    pub async fn run(&self) -> anyhow::Result<()> {
        let name = {
            use NameTypeSupport as Nts;
            match NameTypeSupport::query() {
                Nts::OnlyFs => "/tmp/odyhub.sock".to_fs_name().unwrap(),
                Nts::OnlyNs | Nts::Both => "@odyhub.sock".to_ns_name().unwrap(),
            }
        };

        // Await this here since we can't do a whole lot without a connection.
        // URI is ignored
        let channel = Endpoint::try_from("http://[::]:50051")?
            .connect_with_connector(service_fn(move |_: Uri| {
                let name = name.clone();
                async move {
                    let r = LocalSocketStream::connect(name).await?;
                    let r = odyssey_hub_service::service::LocalSocketStream::new(r);
                    dbg!(std::io::Result::Ok(r))
                }
            })).await.unwrap();
        let mut client = GreeterClient::new(channel);
        println!("connected");

        let request = tonic::Request::new(HelloRequest {
            name: "Tonic".into(),
        });
        tokio::time::sleep(Duration::from_secs(1)).await;

        println!("sending request");
        let response = client.say_hello(request).await.unwrap();

        println!("RESPONSE={:?}", response);
        Ok(())
    }
}
