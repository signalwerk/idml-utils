//! The model can move between threads and be shared.
#![recursion_limit = "512"]

fn send_sync<T: Send + Sync + 'static>() {}

#[test]
fn model_types_are_send_and_sync() {
    send_sync::<idml::Package>();
    #[cfg(feature = "owned")]
    send_sync::<idml::OwnedPackage>();
    #[cfg(feature = "owned")]
    send_sync::<idml::model::Story>();
    send_sync::<idml::Error>();
}
