//! The rows' one way to a product served on a Unix socket in a temp dir.

/// A channel to the server on `socket`.
pub(crate) async fn channel(socket: std::path::PathBuf) -> tonic::transport::Channel {
    tonic::transport::Endpoint::from_static("http://[::]:50051")
        .connect_with_connector(tower::service_fn(move |_: tonic::transport::Uri| {
            let socket = socket.clone();
            async move {
                Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(tokio::net::UnixStream::connect(socket).await?))
            }
        }))
        .await
        .expect("connects")
}
