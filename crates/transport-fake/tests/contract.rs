use filebridge_transport_api::{Backend, Destination};
use filebridge_transport_fake::FakeTransport;

#[tokio::test]
async fn keeps_the_transport_contract() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("dest");
    std::fs::create_dir(&root).unwrap();
    let dest = Destination {
        transport: FakeTransport::ID.into(),
        backend: Backend::Local,
        root: root.to_string_lossy().into_owned(),
    };
    filebridge_transport_api::contract::check(&FakeTransport::default(), &dest, dir.path()).await;
}
